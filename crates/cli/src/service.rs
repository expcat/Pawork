//! `pawork service install|start|stop`：默认 dry-run，`--apply` 才改系统。

use std::path::{Path, PathBuf};
use std::process::Command;

use pawork_app::DEFAULT_INSTANCE;

use crate::ops::service_name;
use crate::{CliError, ServiceCommand};

pub fn run_service(command: ServiceCommand, instance: &str, json: bool) -> Result<(), CliError> {
    let instance = if instance.trim().is_empty() {
        DEFAULT_INSTANCE
    } else {
        instance
    };
    let (action, apply) = match command {
        ServiceCommand::Install { apply } => ("install", apply),
        ServiceCommand::Start { apply } => ("start", apply),
        ServiceCommand::Stop { apply } => ("stop", apply),
    };
    let exe = std::env::current_exe()
        .map_err(CliError::Io)?
        .display()
        .to_string();
    let name = service_name(instance);
    let plan = install_definition(&exe, &name, instance);
    let activation = activation_hint(&name);
    if apply {
        execute_service_action(action, &exe, &name, instance)?;
    }
    if json {
        println!(
            "{}",
            serde_json::json!({
                "service": name,
                "action": action,
                "dry_run": !apply,
                "plan": plan,
                "applied": apply,
                "platform": std::env::consts::OS,
            })
        );
    } else if apply {
        eprintln!("applied {action} for service '{name}'");
    } else {
        eprintln!("install plan for service '{name}':\n{plan}\nthen activate:\n  {activation}");
        eprintln!("(dry-run; pass --apply to modify the system)");
    }
    Ok(())
}

fn install_definition(exe: &str, name: &str, instance: &str) -> String {
    if cfg!(windows) {
        format!(
            "sc create {name} binPath= \"\\\"{exe}\\\" --instance {instance} gui serve\" start= auto displayname= \"Pawork Core\""
        )
    } else if cfg!(target_os = "macos") {
        let plist = launchd_plist(exe, name, instance);
        format!("{plist}")
    } else if cfg!(target_os = "linux") {
        format!(
            "[Unit]\nDescription=Pawork Core\n\n[Service]\nExecStart={exe} --instance {instance} gui serve\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n"
        )
    } else {
        "unsupported platform for service install".to_string()
    }
}

fn launchd_plist(exe: &str, name: &str, instance: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{name}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>--instance</string>
    <string>{instance}</string>
    <string>gui</string>
    <string>serve</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
</dict>
</plist>
"#
    )
}

fn activation_hint(name: &str) -> String {
    if cfg!(windows) {
        format!("sc start {name}")
    } else if cfg!(target_os = "macos") {
        format!("launchctl load ~/Library/LaunchAgents/{name}.plist")
    } else if cfg!(target_os = "linux") {
        format!("systemctl --user enable --now {name}.service")
    } else {
        "unsupported platform".into()
    }
}

fn execute_service_action(
    action: &str,
    exe: &str,
    name: &str,
    instance: &str,
) -> Result<(), CliError> {
    if cfg!(windows) {
        match action {
            "install" => run_cmd(
                "sc",
                &[
                    "create",
                    name,
                    "binPath=",
                    &format!("\"{exe}\" --instance {instance} gui serve"),
                    "start=",
                    "auto",
                    "displayname=",
                    "Pawork Core",
                ],
            ),
            "start" => run_cmd("sc", &["start", name]),
            "stop" => apply_teardown(&windows_stop_steps(name)),
            _ => Err(CliError::Usage(format!("unknown service action {action}"))),
        }
    } else if cfg!(target_os = "macos") {
        let plist_path = launch_agent_path(name)?;
        match action {
            "install" => {
                if let Some(parent) = plist_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&plist_path, launchd_plist(exe, name, instance))?;
                Ok(())
            }
            "start" => run_cmd("launchctl", &["load", &plist_path.display().to_string()]),
            "stop" => apply_teardown(&macos_stop_steps(&plist_path)),
            _ => Err(CliError::Usage(format!("unknown service action {action}"))),
        }
    } else if cfg!(target_os = "linux") {
        let unit = user_unit_path(name)?;
        match action {
            "install" => {
                if let Some(parent) = unit.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&unit, install_definition(exe, name, instance))?;
                Ok(())
            }
            "start" => run_cmd("systemctl", &["--user", "start", &format!("{name}.service")]),
            "stop" => apply_teardown(&linux_stop_steps(name, &unit)),
            _ => Err(CliError::Usage(format!("unknown service action {action}"))),
        }
    } else {
        Err(CliError::Usage(
            "unsupported platform for service install".into(),
        ))
    }
}

fn launch_agent_path(name: &str) -> Result<PathBuf, CliError> {
    let home = std::env::var_os("HOME").ok_or_else(|| CliError::Usage("HOME is not set".into()))?;
    Ok(PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{name}.plist")))
}

fn user_unit_path(name: &str) -> Result<PathBuf, CliError> {
    let home = std::env::var_os("HOME").ok_or_else(|| CliError::Usage("HOME is not set".into()))?;
    Ok(PathBuf::from(home)
        .join(".config/systemd/user")
        .join(format!("{name}.service")))
}

fn run_cmd(program: &str, args: &[&str]) -> Result<(), CliError> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(CliError::Io)?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Usage(format!(
            "{program} {} failed with {status}",
            args.join(" ")
        )))
    }
}

/// `stop --apply` 回收步骤。前缀命令尽力执行，最后一步（删单元文件或
/// `sc delete`）必须落地，避免 KeepAlive / 登录再拉起。
#[derive(Debug, Clone, PartialEq, Eq)]
enum TeardownStep {
    Run { program: String, args: Vec<String> },
    RemoveFile { path: PathBuf },
}

fn macos_stop_steps(plist_path: &Path) -> Vec<TeardownStep> {
    vec![
        TeardownStep::Run {
            program: "launchctl".into(),
            args: vec!["unload".into(), plist_path.display().to_string()],
        },
        TeardownStep::RemoveFile {
            path: plist_path.to_path_buf(),
        },
    ]
}

fn linux_stop_steps(name: &str, unit_path: &Path) -> Vec<TeardownStep> {
    vec![
        TeardownStep::Run {
            program: "systemctl".into(),
            args: vec![
                "--user".into(),
                "stop".into(),
                format!("{name}.service"),
            ],
        },
        TeardownStep::Run {
            program: "systemctl".into(),
            args: vec![
                "--user".into(),
                "disable".into(),
                format!("{name}.service"),
            ],
        },
        TeardownStep::RemoveFile {
            path: unit_path.to_path_buf(),
        },
    ]
}

fn windows_stop_steps(name: &str) -> Vec<TeardownStep> {
    vec![
        TeardownStep::Run {
            program: "sc".into(),
            args: vec!["stop".into(), name.to_string()],
        },
        TeardownStep::Run {
            program: "sc".into(),
            args: vec!["delete".into(), name.to_string()],
        },
    ]
}

fn apply_teardown(steps: &[TeardownStep]) -> Result<(), CliError> {
    let last = steps.len().saturating_sub(1);
    for (index, step) in steps.iter().enumerate() {
        let required = index == last;
        match step {
            TeardownStep::Run { program, args } => {
                let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                match run_cmd(program, &argv) {
                    Ok(()) => {}
                    Err(err) if required => return Err(err),
                    Err(_) => {}
                }
            }
            TeardownStep::RemoveFile { path } => match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(CliError::Io(err)),
            },
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_teardown_plan_deletes_macos_plist() {
        let plist = PathBuf::from("/tmp/Library/LaunchAgents/pawork.dev.plist");
        let steps = macos_stop_steps(&plist);
        assert!(
            steps.iter().any(|step| matches!(
                step,
                TeardownStep::Run { program, args }
                    if program == "launchctl"
                        && args.first().map(String::as_str) == Some("unload")
                        && args.get(1).map(String::as_str) == Some(plist.to_str().unwrap_or(""))
            )),
            "macOS stop must launchctl unload: {steps:?}"
        );
        assert!(
            steps.iter().any(|step| matches!(
                step,
                TeardownStep::RemoveFile { path } if path == &plist
            )),
            "macOS stop must delete LaunchAgents plist: {steps:?}"
        );
    }

    fn run_args_eq(step: &TeardownStep, program: &str, expected: &[&str]) -> bool {
        match step {
            TeardownStep::Run {
                program: got_program,
                args,
            } => {
                got_program == program
                    && args
                        .iter()
                        .map(String::as_str)
                        .eq(expected.iter().copied())
            }
            TeardownStep::RemoveFile { .. } => false,
        }
    }

    #[test]
    fn stop_teardown_plan_disables_and_deletes_linux_unit() {
        let unit = PathBuf::from("/tmp/.config/systemd/user/pawork.dev.service");
        let steps = linux_stop_steps("pawork.dev", &unit);
        assert!(
            steps
                .iter()
                .any(|step| run_args_eq(step, "systemctl", &["--user", "stop", "pawork.dev.service"])),
            "Linux stop must systemctl --user stop: {steps:?}"
        );
        assert!(
            steps.iter().any(|step| run_args_eq(
                step,
                "systemctl",
                &["--user", "disable", "pawork.dev.service"]
            )),
            "Linux stop must systemctl --user disable: {steps:?}"
        );
        assert!(
            steps.iter().any(|step| matches!(
                step,
                TeardownStep::RemoveFile { path } if path == &unit
            )),
            "Linux stop must delete user unit file: {steps:?}"
        );
    }

    #[test]
    fn stop_teardown_plan_deletes_windows_scm_service() {
        let steps = windows_stop_steps("pawork");
        assert!(
            steps.iter().any(|step| matches!(
                step,
                TeardownStep::Run { program, args }
                    if program == "sc" && args == &["stop".to_string(), "pawork".to_string()]
            )),
            "Windows stop must sc stop: {steps:?}"
        );
        assert!(
            steps.iter().any(|step| matches!(
                step,
                TeardownStep::Run { program, args }
                    if program == "sc" && args == &["delete".to_string(), "pawork".to_string()]
            )),
            "Windows stop must sc delete: {steps:?}"
        );
    }

    #[test]
    fn apply_teardown_removes_unit_file() {
        let dir = std::env::temp_dir().join(format!(
            "pawork-svc-teardown-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create teardown dir");
        let path = dir.join("pawork.test.plist");
        std::fs::write(&path, "unit").expect("write unit");
        apply_teardown(&[TeardownStep::RemoveFile { path: path.clone() }])
            .expect("delete unit file");
        assert!(!path.exists(), "stop path must delete the unit file");
        let _ = std::fs::remove_dir_all(dir);
    }
}
