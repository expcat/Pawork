//! macOS sandbox-exec（Seatbelt）硬隔离后端。
//!
//! 纯函数 [`escape_seatbelt_string`] / [`generate_seatbelt_profile`] 在所有平台编译
//! 并单测；spawn 后端 [`SandboxExecBackend`] 与探测仅在 macOS 编译。

use std::path::PathBuf;

use crate::{NetworkMode, SandboxPolicy};

/// sandbox-exec 二进制路径（macOS 固定位置）。
pub const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

/// 转义 Seatbelt profile 字符串字面量：双引号包裹，转义 `\` 与 `"`。
pub fn escape_seatbelt_string(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    out.push('"');
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 把 [`SandboxPolicy`] 编译为 Seatbelt profile 文本（version 1 s-expression）。
///
/// 强项：文件系统 read/write/deny 与网络 Enforce。`max_procs` 在 Seatbelt 中无可靠
/// 原语，profile 仅以注释标注，实际限制依赖 RLIMIT_NPROC（诚实降级）。
pub fn generate_seatbelt_profile(policy: &SandboxPolicy, workspace_roots: &[PathBuf]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "(version 1)");
    let _ = writeln!(s, "(deny default)");
    let _ = writeln!(s, "(allow process-exec)");
    let _ = writeln!(s, "(allow process-fork)");
    let _ = writeln!(s, "(allow signal (target self))");
    let _ = writeln!(s, "(allow sysctl-read)");
    let _ = writeln!(s, "(allow mach-lookup)");
    let _ = writeln!(s, "(allow ipc-posix-shm)");
    let _ = writeln!(s, "(allow file-read-metadata)");
    for sys in [
        "/bin",
        "/private/etc",
        "/private/var/db/dyld",
        "/System",
        "/usr/bin",
        "/usr/lib",
        "/usr/share",
        "/Library/Apple",
        "/Library/Frameworks",
        "/System/Library/Frameworks",
    ] {
        let _ = writeln!(
            s,
            "(allow file-read* (subpath {}))",
            escape_seatbelt_string(sys)
        );
    }
    for r in &policy.filesystem.read_roots {
        let _ = writeln!(
            s,
            "(allow file-read* (subpath {}))",
            escape_seatbelt_string(&r.to_string_lossy())
        );
    }
    for w in &policy.filesystem.write_roots {
        let _ = writeln!(
            s,
            "(allow file-read* (subpath {}))",
            escape_seatbelt_string(&w.to_string_lossy())
        );
        let _ = writeln!(
            s,
            "(allow file-write* (subpath {}))",
            escape_seatbelt_string(&w.to_string_lossy())
        );
    }
    for root in workspace_roots {
        let _ = writeln!(
            s,
            "(allow file-read* (subpath {}))",
            escape_seatbelt_string(&root.to_string_lossy())
        );
    }
    for d in &policy.filesystem.deny {
        let _ = writeln!(
            s,
            "(deny file* (subpath {}))",
            escape_seatbelt_string(&d.to_string_lossy())
        );
    }
    match policy.network_mode {
        NetworkMode::Enforce => {
            let _ = writeln!(s, "(deny network*)");
            // not implemented, awaiting egress broker: network_allow_hosts 仅记录意图，
            // 不编译进 profile（Seatbelt 需解析后的 endpoint filter），Enforce 下网络保持全拒。
            if !policy.network_allow_hosts.is_empty() {
                let _ = writeln!(
                    s,
                    "; hostname allowlist intentionally not compiled: Seatbelt requires resolved endpoint filters; network remains denied"
                );
            }
        }
        NetworkMode::Hint | NetworkMode::Off => {
            let _ = writeln!(s, "(allow network*)");
        }
    }
    if let Some(max) = policy.max_procs {
        let _ = writeln!(
            s,
            "; max_procs={max}: not enforceable by Seatbelt; rely on RLIMIT_NPROC"
        );
    }
    s
}

#[cfg(target_os = "macos")]
mod seatbelt {
    use super::{generate_seatbelt_profile, SANDBOX_EXEC_PATH};
    use crate::{
        apply_soft_restrictions, SandboxBackend, SandboxError, SandboxPolicy, SandboxProcess,
        SandboxProcessSpec,
    };
    use agent_domain::CancellationToken;
    use async_trait::async_trait;
    use process_runtime::ProcessRuntime;
    use std::path::Path;
    use std::sync::OnceLock;

    fn run_probe() -> (bool, String) {
        let p = Path::new(SANDBOX_EXEC_PATH);
        if !p.exists() {
            return (false, format!("{SANDBOX_EXEC_PATH} not found"));
        }
        match std::process::Command::new(SANDBOX_EXEC_PATH)
            .args(["-p", "(version 1) (allow default)", "/usr/bin/true"])
            .output()
        {
            Ok(output) if output.status.success() => (true, String::new()),
            Ok(output) => (
                false,
                format!("sandbox-exec smoke probe failed: {}", output.status),
            ),
            Err(error) => (false, format!("sandbox-exec probe failed: {error}")),
        }
    }

    pub(super) fn probe() -> &'static (bool, String) {
        static PROBE: OnceLock<(bool, String)> = OnceLock::new();
        PROBE.get_or_init(run_probe)
    }

    pub fn probe_reason() -> String {
        probe().1.clone()
    }

    /// macOS sandbox-exec（Seatbelt）硬隔离后端。
    #[derive(Clone, Debug)]
    pub struct SandboxExecBackend {
        runtime: ProcessRuntime,
        available: bool,
    }

    impl SandboxExecBackend {
        pub fn with_runtime(runtime: ProcessRuntime) -> Self {
            let available = probe().0;
            Self { runtime, available }
        }
    }

    #[async_trait]
    impl SandboxBackend for SandboxExecBackend {
        fn id(&self) -> &'static str {
            "sandbox_exec"
        }

        fn available(&self) -> bool {
            self.available
        }

        async fn spawn(
            &self,
            mut spec: SandboxProcessSpec,
            policy: SandboxPolicy,
            cancel: CancellationToken,
        ) -> Result<SandboxProcess, SandboxError> {
            if !self.available {
                return Err(SandboxError::BackendUnavailable("sandbox_exec"));
            }
            apply_soft_restrictions(&mut spec, &policy)?;
            let profile = generate_seatbelt_profile(&policy, &spec.workspace_roots);
            let inner_program = spec.command.program.clone();
            let inner_args = std::mem::take(&mut spec.command.args);
            let mut argv: Vec<String> = Vec::with_capacity(inner_args.len() + 3);
            argv.push("-p".into());
            argv.push(profile);
            argv.push(inner_program);
            argv.extend(inner_args);
            spec.command.program = SANDBOX_EXEC_PATH.to_string();
            spec.command.args = argv;
            let (events, handle) = self
                .runtime
                .spawn_stream(spec.command, cancel)
                .await
                .map_err(SandboxError::Process)?;
            Ok(SandboxProcess {
                events,
                _handle: handle,
            })
        }
    }
}

#[cfg(target_os = "macos")]
pub use seatbelt::{probe_reason, SandboxExecBackend};

#[cfg(not(target_os = "macos"))]
pub fn probe_reason() -> String {
    "sandbox-exec only available on macOS".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FilesystemPolicy;

    #[test]
    fn seatbelt_escape_quotes_and_backslashes() {
        assert_eq!(escape_seatbelt_string("/a/b"), "\"/a/b\"");
        assert_eq!(escape_seatbelt_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    #[test]
    fn profile_denies_network_when_enforce() {
        let policy = SandboxPolicy {
            network_mode: NetworkMode::Enforce,
            ..Default::default()
        };
        let profile = generate_seatbelt_profile(&policy, &[]);
        assert!(profile.contains("(deny network*)"));
    }

    #[test]
    fn profile_allows_network_when_hint() {
        let policy = SandboxPolicy {
            network_mode: NetworkMode::Hint,
            ..Default::default()
        };
        let profile = generate_seatbelt_profile(&policy, &[]);
        assert!(profile.contains("(allow network*)"));
        assert!(!profile.contains("(deny network*)"));
    }

    #[test]
    fn profile_emits_deny_for_secret_paths() {
        let policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                deny: vec![PathBuf::from("/Users/x/.ssh")],
                ..Default::default()
            },
            ..Default::default()
        };
        let profile = generate_seatbelt_profile(&policy, &[]);
        assert!(profile.contains("(deny file* (subpath \"/Users/x/.ssh\"))"));
    }

    #[test]
    fn profile_emits_write_roots_as_file_write() {
        let policy = SandboxPolicy {
            filesystem: FilesystemPolicy {
                write_roots: vec![PathBuf::from("/tmp/ws")],
                ..Default::default()
            },
            ..Default::default()
        };
        let profile = generate_seatbelt_profile(&policy, &[]);
        assert!(profile.contains("(allow file-write* (subpath \"/tmp/ws\"))"));
    }

    #[test]
    fn profile_notes_max_procs_unenforced() {
        let policy = SandboxPolicy {
            max_procs: Some(8),
            ..Default::default()
        };
        let profile = generate_seatbelt_profile(&policy, &[]);
        assert!(profile.contains("max_procs=8"));
        assert!(profile.contains("RLIMIT_NPROC"));
    }

    #[test]
    fn profile_includes_version_header() {
        let profile = generate_seatbelt_profile(&SandboxPolicy::default(), &[]);
        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(deny default)"));
    }
}
