use std::path::PathBuf;
use std::sync::Arc;

use pawork_domain::{ToolCapability, WorkspaceId};
use pawork_exec::{
    OwnerSessionId, PtyCreateSpec, PtyEvent, PtySessionState, PtyWindowSize, TerminalId,
};
use pawork_policy::{ApprovalMode, PolicyDecision, PolicyEngine, PolicyInput};
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppEvent, AppResponse, TerminalExitReason,
    WorkspaceRelativePath,
};
use pawork_workspace::resolve_relative_path;
use serde_json::{json, Value};

use crate::gui_server::GuiHostError;

use super::super::GuiHostAdapter;

impl GuiHostAdapter {
    fn terminal_owner(&self, terminal_session_id: &str) -> Result<OwnerSessionId, GuiHostError> {
        self.terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(terminal_session_id)
            .cloned()
            .map(|registration| decode_terminal_registration(&registration).0.to_string())
            .map(OwnerSessionId::new)
            .ok_or_else(|| {
                Self::host_error(
                    "not_found",
                    format!("terminal {terminal_session_id} is not registered"),
                )
            })
    }

    fn remember_terminal(&self, terminal_id: &TerminalId, owner: &OwnerSessionId, cwd: &str) {
        self.terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                terminal_id.as_str().to_string(),
                encode_terminal_registration(owner, cwd),
            );
    }

    /// ADR-045 D1：close 后从注册表注销，快照 terminal_sessions 节不再出现该条目。
    fn forget_terminal(&self, terminal_session_id: &str) {
        self.terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(terminal_session_id);
    }

    fn spawn_terminal_forwarder(&self, terminal_id: TerminalId, owner: OwnerSessionId) {
        let Ok(mut receiver) = self.pty.subscribe(&terminal_id, &owner) else {
            return;
        };
        let bus = Arc::clone(&self.bus);
        let instance = self.instance.clone();
        let terminal_session_id = terminal_id.as_str().to_string();
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(PtyEvent::Output { data, .. }) => {
                        bus.publish_terminal(
                            instance.clone(),
                            &terminal_session_id,
                            AppEvent::TerminalOutput {
                                terminal_session_id: terminal_session_id.clone(),
                                delta: String::from_utf8_lossy(&data).into_owned(),
                            },
                        );
                    }
                    // ADR-045 D2：终态事件上 wire，同一终端只发一条（forwarder
                    // 是唯一广播点）。PtyEvent 携 waiter 已写入的权威终态，
                    // 即使 terminal_close 同时 cleanup 移除 service map 条目，
                    // 仍无竞态地区分 kill 与自然退出。
                    Ok(PtyEvent::Exit {
                        code,
                        signal,
                        state,
                    }) => {
                        let reason = if state == PtySessionState::Killed {
                            TerminalExitReason::Killed
                        } else {
                            TerminalExitReason::Exited
                        };
                        bus.publish_terminal(
                            instance.clone(),
                            &terminal_session_id,
                            AppEvent::TerminalExited {
                                terminal_session_id: terminal_session_id.clone(),
                                exit_code: code,
                                signal,
                                reason,
                            },
                        );
                        break;
                    }
                    // 转发链路异常断流（lagged / 广播关闭）：诚实 Failed，不臆造退出码。
                    Err(_) => {
                        bus.publish_terminal(
                            instance.clone(),
                            &terminal_session_id,
                            AppEvent::TerminalExited {
                                terminal_session_id: terminal_session_id.clone(),
                                exit_code: None,
                                signal: None,
                                reason: TerminalExitReason::Failed,
                            },
                        );
                        break;
                    }
                }
            }
        });
    }

    fn resolve_terminal_cwd(
        core: &crate::AppCore,
        workspace_id: &WorkspaceId,
        working_directory: Option<&WorkspaceRelativePath>,
    ) -> Result<(Option<PathBuf>, String), GuiHostError> {
        let workspace = core
            .workspace_by_id(workspace_id)
            .map_err(GuiHostAdapter::app_error)?;
        let roots = workspace.roots;
        match working_directory {
            None => Ok((roots.first().cloned(), ".".to_string())),
            Some(relative) => {
                if roots.is_empty() {
                    return Err(Self::host_error(
                        "not_found",
                        format!("workspace {} has no roots", workspace_id.as_str()),
                    ));
                }
                resolve_relative_path(&roots, relative.as_str())
                    .map(|resolved| {
                        (
                            Some(resolved.absolute),
                            terminal_cwd_label(resolved.relative),
                        )
                    })
                    .map_err(|error| Self::host_error("invalid_argument", error.to_string()))
            }
        }
    }

    pub(in crate::gui_host) fn terminal_snapshots(&self) -> Vec<Value> {
        let registered: Vec<(String, String)> = self
            .terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(id, owner)| (id.clone(), owner.clone()))
            .collect();
        registered
            .into_iter()
            .filter_map(|(id, registration)| {
                let (owner, cwd) = decode_terminal_registration(&registration);
                let terminal_id = TerminalId::new(id);
                let owner = OwnerSessionId::new(owner);
                let snapshot = self.pty.snapshot(&terminal_id, &owner).ok()?;
                let mut entry = json!({
                    "terminal_session_id": snapshot.terminal_id.as_str(),
                    "owner_session": snapshot.owner_session.as_str(),
                    "state": format!("{:?}", snapshot.state).to_ascii_lowercase(),
                    "columns": snapshot.size.cols,
                    "rows": snapshot.size.rows,
                    "dropped_events": snapshot.dropped_events,
                });
                // 快照段 data 是不透明 JSON Value（非 golden 锁定的帧 serde
                // 形状）；Desktop 需要创建时的 workspace 相对 cwd 做如实展示
                // 与 exited 后重建。记账缺失（不可能经本进程 create 产生）时
                // 省略键，Desktop 诚实显示 unknown 而非臆造工作区根。
                if let Some(cwd) = cwd {
                    entry["cwd"] = Value::String(cwd.to_string());
                }
                Some(entry)
            })
            .collect()
    }
}

/// `terminals` 注册表值编码：`owner\u{0}workspace 相对 cwd`。字段声明位于
/// gui_host/mod.rs，值类型保持 String 不动；编码/解码集中在这两个函数，
/// 读写注册表的三处（owner 解析 / create 记账 / 快照回填）同源使用。
fn encode_terminal_registration(owner: &OwnerSessionId, cwd: &str) -> String {
    format!("{}\u{0}{}", owner.as_str(), cwd)
}

fn decode_terminal_registration(registration: &str) -> (&str, Option<&str>) {
    match registration.split_once('\u{0}') {
        Some((owner, cwd)) => (owner, Some(cwd)),
        None => (registration, None),
    }
}

/// 记账/快照的 cwd 标签口径：策略层把根目录归一为空串（`resolve "."` 的
/// canonical 形），标签统一回落 `"."`（与 None 分支同口径），避免面板
/// cwd 空白（片 3 真窗口缺陷）。
fn terminal_cwd_label(relative: String) -> String {
    if relative.is_empty() {
        ".".to_string()
    } else {
        relative
    }
}

pub(crate) async fn terminal_create(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::TerminalCreate {
        workspace_id,
        working_directory,
    } = command
    else {
        unreachable!("terminal_create handler receives TerminalCreate")
    };
    let core = adapter.core.read().await;
    let approval_mode = core.approval.mode();
    let workspace_trusted = core.approval.workspace_trusted();
    let (cwd, cwd_label) =
        GuiHostAdapter::resolve_terminal_cwd(&core, workspace_id, working_directory.as_ref())?;
    drop(core);
    let owner = OwnerSessionId::new(workspace_id.as_str());
    let spec = PtyCreateSpec {
        owner_session: owner.clone(),
        cwd,
        size: PtyWindowSize::default(),
        ..PtyCreateSpec::default()
    };
    let gate = decide_terminal_create(
        approval_mode,
        workspace_trusted,
        &classification_shell(spec.shell.as_deref()),
        &spec.args,
    );
    let policy = match gate {
        TerminalCreateGate::Allow { policy } => policy,
        TerminalCreateGate::Deny { reason } => {
            return Err(GuiHostAdapter::host_error("forbidden", reason));
        }
    };
    let terminal_id = adapter
        .pty
        .create(spec)
        .await
        .map_err(GuiHostAdapter::pty_error)?;
    adapter.remember_terminal(&terminal_id, &owner, &cwd_label);
    adapter.spawn_terminal_forwarder(terminal_id.clone(), owner);
    Ok(AppResponse::Data(terminal_create_payload(
        &terminal_id,
        policy,
        approval_mode,
    )))
}

/// PTY 创建闸的裁决结果;Allow 携带进入响应负载的 policy 标签。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TerminalCreateGate {
    Allow { policy: &'static str },
    Deny { reason: String },
}

/// terminal_create 分类输入使用的如实 shell 程序。
///
/// `PtyCreateSpec::shell` 为 `None` 时由 `pawork-exec` 内部兜底
/// (Unix 取 `$SHELL` 否则 `/bin/sh`,Windows 用 `cmd.exe`,
/// 见 crates/exec/src/pty/mod.rs 的 `build_command`);分类必须取同一值。
fn classification_shell(shell: Option<&str>) -> String {
    match shell {
        Some(shell) => shell.to_string(),
        None => {
            #[cfg(windows)]
            {
                "cmd.exe".to_string()
            }
            #[cfg(not(windows))]
            {
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
            }
        }
    }
}

fn approval_mode_label(mode: ApprovalMode) -> String {
    serde_json::to_value(mode)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{mode:?}"))
}

/// ADR-041 D2:GUI terminal_create 的创建动作过 PolicyEngine(capability=Process)。
/// 纯函数便于单测;handler 只做装配。
///
/// - NeverAsk/ReadOnly 按 D2 fail-closed 拒绝创建(比引擎一般语义更紧);
/// - 未信任 workspace:交互 shell 不属于未信任能力,交给引擎 Deny;
/// - AskFor* 档产生 AskUser 时,GUI 命令通道不承载命令级交互审批,
///   按用户拍板(选项 A)fail-closed 落 Deny,reason 如实。
pub(crate) fn decide_terminal_create(
    mode: ApprovalMode,
    trusted: bool,
    shell: &str,
    args: &[String],
) -> TerminalCreateGate {
    let label = approval_mode_label(mode);
    if matches!(mode, ApprovalMode::NeverAsk | ApprovalMode::ReadOnly) {
        return TerminalCreateGate::Deny {
            reason: format!(
                "审批档 {label} 禁止创建终端:ADR-041 D2 决议该档拒绝创建交互 shell(fail-closed)"
            ),
        };
    }
    match PolicyEngine::new(mode).decide(&PolicyInput {
        capability: ToolCapability::Process,
        // 与 PolicyEngine::extract_command 的解析形状一致:program 走
        // `command` 键、参数走 `args` 键(`argv` 键要求含 program 的完整 argv)。
        input: json!({"command": shell, "args": args}),
        trusted,
        allowed_in_untrusted_workspace: false,
        approval_mode: mode,
    }) {
        PolicyDecision::Allow => TerminalCreateGate::Allow { policy: "allow" },
        PolicyDecision::AllowWithConstraints { .. } => TerminalCreateGate::Allow {
            policy: "allow_with_constraints",
        },
        PolicyDecision::Deny { reason } => TerminalCreateGate::Deny {
            reason: format!("terminal 创建被 policy 拒绝:{reason}"),
        },
        PolicyDecision::AskUser { .. } => TerminalCreateGate::Deny {
            reason: format!(
                "terminal 创建需交互审批,GUI 命令通道暂不承载命令级审批(已登记待 ADR),fail-closed 拒绝;审批档={label}"
            ),
        },
    }
}

/// terminal_create 成功响应负载;形状由 protocol golden
/// server_response_terminal_create.json 钉死。
fn terminal_create_payload(terminal_id: &TerminalId, policy: &str, mode: ApprovalMode) -> Value {
    let label = approval_mode_label(mode);
    json!({
        "terminal_session_id": terminal_id.as_str(),
        "sandboxed": false,
        "policy": policy,
        "approval_mode": label,
        "note": format!("创建已经 policy 闸({label} 档);PTY 会话内容不经沙箱与逐条审批"),
    })
}

pub(crate) async fn terminal_write(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::TerminalWrite {
        terminal_session_id,
        data,
    } = command
    else {
        unreachable!("terminal_write handler receives TerminalWrite")
    };
    let owner = adapter.terminal_owner(terminal_session_id)?;
    adapter
        .pty
        .write(
            &TerminalId::new(terminal_session_id),
            &owner,
            data.as_bytes().to_vec(),
        )
        .await
        .map_err(GuiHostAdapter::pty_error)?;
    Ok(AppResponse::Accepted {
        command_id: envelope.command_id.clone(),
        run_id: None,
    })
}

pub(crate) async fn terminal_resize(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::TerminalResize {
        terminal_session_id,
        columns,
        rows,
    } = command
    else {
        unreachable!("terminal_resize handler receives TerminalResize")
    };
    let owner = adapter.terminal_owner(terminal_session_id)?;
    adapter
        .pty
        .resize(
            &TerminalId::new(terminal_session_id),
            &owner,
            PtyWindowSize {
                rows: *rows,
                cols: *columns,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
        .await
        .map_err(GuiHostAdapter::pty_error)?;
    Ok(AppResponse::Accepted {
        command_id: envelope.command_id.clone(),
        run_id: None,
    })
}

/// ADR-045 D1：终止并注销终端会话。running 终端经 PtyService::cleanup
/// 终止进程组并移除 PTY service 条目（已自然退出条目只做清理），随后从
/// GuiHost 注册表注销；未知 id 报 not_found。
/// 终态事件由 forwarder 统一广播（reason=Killed 与自发 Exit 去重），本 handler
/// 不广播。
pub(crate) async fn terminal_close(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::TerminalClose {
        terminal_session_id,
    } = command
    else {
        unreachable!("terminal_close handler receives TerminalClose")
    };
    let owner = adapter.terminal_owner(terminal_session_id)?;
    adapter
        .pty
        .cleanup(&TerminalId::new(terminal_session_id), &owner)
        .await
        .map_err(GuiHostAdapter::pty_error)?;
    adapter.forget_terminal(terminal_session_id);
    Ok(AppResponse::Accepted {
        command_id: envelope.command_id.clone(),
        run_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 片 3 缺陷：`resolve "."` 经策略层归一为空串，cwd 标签必须回落
    /// `"."`（与未传 working_directory 同口径），子目录原样保留。
    #[test]
    fn terminal_cwd_label_maps_root_to_dot() {
        assert_eq!(terminal_cwd_label(String::new()), ".");
        assert_eq!(terminal_cwd_label("src".to_string()), "src");
    }

    fn deny_reason(gate: TerminalCreateGate) -> String {
        match gate {
            TerminalCreateGate::Deny { reason } => reason,
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[test]
    fn read_only_mode_denies_terminal_create() {
        let reason = deny_reason(decide_terminal_create(
            ApprovalMode::ReadOnly,
            true,
            "/bin/zsh",
            &[],
        ));
        assert!(reason.contains("read_only"), "{reason}");
        assert!(reason.contains("D2"), "{reason}");
    }

    #[test]
    fn never_ask_mode_denies_terminal_create() {
        let reason = deny_reason(decide_terminal_create(
            ApprovalMode::NeverAsk,
            true,
            "/bin/zsh",
            &[],
        ));
        assert!(reason.contains("never_ask"), "{reason}");
        assert!(reason.contains("fail-closed"), "{reason}");
    }

    #[test]
    fn ask_for_dangerous_allows_default_shell_with_constraints() {
        let shell = classification_shell(None);
        let gate = decide_terminal_create(ApprovalMode::AskForDangerous, true, &shell, &[]);
        match gate {
            TerminalCreateGate::Allow { policy } => {
                assert_eq!(policy, "allow_with_constraints");
            }
            other => panic!("expected Allow, got {other:?}"),
        }
    }

    #[test]
    fn ask_for_writes_fails_closed_on_ask_user() {
        let reason = deny_reason(decide_terminal_create(
            ApprovalMode::AskForWrites,
            true,
            "/bin/zsh",
            &[],
        ));
        assert!(reason.contains("ask_for_writes"), "{reason}");
        assert!(reason.contains("命令级审批"), "{reason}");
        assert!(reason.contains("fail-closed"), "{reason}");
    }

    #[test]
    fn untrusted_workspace_denies_terminal_create() {
        let reason = deny_reason(decide_terminal_create(
            ApprovalMode::AskForDangerous,
            false,
            "/bin/zsh",
            &[],
        ));
        assert!(reason.contains("untrusted"), "{reason}");
    }

    #[test]
    fn terminal_create_payload_reports_policy_gate_truthfully() {
        let terminal_id = TerminalId::new("terminal-1");
        let payload = terminal_create_payload(
            &terminal_id,
            "allow_with_constraints",
            ApprovalMode::AskForDangerous,
        );
        assert_eq!(
            payload,
            json!({
                "terminal_session_id": "terminal-1",
                "sandboxed": false,
                "policy": "allow_with_constraints",
                "approval_mode": "ask_for_dangerous",
                "note": "创建已经 policy 闸(ask_for_dangerous 档);PTY 会话内容不经沙箱与逐条审批",
            })
        );
    }
}
