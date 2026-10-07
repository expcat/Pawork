//! CU-17（API 1.27）：GUI 观测证据与 computer-use 目标状态的 wire 词汇。
//!
//! 两条边界：
//! - 观测证据只引用持久化事件中的图像（session 事件账本），读取永不触发
//!   新截图或输入；巨幅 Base64 不进入 Timeline 帧，图像字节经有界读取
//!   分块获取。
//! - 目标状态形状镜像 CU-02 已裁决契约（pawork-computer-use 的
//!   TargetIdentity / GrantKind / 能力矩阵），由 Host 映射；本包不依赖
//!   computer-use（protocol 只依赖 domain 的红线不变）。

use pawork_domain::{RunId, SessionId};
use serde::{Deserialize, Serialize};
#[cfg(feature = "typegen")]
use ts_rs::TS;

/// observation_image_read 单次调用的解码字节上限（与 artifact 块同量级）。
pub const MAX_OBSERVATION_READ_BYTES: u64 = 64 * 1024;
/// observation_list 单页条数上限（与 timeline 分页同口径）。
pub const MAX_OBSERVATION_LIST_LIMIT: u32 = 500;
/// observation_list 缺省页大小。
pub const DEFAULT_OBSERVATION_LIST_LIMIT: u32 = 200;
/// observation_list 单页条目序列化字节预算（审查修复：帧上限防线的
/// 第二道闸，事件粒度停止保证可完整续读）。
pub const MAX_OBSERVATION_LIST_RESPONSE_BYTES: usize = 512 * 1024;
/// computer_target_list 单页目标数上限（审查修复：响应帧有界）。
pub const MAX_COMPUTER_TARGET_LIST_TARGETS: usize = 256;
/// computer_target_list 完整响应的序列化字节预算（审查修复：条目数
/// 上限不足以约束帧大小——bundle_id 等字段无长度上限，超预算整页
/// 拒绝，为 1 MiB 帧上限留出信封余量）。
pub const MAX_COMPUTER_TARGET_LIST_RESPONSE_BYTES: usize = 512 * 1024;

/// 应用族（CU-01 实测能力矩阵的族划分）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum ComputerAppFamily {
    Appkit,
    Chromium,
    Browser,
}

/// 应用身份：bundle id + 族。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ComputerAppIdentity {
    pub bundle_id: String,
    pub family: ComputerAppFamily,
}

/// 目标身份：整应用或浏览器内单源网站（镜像 CU-02 TargetIdentity 形状；
/// Host 侧构造时重新校验并归一化 origin）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ComputerTargetIdentity {
    Application { app: ComputerAppIdentity },
    Website {
        browser: ComputerAppIdentity,
        origin: String,
    },
}

/// 授权寿命，镜像既有审批决定词汇。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum ComputerGrantKind {
    Once,
    ForRun,
}

/// 目标状态条目的 scope（CU-02 Scope = workspace + run）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ComputerTargetScope {
    pub workspace_id: String,
    pub run_id: String,
}

/// 后台能力裁决（CU-01 实测矩阵）：unverified 与 unsupported 同拒。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum ComputerCapabilitySupport {
    Supported,
    Unsupported,
    Unverified,
}

/// CU-02 十动作词汇。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum ComputerActionKind {
    CaptureWindow,
    AxRead,
    AxSemanticWrite,
    AxPress,
    TargetedKeys,
    MenuShortcut,
    MenubarPress,
    Pointer,
    WindowMove,
    WindowResize,
}

/// 单动作的后台能力：background 为常规态，minimized 为最小化态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ComputerTargetCapability {
    pub action: ComputerActionKind,
    pub background: ComputerCapabilitySupport,
    pub minimized: ComputerCapabilitySupport,
}

/// computer_target_list 的单条目标状态。registry 记录的事实视图：
/// 绑定/授权/占用/暂停的登记状态，活性在使用时校验（列表不做 probe）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ComputerTargetState {
    pub target: ComputerTargetIdentity,
    pub scope: ComputerTargetScope,
    /// 当前有效授权；None = 无授权（未批准、已消耗的一次授权或已撤销）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<ComputerGrantKind>,
    /// 已绑定窗口的宿主句柄；None = 未绑定。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_window: Option<String>,
    /// 该目标占用边界（应用进程）当前的拥有者 scope；可能不是本 scope。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupied_by: Option<ComputerTargetScope>,
    /// 用户接管暂停（CU-09）：true 时派发让出，租约不消耗。
    pub dispatch_paused: bool,
    /// 该目标族 × 十动作的后台能力裁决。
    pub capabilities: Vec<ComputerTargetCapability>,
}

/// computer_target_list 的响应 Data。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ComputerTargetListData {
    pub session_id: SessionId,
    /// 会话归属的 workspace；无归属会话恒为空列表。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub targets: Vec<ComputerTargetState>,
}

/// 观测证据状态：持久化读取只产出 historical（历史记录，不是可用租约；
/// 历史读取不触发新截图或输入）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum ObservationEvidenceStatus {
    Historical,
}

/// observation_list 的单条观测图像引用（身份 = session 内事件序号 +
/// part_index，分页 / 重连不错配）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ObservationEvidenceEntry {
    pub sequence: u64,
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub part_index: u32,
    pub media_type: String,
    /// 解码后字节数。
    pub byte_length: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_height: Option<u32>,
    /// 事件落库时间（unix 毫秒）。
    pub captured_at_ms: u64,
    pub status: ObservationEvidenceStatus,
}

/// observation_list 的响应 Data；分页游标语义与 timeline 一致
///（游标按事件序推进，不按过滤后条目）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ObservationListData {
    pub session_id: SessionId,
    pub entries: Vec<ObservationEvidenceEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_sequence: Option<u64>,
    pub head_sequence: u64,
    pub complete: bool,
}

/// observation_image_read 的响应 Data：data 为解码字节切片
/// [offset, offset+len) 的 Base64。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ObservationImageData {
    pub session_id: SessionId,
    pub sequence: u64,
    pub event_id: String,
    pub part_index: u32,
    pub media_type: String,
    pub byte_length: u64,
    pub offset: u64,
    pub data: String,
    pub eof: bool,
    pub captured_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_height: Option<u32>,
    pub status: ObservationEvidenceStatus,
}

/// computer_target_approve 命令参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ComputerTargetApproveParams {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub target: ComputerTargetIdentity,
    pub kind: ComputerGrantKind,
}

/// computer_target_revoke 命令参数（撤销授权并释放占用，即目标级停止）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct ComputerTargetRevokeParams {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub target: ComputerTargetIdentity,
}
