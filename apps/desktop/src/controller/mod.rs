//! Controller 层：唯一业务出口是 pawork-client。
//!
//! 职责：连接握手 + 事件泵、SessionGet 分页、SessionCreate / SessionFork /
//! RunStart / RunCancel / ToolApprove / ModelList，以及 TerminalCreate /
//! TerminalWrite / TerminalResize。重连走 [`GuiClient::connect_with_resume_config`]，
//! 记录 last_acked `global_sequence`（来自事件与 Ack），按 Replay /
//! SnapshotRequired / UpToDate 三态交给 projection。

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use pawork_client::{
    ActorIdentity, AppCommand, AppEvent, AppEventEnvelope, AppQuery, AppResponse,
    AppResponseEnvelope, ApprovalModeWire, AuthStartData, ClientConfig, ClientError, CommandSource,
    ComputerTargetListData, ConnectOptions, DefaultModelPair, GeneralSettingsData, GlobalSequence,
    GuiCapability, GuiClient, GuiTransportClient, LocalTransport, ObservationImageData,
    ObservationListData, PermissionsSettingsData, ProtocolErrorCode, ProviderAuthStatusData,
    ResumeDisposition, ResumeOutcome, Snapshot, SubagentListData, SubagentSettingsData,
    TerminalSettingsData, TimelinePage, TransportEndpoint,
};
use serde_json::json;

use crate::projection::{sessions_in_snapshot, ModelEntry, SettingsRole};

pub(super) const PAGE_LIMIT: u32 = 500;
pub(super) const MAX_PAGES: usize = 200;

/// UI 消费的控制器事件（经 smol channel 跨线程投递）。
#[derive(Clone, Debug)]
pub enum ControllerEvent {
    ComposerAttachmentsLoaded {
        draft: Option<String>,
        result: Result<Vec<ComposerAttachment>, ComposerAttachmentError>,
    },
    WorkspaceFileResult {
        workspace_id: String,
        path: String,
        epoch: u64,
        operation: FileOperation,
        result: Result<serde_json::Value, String>,
    },
    BrowserRequest {
        session_id: String,
        run_id: String,
        request_id: String,
        action: serde_json::Value,
    },
    Disconnected {
        reason: String,
    },
    Snapshot(Snapshot),
    TimelineLoaded {
        session_id: String,
        page: TimelinePage,
    },
    Event(AppEventEnvelope),
    SessionArchiveFinished {
        session_id: String,
        archived: bool,
        result: Result<String, String>,
    },
    SessionCreated {
        session_id: String,
    },
    WorkspaceOpened {
        workspace_id: String,
        name: String,
        create_task: bool,
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
    AccountModeFinished {
        provider_id: String,
        epoch: u64,
        data: Option<ProviderAuthStatusData>,
    },
    AccountQuotaLoaded {
        provider_id: String,
        credential_id: String,
        epoch: u64,
        view: Option<pawork_client::QuotaOverviewView>,
    },
    /// set_default_role_model 获 Host Data 确认（OPT-3b / ADR-055 D5；回执
    /// 即写后状态，不重查）：按角色落地键对；conversation 另同步 Composer
    /// 已确认默认。
    DefaultRoleModelConfirmed {
        role: SettingsRole,
        value: Option<DefaultModelPair>,
    },
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
    /// model_list（include_disabled=true）查询成功（OPT-3a / ADR-055 D4）：
    /// Settings「Manage models」弹层的全量目录权威状态。
    ModelCatalogLoaded(Vec<ModelEntry>),
    /// set_model_enabled 获 Host Data 确认（OPT-3a / ADR-055 D2/D3；回执
    /// 即写后状态）：弹层先按回执收敛再重查权威全态；cleared_roles 诚实
    /// 呈现，不静默换绑。
    ModelEnabledConfirmed {
        provider_id: String,
        model_id: String,
        enabled: bool,
        cleared_roles: Vec<String>,
    },
    /// set_provider_models_enabled 获 Host Data 确认（OPT-3a；同上）。
    ProviderModelsEnabledConfirmed {
        provider_id: String,
        enabled: bool,
        cleared_roles: Vec<String>,
    },
    /// set_model_reasoning 获 Host Data 确认（ADR-063 / API 1.21；回执
    /// 即写后状态）：目录条目按回执收敛默认强度与手动范围。
    ModelReasoningConfirmed {
        provider_id: String,
        model_id: String,
        default_effort: Option<String>,
        manual_efforts: Option<Vec<String>>,
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
    /// subagent_settings 查询成功（Settings 子代理页；Host 权威 Global 配置）。
    SubagentSettingsLoaded(SubagentSettingsData),
    /// set_subagent_settings 获 Host Data 确认（回执即写后完整状态）。
    SubagentSettingsConfirmed(SubagentSettingsData),
    /// subagent_list 查询成功（Activity 浮层「子智能体」卡；按 session 落地）。
    SubagentListLoaded {
        session_id: String,
        request_id: u64,
        data: SubagentListData,
    },
    SubagentListFailed {
        session_id: String,
        request_id: u64,
        reason: String,
    },
    SubagentCancelFinished {
        session_id: String,
        agent_id: String,
        result: Result<(), String>,
    },
    /// 子代理会话时间线一页（Inspector「子代理」对话栏；session_id 为
    /// 子会话 id，与主 TimelineLoaded 分流，互不污染）。generation 为
    /// 发起加载时的对话栏代际，过期回执被状态侧丢弃。
    SubagentTimelineLoaded {
        session_id: String,
        generation: u32,
        page: TimelinePage,
    },
    /// 子代理会话时间线分页失败（按子会话 id + 代际归属，避免清错代理
    /// 或旧链迟到失败污染新链）。
    SubagentTimelineFailed {
        session_id: String,
        generation: u32,
        reason: String,
    },
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
    /// computer_target_list 查询成功（CU-18，GUI API 1.27；按 session +
    /// 请求代次归属，迟到回执不覆盖新查询）。
    ComputerTargetsLoaded {
        session_id: String,
        request_id: u64,
        data: ComputerTargetListData,
    },
    ComputerTargetsFailed {
        session_id: String,
        request_id: u64,
        reason: String,
    },
    /// computer_target_approve / revoke 的回执：Ok 表示 Host 已落地
    /// 授权决定，UI 随即重查权威列表；Err 为结构化 / 传输原因，落在
    /// 面板内呈现（不冒充成功）。action_id 为派出代次：A→B→A 切换或
    /// 断线后的迟到回执不匹配当前在途代次，不收敛状态也不触发重查。
    ComputerTargetActionFinished {
        session_id: String,
        action_id: u64,
        result: Result<(), String>,
    },
    /// observation_list 查询成功（CU-19，GUI API 1.27；按 session + 请求
    /// 代次归属，迟到回执不覆盖新查询）。条目只读持久化事件账本
    ///（status=historical），不触发新截图或输入。
    ObservationsLoaded {
        session_id: String,
        request_id: u64,
        data: ObservationListData,
    },
    ObservationsFailed {
        session_id: String,
        request_id: u64,
        reason: String,
    },
    /// observation_image_read 单块回执（CU-19）：data 为 [offset,
    /// offset+len) 解码字节的 Base64；eof=false 时 UI 以回显 offset 续读，
    /// 单块上限 MAX_OBSERVATION_READ_BYTES。
    ObservationImageChunk {
        session_id: String,
        request_id: u64,
        data: ObservationImageData,
    },
    ObservationImageFailed {
        session_id: String,
        request_id: u64,
        reason: String,
    },
    /// run_cancel（CU-19 停止后台任务）的回执：Ok 只表示取消命令已被
    /// Host 接受，不等于已停止；真实释放 / 终态由 UI 重查目标列表与 run
    /// 终态事件确认。run_id 随回执带回供停止状态机归属。
    ComputerStopFinished {
        session_id: String,
        action_id: u64,
        run_id: String,
        result: Result<(), String>,
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
    browser_polling: std::sync::atomic::AtomicBool,
}

pub struct DesktopController {
    runtime: tokio::runtime::Handle,
    state: Arc<SharedState>,
}

mod attachments;
mod browser;
mod computer;
mod files;
mod products;
#[cfg(test)]
pub(crate) use attachments::read_composer_files_for_test;
pub use attachments::{ComposerAttachment, ComposerAttachmentError, ComposerOptions};
pub use files::FileOperation;
mod session;
mod settings;
mod terminal;
mod wire;
use wire::*;

impl DesktopController {
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self {
            runtime,
            state: Arc::new(SharedState {
                client: Mutex::new(None),
                events: Mutex::new(None),
                last_acked: Mutex::new(None),
                generation: AtomicU64::new(0),
                browser_polling: std::sync::atomic::AtomicBool::new(false),
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

    /// 连接 + 握手 + 订阅。有 last_ack 时走 `connect_with_resume_config`，不要永远
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
                        let meta_changed =
                            matches!(event.payload, AppEvent::SessionMetaChanged { .. });
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
                                        let _ =
                                            events.send(ControllerEvent::Snapshot(snapshot)).await;
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

fn record_shared_last_acked(state: &SharedState, sequence: u64) {
    let mut slot = state.last_acked.lock().unwrap_or_else(|p| p.into_inner());
    *slot = Some(advance_last_acked(*slot, sequence));
}

fn advance_last_acked(current: Option<u64>, incoming: u64) -> u64 {
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
        GuiCapability::BrowserControl,
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
                GuiCapability::BrowserControl => "browser_control",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_capabilities_pin_desktop_surface() {
        assert_eq!(
            desktop_capabilities(),
            vec![
                GuiCapability::Events,
                GuiCapability::Snapshots,
                GuiCapability::Approvals,
                GuiCapability::TerminalStreaming,
                GuiCapability::BrowserControl,
            ],
        );
    }

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
            Some("high"),
            &ComposerOptions::default(),
        );
        let value = serde_json::to_value(&command).expect("serialize run_start");
        assert_eq!(value["method"], "run_start");
        assert_eq!(value["params"]["provider"], "deepseek");
        assert_eq!(value["params"]["model"], "deepseek-v4-flash");
        assert_eq!(value["params"]["effort"], "high");
        // 未显式选择（自动）时省略 effort 参数（ADR-063 回落语义）。
        let command = run_start_command("s-1", "hi", None, None, &ComposerOptions::default());
        let value = serde_json::to_value(&command).expect("serialize run_start");
        assert!(value["params"].get("effort").is_none());
        assert!(value["params"].get("attachment_ids").is_none());
        assert!(value["params"].get("web_search").is_none());
        let options = ComposerOptions {
            attachments: vec![ComposerAttachment {
                id: "picked-1".into(),
                name: "image.png".into(),
                bytes: Arc::new(vec![1]),
                image: true,
            }],
            web_search: Some(false),
            video_urls: Vec::new(),
            attachment_error: None,
        };
        let value = serde_json::to_value(run_start_command("unassigned", "", None, None, &options))
            .unwrap();
        assert_eq!(value["params"]["attachment_ids"], json!(["picked-1"]));
        assert_eq!(value["params"]["web_search"], false);
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
    fn terminal_create_receipt_accepts_only_terminal_session_id() {
        let canonical = envelope(json!({
            "terminal_session_id": "term-1",
            "id": "other",
            "sandboxed": false
        }));
        assert_eq!(terminal_session_id(&canonical).as_deref(), Some("term-1"));

        let id_only = envelope(json!({ "id": "term-1" }));
        assert_eq!(terminal_session_id(&id_only), None);

        let malformed = envelope(json!({
            "terminal_session_id": 1,
            "id": "term-1"
        }));
        assert_eq!(terminal_session_id(&malformed), None);
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

    /// OPT-3b / ADR-055 D5：set_default_role_model 的冻结 wire 形状与
    /// 回执解析（set / clear / 未知 role / 缺 value / Error 信封）。
    #[test]
    fn default_role_model_pins_wire_and_receipt_parsing() {
        let pair = DefaultModelPair {
            provider_id: "kimi".into(),
            model_id: "kimi-k2-0905-preview".into(),
        };
        let set =
            serde_json::to_value(set_default_role_model_command("naming", Some(&pair))).unwrap();
        assert_eq!(set["method"], "set_default_role_model");
        assert_eq!(set["params"]["role"], "naming");
        assert_eq!(set["params"]["value"]["provider_id"], "kimi");
        assert_eq!(set["params"]["value"]["model_id"], "kimi-k2-0905-preview");

        let clear = serde_json::to_value(set_default_role_model_command("vision", None)).unwrap();
        assert_eq!(clear["params"]["role"], "vision");
        assert_eq!(clear["params"]["value"], serde_json::Value::Null);

        let receipt = |role: &str, value: serde_json::Value| {
            envelope(serde_json::json!({ "role": role, "value": value }))
        };
        let (role, value) = parse_default_role_model_confirmation(&receipt(
            "search",
            serde_json::json!({
                "provider_id": "deepseek",
                "model_id": "deepseek-v4-flash"
            }),
        ))
        .expect("parse set receipt");
        assert_eq!(role, SettingsRole::Search);
        assert_eq!(
            value,
            Some(DefaultModelPair {
                provider_id: "deepseek".into(),
                model_id: "deepseek-v4-flash".into(),
            })
        );

        let (role, value) =
            parse_default_role_model_confirmation(&receipt("naming", serde_json::Value::Null))
                .expect("parse clear receipt");
        assert_eq!(role, SettingsRole::Naming);
        assert_eq!(value, None);

        assert!(parse_default_role_model_confirmation(&receipt(
            "summarizer",
            serde_json::Value::Null
        ))
        .is_err());
        assert!(
            parse_default_role_model_confirmation(&envelope(serde_json::json!({
                "role": "vision"
            })))
            .is_err()
        );
        let error = serde_json::from_value(serde_json::json!({
            "api_version": { "major": 1, "minor": 12 },
            "request_id": "q-test",
            "responded_at": 0,
            "response": {
                "type": "error",
                "data": {
                    "category": "invalid_request",
                    "message": "provider disabled",
                    "retryable": false
                }
            }
        }))
        .expect("test error envelope");
        assert_eq!(
            parse_default_role_model_confirmation(&error).unwrap_err(),
            "provider disabled"
        );
    }

    /// OPT-3a / ADR-055 D2-D4：模型启用 wire 冻结形状与回执解析
    ///（目录查询显式 include_disabled=true；cleared_roles 原样带回；
    /// Error 取 Host 脱敏 message 原文；缺字段 fail-closed）。
    #[test]
    fn model_enablement_pins_wire_and_receipts() {
        let catalog = serde_json::to_value(model_catalog_query()).unwrap();
        assert_eq!(catalog["method"], "model_list");
        assert_eq!(catalog["params"]["include_disabled"], true);
        let filtered = serde_json::to_value(model_list_query()).unwrap();
        assert_eq!(
            filtered["params"].as_object().map(|params| params.len()),
            Some(0)
        );

        let enable =
            serde_json::to_value(set_model_enabled_command("kimi", "kimi-k2", false)).unwrap();
        assert_eq!(enable["method"], "set_model_enabled");
        assert_eq!(enable["params"]["provider_id"], "kimi");
        assert_eq!(enable["params"]["model_id"], "kimi-k2");
        assert_eq!(enable["params"]["enabled"], false);

        let all = serde_json::to_value(set_provider_models_enabled_command("kimi", false)).unwrap();
        assert_eq!(all["method"], "set_provider_models_enabled");
        assert_eq!(all["params"]["provider_id"], "kimi");
        assert_eq!(all["params"]["enabled"], false);

        let receipt = parse_model_enabled_confirmation(&envelope(serde_json::json!({
            "provider_id": "kimi",
            "model_id": "kimi-k2",
            "enabled": false,
            "cleared_roles": ["naming", "vision"]
        })))
        .expect("parse model receipt");
        assert_eq!(
            receipt,
            ModelEnabledReceipt {
                provider_id: "kimi".into(),
                model_id: "kimi-k2".into(),
                enabled: false,
                cleared_roles: vec!["naming".into(), "vision".into()],
            }
        );
        // 畸形 cleared_roles 条目剔除，缺键视为空（启用路径恒为空）。
        let enabled_receipt = parse_model_enabled_confirmation(&envelope(serde_json::json!({
            "provider_id": "kimi",
            "model_id": "kimi-k2",
            "enabled": true,
            "cleared_roles": ["naming", 7]
        })))
        .expect("parse enabled receipt");
        assert_eq!(enabled_receipt.cleared_roles, vec!["naming".to_string()]);

        let all_receipt =
            parse_provider_models_enabled_confirmation(&envelope(serde_json::json!({
                "provider_id": "kimi",
                "enabled": false,
                "cleared_roles": ["conversation"]
            })))
            .expect("parse provider models receipt");
        assert_eq!(all_receipt.provider_id, "kimi");
        assert!(!all_receipt.enabled);
        assert_eq!(all_receipt.cleared_roles, vec!["conversation".to_string()]);

        assert!(
            parse_model_enabled_confirmation(&envelope(serde_json::json!({
                "provider_id": "kimi",
                "model_id": "kimi-k2"
            })))
            .is_err()
        );
        assert!(parse_provider_models_enabled_confirmation(&envelope(
            serde_json::json!({ "provider_id": "kimi", "enabled": true })
        ))
        .is_ok());
        assert!(parse_provider_models_enabled_confirmation(&envelope(
            serde_json::json!({ "provider_id": "kimi" })
        ))
        .is_err());
        let error = serde_json::from_value(serde_json::json!({
            "api_version": { "major": 1, "minor": 12 },
            "request_id": "q-test",
            "responded_at": 0,
            "response": {
                "type": "error",
                "data": {
                    "category": "unavailable",
                    "message": "catalog unavailable",
                    "retryable": true
                }
            }
        }))
        .expect("test error envelope");
        assert_eq!(
            parse_provider_models_enabled_confirmation(&error).unwrap_err(),
            "catalog unavailable"
        );
    }

    /// ADR-055 D4：model_list 条目 enabled 为 additive 字段——有值按值
    /// 读，缺字段（旧 Host）视为启用。
    #[test]
    fn parse_models_reads_enabled_flag_additively() {
        let models = parse_models(&envelope(serde_json::json!([
            {
                "provider_id": "kimi",
                "id": "kimi-k2",
                "display_name": "Kimi K2",
                "enabled": false
            },
            { "provider_id": "glm", "id": "glm-4.7", "display_name": "GLM 4.7" }
        ])))
        .expect("parse models");
        assert_eq!(models.len(), 2);
        assert_eq!(
            (models[0].id.as_str(), models[0].enabled),
            ("kimi-k2", false)
        );
        assert_eq!(
            (models[1].id.as_str(), models[1].enabled),
            ("glm-4.7", true)
        );
    }

    /// ADR-064（API 1.25）：additive 能力位——text 缺省 true（旧 Host 目录
    /// 只含对话模型），image_output 缺省 false；显式值原样读取。
    #[test]
    fn parse_models_reads_capability_bits_additively() {
        let models = parse_models(&envelope(serde_json::json!([
            { "provider_id": "qwen", "id": "wan2.7-image", "text": false, "image_output": true },
            { "provider_id": "glm", "id": "glm-4.7" }
        ])))
        .expect("parse models");
        assert_eq!(models.len(), 2);
        assert!(!(models[0].text) && models[0].image_output);
        assert!(models[1].text && !models[1].image_output);
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
