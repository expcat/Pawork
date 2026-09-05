//! Controller 层：唯一业务出口是 pawork-client。
//!
//! 职责：连接握手 + 事件泵、SessionGet 分页、SessionCreate / SessionFork /
//! RunStart / RunCancel / ToolApprove / ModelList，以及 TerminalCreate /
//! TerminalWrite / TerminalResize。重连走 [`GuiClient::connect_with_resume`]，
//! 记录 last_acked `global_sequence`（来自事件与 Ack），按 Replay /
//! SnapshotRequired / UpToDate 三态交给 projection。

use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use pawork_client::{
    ActorIdentity, AppCommand, AppEvent, AppEventEnvelope, AppQuery, AppResponse,
    AppResponseEnvelope, ApprovalModeWire, AuthStartData, ClientAuthentication, ClientConfig,
    ClientError, CommandSource, ConnectOptions, DefaultModelPair, GeneralSettingsData,
    GlobalSequence, GuiCapability, GuiClient, GuiTransportClient, LocalTransport,
    PermissionsSettingsData, ProtocolErrorCode, ProviderAuthStatusData, ProviderUseProxyData,
    ResumeDisposition, ResumeOutcome, Snapshot,
    TOKEN_SCHEME, TerminalSettingsData, TimelinePage, TransportEndpoint,
};
use serde_json::json;

use crate::projection::{ModelEntry, sessions_in_snapshot};

pub(super) const PAGE_LIMIT: u32 = 500;
pub(super) const MAX_PAGES: usize = 200;

/// UI 消费的控制器事件（经 smol channel 跨线程投递）。
#[derive(Clone, Debug)]
pub enum ControllerEvent {
    Disconnected {
        reason: String,
    },
    Snapshot(Snapshot),
    TimelineLoaded {
        session_id: String,
        page: TimelinePage,
    },
    Event(AppEventEnvelope),
    SessionCreated {
        session_id: String,
    },
    WorkspaceOpened {
        workspace_id: String,
        name: String,
    },
    /// 发送回执：text 随行携带，供 UI 在 wire 用户消息事件缺席时乐观回显。
    MessageSent {
        session_id: String,
        run_id: String,
        text: String,
    },
    ModelsLoaded(Vec<ModelEntry>),
    /// provider_auth_status 查询成功（SET-3 只读供应商页；SET-5 起随载荷
    /// 携带 Host 权威默认模型）。
    ProviderStatusLoaded(ProviderAuthStatusData),
    /// set_default_model 获 Host Data 确认（SET-5；echo 携带已确认 pair，
    /// Composer 据此同步）。随后 controller 重查 provider_auth_status 取回
    /// 权威 default。
    DefaultModelConfirmed(DefaultModelPair),
    /// general_settings 查询成功（SET-6a Network 页；Host 权威 proxy_url）。
    GeneralSettingsLoaded(GeneralSettingsData),
    /// set_proxy_url 获 Host Data 确认（SET-6a；回执即写后状态）。
    ProxyUrlConfirmed(GeneralSettingsData),
    /// set_provider_use_proxy 获 Host Data 确认（ADR-052 SET-6h；回执即
    /// 写后状态，直接落 projection，不重查）。
    ProviderUseProxyConfirmed {
        provider_id: String,
        use_proxy: bool,
    },
    /// permissions_settings 查询成功（SET-6b 权限与审批页；Host 权威
    /// 三元组：当前 mode / 会话 trusted / Global 持久默认）。
    PermissionsSettingsLoaded(PermissionsSettingsData),
    /// set_approval_mode 获 Host Data 确认（SET-6b / ADR-048 D2；回执即
    /// 写后状态）。
    ApprovalModeConfirmed {
        mode: ApprovalModeWire,
    },
    /// workspace_trust 获 Host Data 确认（SET-6b / ADR-048 D3；回执即
    /// 写后状态）。
    WorkspaceTrustConfirmed {
        trusted: bool,
    },
    /// terminal_settings 查询成功（SET-6d 终端页；Host 权威生效值）。
    TerminalSettingsLoaded(TerminalSettingsData),
    /// set_terminal_settings 获 Host Data 确认（SET-6d / ADR-050 D3；回执
    /// 即写后完整状态）。
    TerminalSettingsConfirmed(TerminalSettingsData),
    /// auth_start 响应（SET-4）：OAuth 授权等待信息；进度经 AuthChanged
    /// 事件流下发，token 不经过 Desktop。
    AuthStarted {
        provider_id: String,
        data: AuthStartData,
    },
    SessionForked {
        session_id: String,
    },
    TerminalCreated {
        workspace_id: String,
        terminal_session_id: String,
    },
    TerminalCreateFailed {
        workspace_id: String,
        reason: String,
    },
    TerminalWriteSucceeded {
        terminal_session_id: String,
    },
    TerminalWriteFailed {
        terminal_session_id: String,
        reason: String,
    },
    TerminalResizeSucceeded {
        terminal_session_id: String,
        columns: u16,
        rows: u16,
    },
    TerminalResizeFailed {
        terminal_session_id: String,
        reason: String,
    },
    /// terminal_close 已被 Host 接受（ADR-045）。running 的终态由 live
    /// TerminalExited 事件刷新；exited 清理由 UI 在回执后本地移除条目。
    TerminalCloseSucceeded {
        terminal_session_id: String,
    },
    TerminalCloseFailed {
        terminal_session_id: String,
        reason: String,
    },
    /// diff_list_files 成功（epoch 为 UI 侧请求代次，防过期响应覆盖新状态）。
    DiffFilesLoaded {
        epoch: u64,
        session_id: Option<String>,
        files: Vec<DiffFileSummary>,
        git: Option<GitDiffInfo>,
    },
    /// diff_get 成功；file 为 None 表示该路径已不在 diff 中（host 空响应）。
    DiffContentLoaded {
        epoch: u64,
        path: String,
        session_id: Option<String>,
        file: Option<DiffFileDetail>,
    },
    /// mcp_list 成功（响应形状 {"servers":[{name,transport,state,tools,last_error}]}）。
    McpServersLoaded {
        epoch: u64,
        servers: Vec<McpServerEntry>,
    },
    DiffFilesFailed {
        epoch: u64,
        reason: String,
    },
    DiffContentFailed {
        epoch: u64,
        path: String,
        reason: String,
    },
    McpServersFailed {
        epoch: u64,
        reason: String,
    },
    /// mcp_test / mcp_server_remove 的 Data 回执（SET-6c / ADR-049）：形状
    /// 与 mcp_list 相同，无 epoch —— 回执即 Host 权威写后状态，UI 直接
    /// 落地 ResourcesPanelState。
    McpServersReceipt {
        servers: Vec<McpServerEntry>,
    },
    /// open_session 的会话级失败：携带 session_id，UI 仅在该会话仍为
    /// active 时复位分页状态（A→B 快切时 A 的迟到失败不影响 B）。
    SessionOpenFailed {
        session_id: String,
        reason: String,
    },
    OperationFailed {
        action: &'static str,
        reason: String,
    },
}

/// Changes 面 Files 行（diff_list_files 响应的视图模型）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFileSummary {
    pub path: String,
    /// host 序列化的 snake_case 状态（added / modified / …）；缺失记 unknown。
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
}

/// diff_list_files 携带的 git 信息；字段缺失保持 None，UI 显示 unknown。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitDiffInfo {
    pub branch: Option<String>,
    pub work_dir: Option<String>,
    pub dirty_files: Option<u64>,
}

/// diff 行类型（host LineKind 的 snake_case wire 名）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLineDetail {
    pub kind: DiffLineKind,
    /// 行文本（不含 +/-/空格 前缀）。
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunkDetail {
    /// hunk 头原文（如 `@@ -1,3 +1,4 @@`）。
    pub header: String,
    pub lines: Vec<DiffLineDetail>,
}

/// diff_get 响应的单文件视图模型（仅保留渲染所需字段）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFileDetail {
    pub path: String,
    /// rename / copy 时的原始路径。
    pub previous_path: Option<String>,
    pub status: String,
    pub binary: bool,
    pub additions: u64,
    pub deletions: u64,
    pub hunks: Vec<DiffHunkDetail>,
}

/// Resources 页 MCP server 行；tools 在 wire 上是名称数组，这里只留数量。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerEntry {
    pub name: String,
    pub transport: String,
    pub state: String,
    pub tool_count: u64,
    pub last_error: Option<String>,
}

/// Desktop 使用的非 Secret 握手摘要；runtime ID 不等同 CLI `--instance`
/// 配置名，capabilities 使用冻结 wire 的 snake_case 名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopHandshakeInfo {
    pub runtime_id: String,
    pub api_version: String,
    pub capabilities: Vec<String>,
    /// 当前已认证 Host 声明的实际数据目录；缺失时 About fail-closed 隐藏。
    pub host_data_dir: Option<String>,
}

/// 握手 / 重连结果：`resume` 为 None 表示首连（无 last_ack）。
pub struct DesktopConnect {
    pub snapshot: Snapshot,
    pub resume: Option<ResumeOutcome>,
    pub handshake: DesktopHandshakeInfo,
    pub events: smol::channel::Receiver<ControllerEvent>,
}

struct SharedState {
    client: Mutex<Option<GuiClient>>,
    events: Mutex<Option<smol::channel::Sender<ControllerEvent>>>,
    last_acked: Mutex<Option<u64>>,
    /// 连接代次：每次成功连接递增。旧泵 / 旧心跳的迟到失败不得拆掉
    /// 新连接（清 client 槽或投递 Disconnected）。
    generation: AtomicU64,
}

pub struct DesktopController {
    runtime: tokio::runtime::Handle,
    state: Arc<SharedState>,
}

mod session;
mod settings;
mod terminal;

impl DesktopController {
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self {
            runtime,
            state: Arc::new(SharedState {
                client: Mutex::new(None),
                events: Mutex::new(None),
                last_acked: Mutex::new(None),
                generation: AtomicU64::new(0),
            }),
        }
    }

    pub(super) fn current_client(&self) -> Option<GuiClient> {
        self.state
            .client
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// 连接 + 握手 + 订阅。有 last_ack 时走 `connect_with_resume`，不要永远
    /// 全新 Snapshot。
    pub async fn connect(&self, socket: PathBuf) -> Result<DesktopConnect, String> {
        let token_path = crate::platform::token_path_for_socket(&socket);
        let authentication = load_desktop_authentication(&token_path)?;
        let (sender, receiver) = smol::channel::bounded::<ControllerEvent>(512);
        let transport: Arc<dyn GuiTransportClient> = Arc::new(LocalTransport::default());
        let endpoint = TransportEndpoint::Local {
            address: socket.to_string_lossy().into_owned(),
        };
        let options = ConnectOptions {
            timeout_ms: 10_000,
            client_label: Some("pawork-desktop".into()),
            max_frame_bytes: 1024 * 1024,
        };
        let last_ack = self.last_acked_sequence().map(GlobalSequence);
        let has_last_ack = last_ack.is_some();
        // 连接期全部 client 调用（握手 / ack / subscribe_all）都必须在 tokio
        // runtime 上执行：cx.spawn 的 gpui 前台执行器没有 reactor，
        // receive_frame 内的 tokio::time 会在真窗口启动路径直接 panic。
        let state = Arc::clone(&self.state);
        let connected = self
            .runtime
            .spawn(async move {
                let (handshake, resume) = GuiClient::connect_with_resume_config(
                    transport,
                    endpoint,
                    options,
                    Some(authentication),
                    last_ack,
                    desktop_client_config(),
                )
                .await
                .map_err(|error| error.to_string())?;
                let mut snapshot = handshake
                    .initial_snapshot()
                    .ok_or_else(|| "handshake did not deliver an initial snapshot".to_string())?;
                if !has_last_ack {
                    record_shared_last_acked(&state, snapshot.snapshot_sequence.0);
                    let _ = handshake.ack(snapshot.snapshot_sequence).await;
                }
                if let Some(outcome) = &resume {
                    match &outcome.disposition {
                        ResumeDisposition::Replay {
                            through_sequence, ..
                        } => {
                            record_shared_last_acked(&state, through_sequence.0);
                            let _ = handshake.ack(*through_sequence).await;
                        }
                        ResumeDisposition::UpToDate { current_sequence } => {
                            record_shared_last_acked(&state, current_sequence.0);
                        }
                        ResumeDisposition::SnapshotRequired { .. } => {
                            if let Some(fresh) = &outcome.snapshot {
                                snapshot = fresh.clone();
                            }
                            record_shared_last_acked(&state, snapshot.snapshot_sequence.0);
                            let _ = handshake.ack(snapshot.snapshot_sequence).await;
                        }
                    }
                }
                handshake
                    .subscribe_all()
                    .await
                    .map_err(|error| error.to_string())?;
                Ok::<_, String>((handshake, resume, snapshot))
            })
            .await
            .map_err(|error| format!("connect task failed: {error}"))??;
        let (handshake, resume, snapshot) = connected;

        // 代次递增必须先于 client 槽安装：teardown 在 client 锁内对照
        // generation，保证旧连接的迟到失败清不掉新连接。
        let generation = self
            .state
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        *self.state.client.lock().unwrap_or_else(|p| p.into_inner()) = Some(handshake.clone());
        *self.state.events.lock().unwrap_or_else(|p| p.into_inner()) = Some(sender.clone());

        let pump_client = handshake.clone();
        let pump_events = sender;
        let pump_state = Arc::clone(&self.state);
        let heartbeat_client = handshake.clone();
        let heartbeat_events = pump_events.clone();
        let heartbeat_state = Arc::clone(&self.state);
        self.runtime.spawn(async move {
            // 保活由独立的 heartbeat 任务承担：泵可能阻塞在向 UI channel
            // 的 send().await 上（channel 满时不能丢事件），不能因此停跳。
            loop {
                match pump_client.next_event_timeout(Duration::from_secs(1)).await {
                    Ok(event) => {
                        record_shared_last_acked(&pump_state, event.global_sequence.0);
                        let _ = pump_client.ack(event.global_sequence).await;
                        // ADR-054 D5：SessionMetaChanged（改名 / 归档 / 自动
                        // 标题写回）意味着快照里的 session_tree 已过时；重取
                        // snapshot 让列表回到 Host 写后状态。泵任务已在
                        // runtime 上，直接 tokio::spawn 不占用 gpui 执行器。
                        let meta_changed = matches!(
                            event.payload,
                            AppEvent::SessionMetaChanged { .. }
                        );
                        if pump_events
                            .send(ControllerEvent::Event(event))
                            .await
                            .is_err()
                        {
                            break;
                        }
                        if meta_changed {
                            let client = pump_client.clone();
                            let events = pump_events.clone();
                            tokio::spawn(async move {
                                match client.snapshot().await {
                                    Ok(snapshot) => {
                                        let _ = events.send(ControllerEvent::Snapshot(snapshot)).await;
                                    }
                                    Err(error) => {
                                        let _ = events
                                            .send(ControllerEvent::OperationFailed {
                                                action: "refresh sessions",
                                                reason: error.to_string(),
                                            })
                                            .await;
                                    }
                                }
                            });
                        }
                    }
                    // 空闲 tick：继续等事件即可。
                    Err(ClientError::Timeout { .. }) => continue,
                    Err(error) => {
                        teardown_stale_connection(
                            &pump_state,
                            &pump_events,
                            generation,
                            error.to_string(),
                        )
                        .await;
                        break;
                    }
                }
            }
        });
        // 独立心跳：host heartbeat_timeout 30s。泵被 UI 排水阻塞时心跳
        // 也不能停；而 select! 抢占 next_event_timeout 会破坏分帧读的
        // 取消安全性（半帧后流错位），故用独立任务按 interval 保活——
        // client io 为 AsyncMutex，泵内并发调用本就是支持路径。
        self.runtime.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(15));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // interval 首 tick 立即完成，消费掉以进入 15s 节奏。
            interval.tick().await;
            loop {
                interval.tick().await;
                if heartbeat_state.generation.load(Ordering::Acquire) != generation {
                    break;
                }
                if let Err(error) = heartbeat_client.heartbeat().await {
                    teardown_stale_connection(
                        &heartbeat_state,
                        &heartbeat_events,
                        generation,
                        error.to_string(),
                    )
                    .await;
                    break;
                }
            }
        });
        let handshake_info = desktop_handshake_info(&handshake);
        Ok(DesktopConnect {
            snapshot,
            resume,
            handshake: handshake_info,
            events: receiver,
        })
    }

    pub fn last_acked_sequence(&self) -> Option<u64> {
        *self
            .state
            .last_acked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    /// 拉取 Changes 面文件清单（diff_list_files）。epoch 由 UI 递增，
    /// 响应原样带回，过期代次在 UI 侧丢弃。
    pub fn diff_list_files(&self, workspace_id: String, epoch: u64) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::DiffFilesFailed {
                epoch,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let query = diff_list_files_query(&workspace_id);
            match client
                .query(query, command_source(), actor_identity())
                .await
            {
                Ok(response) => match parse_diff_files(&response) {
                    Ok((session_id, files, git)) => {
                        let _ = events
                            .send(ControllerEvent::DiffFilesLoaded {
                                epoch,
                                session_id,
                                files,
                                git,
                            })
                            .await;
                    }
                    Err(reason) => {
                        let _ = events
                            .send(ControllerEvent::DiffFilesFailed { epoch, reason })
                            .await;
                    }
                },
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::DiffFilesFailed {
                            epoch,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    /// 拉取单文件 diff（diff_get）。host 对不存在路径返回空 files，
    /// 解析为 None。
    pub fn diff_get(&self, workspace_id: String, path: String, epoch: u64) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::DiffContentFailed {
                epoch,
                path,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let query = diff_get_query(&workspace_id, &path);
            match client
                .query(query, command_source(), actor_identity())
                .await
            {
                Ok(response) => match parse_diff_file(&response) {
                    Ok((session_id, file)) => {
                        let _ = events
                            .send(ControllerEvent::DiffContentLoaded {
                                epoch,
                                path,
                                session_id,
                                file,
                            })
                            .await;
                    }
                    Err(reason) => {
                        let _ = events
                            .send(ControllerEvent::DiffContentFailed {
                                epoch,
                                path,
                                reason,
                            })
                            .await;
                    }
                },
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::DiffContentFailed {
                            epoch,
                            path,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    /// 拉取 Resources 页 MCP server 清单（mcp_list）。
    pub fn mcp_list(&self, epoch: u64) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::McpServersFailed {
                epoch,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let query = mcp_list_query();
            match client
                .query(query, command_source(), actor_identity())
                .await
            {
                Ok(response) => match parse_mcp_servers(&response) {
                    Ok(servers) => {
                        let _ = events
                            .send(ControllerEvent::McpServersLoaded { epoch, servers })
                            .await;
                    }
                    Err(reason) => {
                        let _ = events
                            .send(ControllerEvent::McpServersFailed { epoch, reason })
                            .await;
                    }
                },
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::McpServersFailed {
                            epoch,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    /// 现场验证单个 MCP server（mcp_test，SET-6c / ADR-049 D1）。Data 回执
    /// 与 mcp_list 同形状（验证后的权威清单），经 McpServersReceipt 投递；
    /// Error / 传输失败经 OperationFailed 呈现，不动 UI 现有清单。
    pub fn mcp_test(&self, name: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "test mcp server".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = mcp_test_command(&name);
            let response = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::OperationFailed {
                            action: "test mcp server",
                            reason: error.to_string(),
                        })
                        .await;
                    return;
                }
            };
            match parse_mcp_receipt(&response) {
                Ok(servers) => {
                    let _ = events
                        .send(ControllerEvent::McpServersReceipt { servers })
                        .await;
                }
                Err(reason) => {
                    let _ = events
                        .send(ControllerEvent::OperationFailed {
                            action: "test mcp server",
                            reason,
                        })
                        .await;
                }
            }
        });
    }

    /// 从 Global 配置移除单个 MCP server（mcp_server_remove，SET-6c /
    /// ADR-049 D2）。Data 回执与 mcp_list 同形状（移除后的权威清单）；
    /// 失败 fail-closed 不动 UI 现有清单。
    pub fn mcp_server_remove(&self, name: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "remove mcp server".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = mcp_server_remove_command(&name);
            let response = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::OperationFailed {
                            action: "remove mcp server",
                            reason: error.to_string(),
                        })
                        .await;
                    return;
                }
            };
            match parse_mcp_receipt(&response) {
                Ok(servers) => {
                    let _ = events
                        .send(ControllerEvent::McpServersReceipt { servers })
                        .await;
                }
                Err(reason) => {
                    let _ = events
                        .send(ControllerEvent::OperationFailed {
                            action: "remove mcp server",
                            reason,
                        })
                        .await;
                }
            }
        });
    }

    pub(super) fn event_sender(&self) -> smol::channel::Sender<ControllerEvent> {
        self.state
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .expect("event channel exists after connect")
    }

    pub(super) fn try_event_sender(&self) -> Option<smol::channel::Sender<ControllerEvent>> {
        self.state
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// 关键生命周期/回执事件不得被 512 槽 event channel 的瞬时峰值吞掉。
    /// 同步 API 里没有 await 点，因此把可靠投递交给 runtime；只有 UI 已
    /// 销毁（receiver 关闭）时才允许失败。
    pub(super) fn emit_reliable(&self, event: ControllerEvent) {
        if let Some(events) = self.try_event_sender() {
            self.runtime.spawn(async move {
                let _ = events.send(event).await;
            });
        }
    }
}

pub(super) fn try_emit(events: &smol::channel::Sender<ControllerEvent>, event: ControllerEvent) {
    let _ = events.try_send(event);
}

/// 连接级失败收尾：在 client 锁内对照 generation，仅当没有更新的连接
/// 接管时才清 client 槽并投递 Disconnected，避免旧泵 / 旧心跳迟到的
/// 失败拆掉重连后的新连接。Disconnected 本身走可靠 send().await。
async fn teardown_stale_connection(
    state: &SharedState,
    events: &smol::channel::Sender<ControllerEvent>,
    generation: u64,
    reason: String,
) {
    {
        let mut client = state.client.lock().unwrap_or_else(|p| p.into_inner());
        if state.generation.load(Ordering::Acquire) != generation {
            return;
        }
        // 泵与心跳可能同时失败：只允许第一个清空者投递 Disconnected，
        // 避免同一代次连发两次把重连后的 UI 再打回断线。
        if client.is_none() {
            return;
        }
        *client = None;
    }
    if state.generation.load(Ordering::Acquire) != generation {
        return;
    }
    let _ = events.send(ControllerEvent::Disconnected { reason }).await;
}

pub(super) fn record_shared_last_acked(state: &SharedState, sequence: u64) {
    let mut slot = state.last_acked.lock().unwrap_or_else(|p| p.into_inner());
    *slot = Some(advance_last_acked(*slot, sequence));
}

pub(super) fn advance_last_acked(current: Option<u64>, incoming: u64) -> u64 {
    current.map_or(incoming, |prev| prev.max(incoming))
}

pub(super) fn desktop_client_config() -> ClientConfig {
    let mut config = ClientConfig::default();
    config.client_name = "pawork-desktop".into();
    config.capabilities = desktop_capabilities();
    config
}

pub(super) fn desktop_capabilities() -> Vec<GuiCapability> {
    vec![
        GuiCapability::Events,
        GuiCapability::Snapshots,
        GuiCapability::Approvals,
        GuiCapability::TerminalStreaming,
    ]
}

pub(super) fn desktop_handshake_info(client: &GuiClient) -> DesktopHandshakeInfo {
    let version = client.api_version();
    DesktopHandshakeInfo {
        runtime_id: client.handle().instance_id.as_str().to_string(),
        api_version: format!("{}.{}", version.major, version.minor),
        capabilities: client
            .capabilities()
            .iter()
            .map(|capability| match capability {
                GuiCapability::Events => "events",
                GuiCapability::Snapshots => "snapshots",
                GuiCapability::ArtifactStreaming => "artifact_streaming",
                GuiCapability::TerminalStreaming => "terminal_streaming",
                GuiCapability::Approvals => "approvals",
            })
            .map(str::to_string)
            .collect(),
        host_data_dir: client.info().host_data_dir.clone(),
    }
}

/// source / identity 占位：服务端 host_stamp_command / host_stamp_query 会统一
/// 覆盖为 LocalGui + LocalUser（host/gui-server/src/session.rs），
/// 客户端只填必填信封字段，不伪造本地身份。
pub(super) fn command_source() -> CommandSource {
    CommandSource::Automation
}

pub(super) fn actor_identity() -> ActorIdentity {
    ActorIdentity::System
}

/// WorkspaceId / SessionId 等 domain id 未从 pawork-client re-export；命令与
/// 查询经冻结的 serde 形状（method/params）构造，避免引入第二个业务依赖。
/// ADR-054 D1（since 1.11）：workspace_id = None 时缺省该字段 → Host 落盘
/// 无归属会话；显式传值行为不变。
pub(super) fn session_create_command(workspace_id: Option<&str>) -> AppCommand {
    let params = match workspace_id {
        Some(workspace_id) => json!({ "workspace_id": workspace_id }),
        None => json!({}),
    };
    serde_json::from_value(json!({
        "method": "session_create",
        "params": params
    }))
    .expect("session_create command shape is frozen")
}

/// ADR-054 D2：会话改名。title trim 后为空由 Host 结构化拒绝，不写盘。
pub(super) fn session_rename_command(session_id: &str, title: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "session_rename",
        "params": { "session_id": session_id, "title": title }
    }))
    .expect("session_rename command shape is frozen")
}

/// ADR-054 D3：归档 / 反归档。Desktop 只暴露归档入口（archived = true）。
pub(super) fn session_archive_command(session_id: &str, archived: bool) -> AppCommand {
    serde_json::from_value(json!({
        "method": "session_archive",
        "params": { "session_id": session_id, "archived": archived }
    }))
    .expect("session_archive command shape is frozen")
}

pub(super) fn workspace_add_command(root_path: &std::path::Path) -> AppCommand {
    serde_json::from_value(json!({
        "method": "workspace_add",
        "params": { "root_path": root_path.to_string_lossy() }
    }))
    .expect("workspace_add command shape is frozen")
}

pub(super) fn session_fork_command(session_id: &str, parent_event_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "session_fork",
        "params": {
            "session_id": session_id,
            "parent_event_id": parent_event_id
        }
    }))
    .expect("session_fork command shape is frozen")
}

pub(super) fn is_workspace_relative_cwd(cwd: &str) -> bool {
    let trimmed = cwd.trim();
    if trimmed.is_empty() {
        return false;
    }
    let bytes = trimmed.as_bytes();
    let has_windows_prefix = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    !(trimmed.starts_with(['/', '\\'])
        || has_windows_prefix
        || trimmed
            .split(['/', '\\'])
            .any(|component| component == ".."))
}

pub(super) fn terminal_create_command(
    workspace_id: &str,
    cwd: Option<&str>,
) -> Result<AppCommand, String> {
    let mut params = json!({ "workspace_id": workspace_id });
    if let Some(cwd) = cwd.map(str::trim).filter(|cwd| !cwd.is_empty()) {
        if !is_workspace_relative_cwd(cwd) {
            return Err("cwd must be a workspace-relative path".into());
        }
        params["working_directory"] = json!(cwd);
    }
    serde_json::from_value(json!({
        "method": "terminal_create",
        "params": params
    }))
    .map_err(|error| format!("terminal_create command shape: {error}"))
}

pub(super) fn terminal_write_command(terminal_session_id: &str, data: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "terminal_write",
        "params": {
            "terminal_session_id": terminal_session_id,
            "data": data
        }
    }))
    .expect("terminal_write command shape is frozen")
}

pub(super) fn terminal_resize_command(
    terminal_session_id: &str,
    columns: u16,
    rows: u16,
) -> AppCommand {
    serde_json::from_value(json!({
        "method": "terminal_resize",
        "params": {
            "terminal_session_id": terminal_session_id,
            "columns": columns,
            "rows": rows
        }
    }))
    .expect("terminal_resize command shape is frozen")
}

pub(super) fn terminal_close_command(terminal_session_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "terminal_close",
        "params": {
            "terminal_session_id": terminal_session_id
        }
    }))
    .expect("terminal_close command shape is frozen")
}

pub(super) fn auth_start_command(provider_id: &str, flow: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "auth_start",
        "params": { "provider_id": provider_id, "flow": flow }
    }))
    .expect("auth_start command shape is frozen")
}

/// ApiKeySecret 在 wire 上是透明字符串；明文只在本函数栈上的 Value 里
/// 短暂停留，from_value 后即弃，不落任何字段或日志。
pub(super) fn auth_set_api_key_command(provider_id: &str, api_key: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "auth_set_api_key",
        "params": { "provider_id": provider_id, "api_key": api_key }
    }))
    .expect("auth_set_api_key command shape is frozen")
}

pub(super) fn auth_cancel_command(provider_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "auth_cancel",
        "params": { "provider_id": provider_id }
    }))
    .expect("auth_cancel command shape is frozen")
}

pub(super) fn auth_remove_command(provider_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "auth_remove",
        "params": { "provider_id": provider_id }
    }))
    .expect("auth_remove command shape is frozen")
}

pub(super) fn set_default_model_command(provider_id: &str, model_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_default_model",
        "params": { "provider_id": provider_id, "model_id": model_id }
    }))
    .expect("set_default_model command shape is frozen")
}

pub(super) fn forked_session_id(response: &AppResponseEnvelope) -> Option<String> {
    match &response.response {
        AppResponse::Data(data) => data
            .get("session_id")
            .or_else(|| data.get("branch_id"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        _ => None,
    }
}

pub(super) fn workspace_opened(response: &AppResponseEnvelope) -> Option<(String, String)> {
    match &response.response {
        AppResponse::Data(data) => Some((
            data.get("id")?.as_str()?.to_string(),
            data.get("name")?.as_str()?.to_string(),
        )),
        _ => None,
    }
}

pub(super) fn terminal_session_id(response: &AppResponseEnvelope) -> Option<String> {
    match &response.response {
        AppResponse::Data(data) => data
            .get("terminal_session_id")
            .or_else(|| data.get("id"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        _ => None,
    }
}

pub(super) fn session_get_query(session_id: &str, after: Option<u64>) -> AppQuery {
    serde_json::from_value(json!({
        "method": "session_get",
        "params": {
            "session_id": session_id,
            "timeline_after_sequence": after,
            "timeline_limit": PAGE_LIMIT
        }
    }))
    .expect("session_get query shape is frozen")
}

pub(super) fn load_desktop_authentication(
    token_path: &std::path::Path,
) -> Result<ClientAuthentication, String> {
    let bytes = std::fs::read(token_path).map_err(|error| {
        format!(
            "gui token file not found or unreadable ({}): {error}",
            token_path.display()
        )
    })?;
    let text = String::from_utf8(bytes).map_err(|_| {
        format!(
            "gui token file is empty or malformed: {}",
            token_path.display()
        )
    })?;
    let proof = text.trim();
    if proof.is_empty() {
        return Err(format!(
            "gui token file is empty or malformed: {}",
            token_path.display()
        ));
    }
    Ok(ClientAuthentication {
        scheme: TOKEN_SCHEME.into(),
        proof: proof.to_string(),
    })
}

pub(super) fn run_start_command(
    session_id: &str,
    text: &str,
    model: Option<&(String, String)>,
) -> AppCommand {
    let mut params = json!({
        "session_id": session_id,
        "user_message": text
    });
    if let Some((provider, id)) = model {
        params["provider"] = json!(provider);
        params["model"] = json!(id);
    }
    serde_json::from_value(json!({
        "method": "run_start",
        "params": params
    }))
    .expect("run_start command shape is frozen")
}

pub(super) fn run_cancel_command(run_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "run_cancel",
        "params": { "run_id": run_id }
    }))
    .expect("run_cancel command shape is frozen")
}

pub(super) fn tool_approve_command(run_id: &str, tool_call_id: &str, decision: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "tool_approve",
        "params": {
            "run_id": run_id,
            "tool_call_id": tool_call_id,
            "decision": decision
        }
    }))
    .expect("tool_approve command shape is frozen")
}

pub(super) fn model_list_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "model_list",
        "params": {}
    }))
    .expect("model_list query shape is frozen")
}

pub(super) fn provider_auth_status_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "provider_auth_status",
        "params": {}
    }))
    .expect("provider_auth_status query shape is frozen")
}

pub(super) fn general_settings_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "general_settings"
    }))
    .expect("general_settings query shape is frozen")
}

pub(super) fn permissions_settings_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "permissions_settings"
    }))
    .expect("permissions_settings query shape is frozen")
}

pub(super) fn terminal_settings_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "terminal_settings"
    }))
    .expect("terminal_settings query shape is frozen")
}

pub(super) fn set_proxy_url_command(proxy_url: Option<&str>) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_proxy_url",
        "params": { "proxy_url": proxy_url }
    }))
    .expect("set_proxy_url command shape is frozen")
}

pub(super) fn set_provider_use_proxy_command(provider_id: &str, use_proxy: bool) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_provider_use_proxy",
        "params": { "provider_id": provider_id, "use_proxy": use_proxy }
    }))
    .expect("set_provider_use_proxy command shape is frozen")
}

pub(super) fn set_approval_mode_command(mode: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_approval_mode",
        "params": { "mode": mode }
    }))
    .expect("set_approval_mode command shape is frozen")
}

pub(super) fn set_workspace_trust_command(workspace_id: &str, trusted: bool) -> AppCommand {
    serde_json::from_value(json!({
        "method": "workspace_trust",
        "params": { "workspace_id": workspace_id, "trusted": trusted }
    }))
    .expect("workspace_trust command shape is frozen")
}

pub(super) fn set_terminal_settings_command(
    shell: Option<&str>,
    columns: u16,
    rows: u16,
) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_terminal_settings",
        "params": { "shell": shell, "columns": columns, "rows": rows }
    }))
    .expect("set_terminal_settings command shape is frozen")
}

pub(super) fn diff_list_files_query(workspace_id: &str) -> AppQuery {
    serde_json::from_value(json!({
        "method": "diff_list_files",
        "params": { "workspace_id": workspace_id }
    }))
    .expect("diff_list_files query shape is frozen")
}

pub(super) fn diff_get_query(workspace_id: &str, path: &str) -> AppQuery {
    serde_json::from_value(json!({
        "method": "diff_get",
        "params": {
            "workspace_id": workspace_id,
            "path": path
        }
    }))
    .expect("diff_get query shape is frozen")
}

pub(super) fn mcp_list_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "mcp_list"
    }))
    .expect("mcp_list query shape is frozen")
}

pub(super) fn mcp_test_command(name: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "mcp_test",
        "params": { "name": name }
    }))
    .expect("mcp_test command shape is frozen")
}

pub(super) fn mcp_server_remove_command(name: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "mcp_server_remove",
        "params": { "name": name }
    }))
    .expect("mcp_server_remove command shape is frozen")
}

pub(super) fn parse_models(response: &AppResponseEnvelope) -> Result<Vec<ModelEntry>, String> {
    match &response.response {
        AppResponse::Data(data) => {
            let entries = data
                .as_array()
                .ok_or_else(|| "model list is not an array".to_string())?;
            Ok(entries
                .iter()
                .filter_map(|entry| {
                    let provider_id = entry.get("provider_id").and_then(|value| value.as_str())?;
                    let id = entry.get("id").and_then(|value| value.as_str())?;
                    let display_name = entry
                        .get("display_name")
                        .and_then(|value| value.as_str())
                        .unwrap_or(id);
                    Some(ModelEntry {
                        provider_id: provider_id.to_string(),
                        id: id.to_string(),
                        display_name: display_name.to_string(),
                        context_window_tokens: entry
                            .get("context_window_tokens")
                            .and_then(serde_json::Value::as_u64),
                    })
                })
                .collect())
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 provider_auth_status 信封：`AppResponse::Data` 载荷形如
/// `{"providers":[…]}`，条目解析在 projection（纯状态可单测）。
pub(super) fn parse_provider_status_response(
    response: &AppResponseEnvelope,
) -> Result<ProviderAuthStatusData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 general_settings / set_proxy_url 信封：Data 为
/// `{ "proxy_url": string | null }`；Error 取 Host 脱敏 message 原文
///（不含 proxy URL）。
pub(super) fn parse_general_settings_response(
    response: &AppResponseEnvelope,
) -> Result<GeneralSettingsData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 set_provider_use_proxy 信封：Data 为
/// `{ "provider_id": string, "use_proxy": bool }`；Error 取 Host 脱敏
/// message 原文。
pub(super) fn parse_provider_use_proxy_response(
    response: &AppResponseEnvelope,
) -> Result<ProviderUseProxyData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 permissions_settings 信封（SET-6b）：Data 为
/// `{ approval_mode, workspace_trusted, trust_workspaces_global }`；Error
/// 取 Host 脱敏 message 原文。
pub(super) fn parse_permissions_settings_response(
    response: &AppResponseEnvelope,
) -> Result<PermissionsSettingsData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 terminal_settings / set_terminal_settings 信封（SET-6d /
/// ADR-050）：Data 为 `{ shell, columns, rows }`（查询与写回执同形状）；
/// Error 取 Host 脱敏 message 原文。
pub(super) fn parse_terminal_settings_response(
    response: &AppResponseEnvelope,
) -> Result<TerminalSettingsData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 set_approval_mode 回执：Data 携带写后 `{ approval_mode }`；未知
/// 档位 fail-closed 报错（UI 保留旧值）。
pub(super) fn parse_approval_mode_confirmation(
    response: &AppResponseEnvelope,
) -> Result<ApprovalModeWire, String> {
    match &response.response {
        AppResponse::Data(data) => {
            let value = data
                .get("approval_mode")
                .cloned()
                .ok_or_else(|| "missing approval_mode".to_string())?;
            serde_json::from_value(value).map_err(|error| error.to_string())
        }
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 workspace_trust 回执：Data 携带写后 `{ workspace_trusted }`。
pub(super) fn parse_workspace_trust_confirmation(
    response: &AppResponseEnvelope,
) -> Result<bool, String> {
    match &response.response {
        AppResponse::Data(data) => data
            .get("workspace_trusted")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| "workspace trust receipt missing workspace_trusted".to_string()),
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 set_default_model 响应：Data 携带 Host 确认的 provider/model pair。
pub(super) fn parse_default_model_confirmation(
    response: &AppResponseEnvelope,
) -> Result<DefaultModelPair, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 auth_start 响应：verification_url 必填，user_code / expires_at
/// 仅 device flow 携带（PKCE 为 None）。
pub(super) fn parse_auth_started(response: &AppResponseEnvelope) -> Result<AuthStartData, String> {
    match &response.response {
        AppResponse::Data(data) => {
            serde_json::from_value(data.clone()).map_err(|error| error.to_string())
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

pub(super) fn timeline_page(
    response: &AppResponseEnvelope,
) -> Result<Option<TimelinePage>, String> {
    match &response.response {
        AppResponse::Data(data) => match data.get("timeline_page") {
            Some(page) => serde_json::from_value::<TimelinePage>(page.clone())
                .map(Some)
                .map_err(|error| format!("decode timeline page: {error}")),
            None => Ok(None),
        },
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

pub(super) fn required_str(entry: &serde_json::Value, field: &str) -> Result<String, String> {
    entry
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("entry missing {field}"))
}

pub(super) fn optional_str(entry: &serde_json::Value, field: &str) -> Option<String> {
    entry
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// diff_list_files 响应：session_id 在无会话（SessionNotFound 空响应）时缺失。
pub(super) fn parse_diff_files(
    response: &AppResponseEnvelope,
) -> Result<(Option<String>, Vec<DiffFileSummary>, Option<GitDiffInfo>), String> {
    match &response.response {
        AppResponse::Data(data) => {
            let session_id = optional_str(data, "session_id");
            let files_value = data
                .get("files")
                .ok_or_else(|| "diff list missing files".to_string())?;
            let files = files_value
                .as_array()
                .ok_or_else(|| "diff files is not an array".to_string())?
                .iter()
                .map(|entry| {
                    Ok(DiffFileSummary {
                        path: required_str(entry, "path")?,
                        status: optional_str(entry, "status").unwrap_or_else(|| "unknown".into()),
                        additions: entry
                            .get("additions")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                        deletions: entry
                            .get("deletions")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                        binary: entry
                            .get("binary")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let git = data.get("git").map(|git| GitDiffInfo {
                branch: optional_str(git, "branch"),
                work_dir: optional_str(git, "work_dir"),
                dirty_files: git.get("dirty_files").and_then(serde_json::Value::as_u64),
            });
            Ok((session_id, files, git))
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// diff_get 响应：带回 Host 实际解析的 latest session；files 为空（路径
/// 不在 diff / 无会话）时 file 为 None。
pub(super) fn parse_diff_file(
    response: &AppResponseEnvelope,
) -> Result<(Option<String>, Option<DiffFileDetail>), String> {
    match &response.response {
        AppResponse::Data(data) => {
            let session_id = optional_str(data, "session_id");
            let Some(files) = data.get("files").and_then(serde_json::Value::as_array) else {
                return Err("diff response missing files".into());
            };
            let Some(entry) = files.first() else {
                return Ok((session_id, None));
            };
            let hunks = entry
                .get("hunks")
                .and_then(serde_json::Value::as_array)
                .map(|hunks| {
                    hunks
                        .iter()
                        .map(|hunk| DiffHunkDetail {
                            header: hunk
                                .get("header")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            lines: hunk
                                .get("lines")
                                .and_then(serde_json::Value::as_array)
                                .map(|lines| {
                                    lines
                                        .iter()
                                        .map(|line| DiffLineDetail {
                                            kind: match line
                                                .get("kind")
                                                .and_then(serde_json::Value::as_str)
                                            {
                                                Some("addition") => DiffLineKind::Addition,
                                                Some("deletion") => DiffLineKind::Deletion,
                                                _ => DiffLineKind::Context,
                                            },
                                            text: line
                                                .get("text")
                                                .and_then(serde_json::Value::as_str)
                                                .unwrap_or("")
                                                .to_string(),
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok((
                session_id,
                Some(DiffFileDetail {
                    path: required_str(entry, "path")?,
                    previous_path: entry
                        .get("previous_path")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    status: optional_str(entry, "status").unwrap_or_else(|| "unknown".into()),
                    binary: entry
                        .get("binary")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    additions: entry
                        .get("additions")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    deletions: entry
                        .get("deletions")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    hunks,
                }),
            ))
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// mcp_list 响应（形状由主代理钉死）：{"servers":[{name,transport,state,tools,last_error}]}。
pub(super) fn parse_mcp_servers(
    response: &AppResponseEnvelope,
) -> Result<Vec<McpServerEntry>, String> {
    match &response.response {
        AppResponse::Data(data) => data
            .get("servers")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "mcp list missing servers".to_string())?
            .iter()
            .map(|entry| {
                Ok(McpServerEntry {
                    name: required_str(entry, "name")?,
                    transport: optional_str(entry, "transport").unwrap_or_else(|| "unknown".into()),
                    state: optional_str(entry, "state").unwrap_or_else(|| "unknown".into()),
                    tool_count: entry
                        .get("tools")
                        .and_then(serde_json::Value::as_array)
                        .map(Vec::len)
                        .unwrap_or(0) as u64,
                    last_error: optional_str(entry, "last_error"),
                })
            })
            .collect(),
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 mcp_test / mcp_server_remove 回执（SET-6c；Data 形状同 mcp_list）；
/// Error 取 Host 脱敏 message 原文（UI 保留旧清单，fail-closed）。
pub(super) fn parse_mcp_receipt(
    response: &AppResponseEnvelope,
) -> Result<Vec<McpServerEntry>, String> {
    match &response.response {
        AppResponse::Data(_) => parse_mcp_servers(response),
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_token_file_fails_closed() {
        let err = load_desktop_authentication(std::path::Path::new(
            "/nonexistent/pawork-desktop-missing.token",
        ))
        .expect_err("missing token must fail");
        assert!(
            err.contains("not found") || err.contains("unreadable"),
            "{err}"
        );
    }

    #[test]
    fn run_start_command_writes_provider_and_model() {
        let command = run_start_command(
            "s-1",
            "hi",
            Some(&("deepseek".into(), "deepseek-v4-flash".into())),
        );
        let value = serde_json::to_value(&command).expect("serialize run_start");
        assert_eq!(value["method"], "run_start");
        assert_eq!(value["params"]["provider"], "deepseek");
        assert_eq!(value["params"]["model"], "deepseek-v4-flash");
    }

    #[test]
    fn last_acked_advances_from_events_and_acks() {
        assert_eq!(advance_last_acked(None, 4), 4);
        assert_eq!(advance_last_acked(Some(4), 2), 4);
        assert_eq!(advance_last_acked(Some(4), 9), 9);
    }

    #[test]
    fn handshake_capabilities_include_terminal_streaming() {
        assert!(desktop_capabilities().contains(&GuiCapability::TerminalStreaming));
    }

    #[test]
    fn session_fork_command_targets_event_id() {
        let command = session_fork_command("s-1", "evt-9");
        let value = serde_json::to_value(&command).expect("serialize fork");
        assert_eq!(value["method"], "session_fork");
        assert_eq!(value["params"]["session_id"], "s-1");
        assert_eq!(value["params"]["parent_event_id"], "evt-9");
    }

    /// ADR-054 D1–D3：create 缺省 workspace_id、rename / archive 的冻结
    /// wire 形状（Desktop 不引入第二个业务依赖，形状钉在测试里）。
    #[test]
    fn session_lifecycle_commands_pin_wire_shapes() {
        let unassigned = serde_json::to_value(session_create_command(None)).unwrap();
        assert_eq!(unassigned["method"], "session_create");
        assert!(unassigned["params"].get("workspace_id").is_none());

        let scoped = serde_json::to_value(session_create_command(Some("ws-1"))).unwrap();
        assert_eq!(scoped["params"]["workspace_id"], "ws-1");

        let rename = serde_json::to_value(session_rename_command("s-1", "New title")).unwrap();
        assert_eq!(rename["method"], "session_rename");
        assert_eq!(rename["params"]["session_id"], "s-1");
        assert_eq!(rename["params"]["title"], "New title");

        let archive = serde_json::to_value(session_archive_command("s-1", true)).unwrap();
        assert_eq!(archive["method"], "session_archive");
        assert_eq!(archive["params"]["session_id"], "s-1");
        assert_eq!(archive["params"]["archived"], true);
    }

    #[test]
    fn terminal_commands_use_workspace_relative_cwd() {
        assert!(terminal_create_command("ws-1", Some("/tmp")).is_err());
        assert!(terminal_create_command("ws-1", Some("../secret")).is_err());
        assert!(terminal_create_command("ws-1", Some(r"C:\Windows")).is_err());
        let created = terminal_create_command("ws-1", Some("src/app")).expect("relative cwd");
        let value = serde_json::to_value(&created).expect("serialize create");
        assert_eq!(value["method"], "terminal_create");
        assert_eq!(value["params"]["workspace_id"], "ws-1");
        assert_eq!(value["params"]["working_directory"], "src/app");

        let write = serde_json::to_value(terminal_write_command("term-1", "ls\n")).unwrap();
        assert_eq!(write["method"], "terminal_write");
        assert_eq!(write["params"]["terminal_session_id"], "term-1");

        let resize = serde_json::to_value(terminal_resize_command("term-1", 80, 24)).unwrap();
        assert_eq!(resize["method"], "terminal_resize");
        assert_eq!(resize["params"]["columns"], 80);
        assert_eq!(resize["params"]["rows"], 24);

        let close = serde_json::to_value(terminal_close_command("term-1")).unwrap();
        assert_eq!(close["method"], "terminal_close");
        assert_eq!(close["params"]["terminal_session_id"], "term-1");
    }

    #[test]
    fn lifecycle_events_carry_workspace_terminal_and_epoch_identity() {
        let created = ControllerEvent::TerminalCreated {
            workspace_id: "ws-1".into(),
            terminal_session_id: "term-1".into(),
        };
        assert!(
            matches!(created, ControllerEvent::TerminalCreated { workspace_id, terminal_session_id }
            if workspace_id == "ws-1" && terminal_session_id == "term-1")
        );
        let failed = ControllerEvent::DiffFilesFailed {
            epoch: 7,
            reason: "stale".into(),
        };
        assert!(matches!(
            failed,
            ControllerEvent::DiffFilesFailed { epoch: 7, .. }
        ));
    }

    #[test]
    fn diff_and_mcp_queries_pin_wire_shapes() {
        let list = serde_json::to_value(diff_list_files_query("ws-1")).unwrap();
        assert_eq!(list["method"], "diff_list_files");
        assert_eq!(list["params"]["workspace_id"], "ws-1");

        let get = serde_json::to_value(diff_get_query("ws-1", "src/main.rs")).unwrap();
        assert_eq!(get["method"], "diff_get");
        assert_eq!(get["params"]["workspace_id"], "ws-1");
        assert_eq!(get["params"]["path"], "src/main.rs");

        let mcp = serde_json::to_value(mcp_list_query()).unwrap();
        assert_eq!(mcp["method"], "mcp_list");
        assert_eq!(mcp["params"], serde_json::Value::Null);

        let test = serde_json::to_value(mcp_test_command("context7")).unwrap();
        assert_eq!(test["method"], "mcp_test");
        assert_eq!(test["params"]["name"], "context7");

        let remove = serde_json::to_value(mcp_server_remove_command("fetch")).unwrap();
        assert_eq!(remove["method"], "mcp_server_remove");
        assert_eq!(remove["params"]["name"], "fetch");
    }

    #[test]
    fn terminal_settings_wire_pins_query_and_full_state_write() {
        // 主路径（全态写串联）：查询帧 + 全态写帧（shell Some/null 两态）
        // 与同形状回执解析（ADR-050 D2/D3）。
        let query = serde_json::to_value(terminal_settings_query()).unwrap();
        assert_eq!(query["method"], "terminal_settings");

        let set =
            serde_json::to_value(set_terminal_settings_command(Some("/bin/zsh"), 120, 40)).unwrap();
        assert_eq!(set["method"], "set_terminal_settings");
        assert_eq!(set["params"]["shell"], "/bin/zsh");
        assert_eq!(set["params"]["columns"], 120);
        assert_eq!(set["params"]["rows"], 40);

        let clear = serde_json::to_value(set_terminal_settings_command(None, 80, 24)).unwrap();
        assert_eq!(clear["params"]["shell"], serde_json::Value::Null);

        let receipt = parse_terminal_settings_response(&envelope(serde_json::json!({
            "shell": null, "columns": 80, "rows": 24
        })))
        .expect("parse terminal settings receipt");
        assert_eq!(receipt.shell, None);
        assert_eq!((receipt.columns, receipt.rows), (80, 24));
    }

    fn envelope(data: serde_json::Value) -> AppResponseEnvelope {
        serde_json::from_value(serde_json::json!({
            "api_version": { "major": 1, "minor": 1 },
            "request_id": "q-test",
            "responded_at": 0,
            "response": { "type": "data", "data": data }
        }))
        .expect("test response envelope")
    }

    #[test]
    fn parse_diff_files_reads_summaries_and_git() {
        let (session_id, files, git) = parse_diff_files(&envelope(serde_json::json!({
            "session_id": "s-1",
            "files": [
                {
                    "path": "src/app.rs",
                    "status": "modified",
                    "additions": 3,
                    "deletions": 1,
                    "binary": false
                },
                { "path": "logo.png", "status": "added", "additions": 0, "deletions": 0, "binary": true }
            ],
            "git": { "branch": "main", "work_dir": "/tmp/repo", "dirty_files": 4 }
        })))
        .expect("parse diff files");
        assert_eq!(session_id.as_deref(), Some("s-1"));
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/app.rs");
        assert_eq!(files[0].status, "modified");
        assert_eq!((files[0].additions, files[0].deletions), (3, 1));
        assert!(files[1].binary);
        let git = git.expect("git info");
        assert_eq!(git.branch.as_deref(), Some("main"));
        assert_eq!(git.dirty_files, Some(4));
    }

    #[test]
    fn parse_diff_files_marks_no_session_response() {
        let (session_id, files, git) =
            parse_diff_files(&envelope(serde_json::json!({ "files": [] })))
                .expect("empty session response parses");
        assert_eq!(session_id, None);
        assert!(files.is_empty());
        assert_eq!(git, None);
    }

    #[test]
    fn parse_diff_file_reads_hunks_and_lines() {
        let (session_id, file) = parse_diff_file(&envelope(serde_json::json!({
            "session_id": "s-1",
            "path": "src/app.rs",
            "files": [{
                "path": "src/app.rs",
                "previous_path": null,
                "status": "modified",
                "binary": false,
                "additions": 1,
                "deletions": 1,
                "hunks": [{
                    "header": "@@ -1,2 +1,2 @@",
                    "lines": [
                        { "kind": "context", "text": "fn main() {" },
                        { "kind": "addition", "text": "    println!(\"new\");" },
                        { "kind": "deletion", "text": "    println!(\"old\");" }
                    ]
                }]
            }]
        })))
        .expect("parse diff file");
        assert_eq!(session_id.as_deref(), Some("s-1"));
        let file = file.expect("file present");
        assert_eq!(file.hunks.len(), 1);
        assert_eq!(file.hunks[0].header, "@@ -1,2 +1,2 @@");
        assert_eq!(file.hunks[0].lines.len(), 3);
        assert_eq!(file.hunks[0].lines[1].kind, DiffLineKind::Addition);
        assert_eq!(file.hunks[0].lines[2].kind, DiffLineKind::Deletion);
        assert_eq!(file.hunks[0].lines[2].text, "    println!(\"old\");");

        let (session_id, missing) = parse_diff_file(&envelope(serde_json::json!({
            "session_id": "s-2",
            "path": "gone.rs",
            "files": [],
            "complete": true
        })))
        .expect("empty diff parses");
        assert_eq!(session_id.as_deref(), Some("s-2"));
        assert_eq!(missing, None);
    }

    #[test]
    fn parse_mcp_servers_reads_pinned_shape() {
        let servers = parse_mcp_servers(&envelope(serde_json::json!({
            "servers": [
                {
                    "name": "fetch",
                    "transport": "stdio",
                    "state": "ready",
                    "tools": ["fetch_url", "search"],
                    "last_error": null
                },
                {
                    "name": "broken",
                    "transport": "http",
                    "state": "failed",
                    "tools": [],
                    "last_error": "connection refused"
                }
            ]
        })))
        .expect("parse mcp servers");
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].tool_count, 2);
        assert_eq!(servers[0].last_error, None);
        assert_eq!(servers[1].state, "failed");
        assert_eq!(servers[1].last_error.as_deref(), Some("connection refused"));
    }

    #[test]
    fn parse_mcp_receipt_reads_data_and_surfaces_error_message() {
        let servers = parse_mcp_receipt(&envelope(serde_json::json!({
            "servers": [
                {
                    "name": "fetch",
                    "transport": "stdio",
                    "state": "ready",
                    "tools": ["fetch_url"],
                    "last_error": null
                }
            ]
        })))
        .expect("parse mcp receipt");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "fetch");

        let error: AppResponseEnvelope = serde_json::from_value(serde_json::json!({
            "api_version": { "major": 1, "minor": 1 },
            "request_id": "q-test",
            "responded_at": 0,
            "response": {
                "type": "error",
                "data": {
                    "category": "invalid_request",
                    "message": "unknown mcp server",
                    "retryable": false
                }
            }
        }))
        .expect("test error envelope");
        assert_eq!(
            parse_mcp_receipt(&error).expect_err("error receipt must fail closed"),
            "unknown mcp server"
        );
    }
}
