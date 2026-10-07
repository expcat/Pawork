//! Inspector「后台操作」页（CU-18/19，GUI API 1.27）：computer-use 目标
//! 状态、授权管理与观测证据。数据只来自 Host computer_target_list /
//! observation_list（登记事实与持久化事件账本视图），授权 / 撤销经
//! computer_target_approve / revoke 命令，停止任务经 run_cancel；Desktop
//! 不依赖 computer-use、不读数据库、不直连 Provider（红线不变）。
//!
//! 语义边界：
//! - 授权按 run scope 生效，永不持久化；run 终态 Host 侧清授权并释放
//!   占用（CU-16/17）。面板如实呈现该口径，不提供任何持久授权入口。
//! - 面板交互只在 Desktop 进程内选择 / 决定，选择目标不激活目标应用；
//!   approve / revoke 只接受真实用户输入路径（鼠标 / 键盘 / AX 共用同一
//!   gate），不存在自动批准路径。
//! - 能力矩阵为 Host 按目标族裁决的后台支持度；unverified 与
//!   unsupported 同拒（CU-02）。系统权限（Screen Recording /
//!   Accessibility）状态不经 wire 传输，面板只在存在未验证能力时给出
//!   可执行指引，不冒充探测结果。
//! - 观测证据只读持久化事件（CU-17：historical，读取不触发新截图或
//!   输入）；图像字节经 observation_image_read 有界分块重组，只在内存
//!   解码缓存，不写仓库。历史观测与当前（最新）观测以身份
//!   (sequence, part_index) 区分，分页 / 重连 / 切换选择不错配。
//! - 停止是状态机不是按钮回执：run_cancel 被接受只进入「停止中」，
//!   已停止只能由 run 终态事件确认（空列表不裁决释放——Host 取消即
//!   移除 registry 登记，清理可能仍在执行）；确认查询失败与确认前
//!   断线分别如实呈现，点击永不直接显示成功。GUI 关闭不取消 Core
//!   Run（既有合同不变）。

use gpui::{div, prelude::*, px, Context, ScrollHandle, SharedString, Window};

use pawork_client::{
    ComputerActionKind, ComputerAppFamily, ComputerCapabilitySupport, ComputerGrantKind,
    ComputerTargetCapability, ComputerTargetIdentity, ComputerTargetListData,
    ComputerTargetState, MAX_OBSERVATION_READ_BYTES, ObservationEvidenceEntry,
    ObservationImageData, ObservationListData,
};

use crate::projection::ConnectionState;
use crate::ui::accessibility::{dynamic_identifier, AxAction, AxNode, AxRect, AxRole};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::empty_state::EmptyState;
use crate::ui::components::icon::{icon_sized, Icon};
use crate::ui::components::label::Label;
use crate::ui::components::skeleton::loading_skeleton;
use crate::ui::i18n::t;
use crate::ui::theme::{dark, font, metrics};

use super::{AppView, INSPECTOR_TAB_INDEX};

/// AX / 布局 / 键盘焦点共用的动作 id 前缀（render 与 AX 同源；dispatch
/// 按前缀反查目标，与 subagent-select- 同构）。
pub(super) const COMPUTER_APPROVE_ONCE_PREFIX: &str = "computer-approve-once-";
pub(super) const COMPUTER_APPROVE_RUN_PREFIX: &str = "computer-approve-run-";
pub(super) const COMPUTER_REVOKE_PREFIX: &str = "computer-revoke-";
pub(super) const COMPUTER_STOP_PREFIX: &str = "computer-stop-";

/// 观测证据身份：(session 内事件 sequence, part_index)，与 Host 端身份
/// 同源；分页 / 重连 / 切换选择不错配。
pub(super) type EvidenceKey = (u64, u32);

/// 证据刷新的事件窗口（事件粒度）：只取会话尾部，更早历史不逐页全量
/// 拉取（有界加载；截断如实标注）。
const OBSERVATION_EVENT_WINDOW: u64 = 300;
/// 尾窗内预算截断（Host 单页 512 KiB 条目上限）时的同代次续读页数
/// 上限；耗尽即如实截断落库（有界请求，不逐页追完全量历史）。
const OBSERVATION_MAX_LIST_PAGES: u32 = 3;
/// 证据行渲染上限；更早条目以计数说明（有界渲染）。
const OBSERVATION_MAX_ROWS: usize = 30;
/// 解码图像缓存上限（张数）；截图只在内存，不落盘。
const OBSERVATION_IMAGE_CACHE: usize = 4;
/// 单张截图的接收 / 解码预算（解码像素内存远超字节量，须设整图上限）。
const OBSERVATION_IMAGE_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// 单帧解码像素预算（宽×高；解码内存 ≈ 像素 × 4B，16M 像素 ≈ 64 MiB
///——压缩字节预算约束不了解码内存，小体积大尺寸图仍可分配数百 MiB）。
const OBSERVATION_IMAGE_MAX_PIXELS: u64 = 16 * 1024 * 1024;
/// 动图帧数预算（GIF 全帧校验与 GPUI 渲染解码一致，按帧累计）。
const OBSERVATION_IMAGE_MAX_FRAMES: u32 = 60;
/// 动图全帧累计像素预算（≈ 192 MiB 解码内存；GPUI 渲染解码全部帧）。
const OBSERVATION_IMAGE_MAX_TOTAL_PIXELS: u64 = 48 * 1024 * 1024;

/// 面板拉取状态（与 ResourcesFetch 同族，reason 单列保持 Copy）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComputerFetch {
    Idle,
    Fetching,
    Ready,
    Failed,
}

/// 停止后台任务的状态机（CU-19）。已停止只由真实 run 终态事件确认；
/// run_cancel 回执（Ok / Err）都只进入确认阶段，不冒充终态——Host 取消
/// 即从 registry 移除 active run，目标列表随之为空，但实际清理仍在执行，
/// 空列表不能独立裁决已停止。
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ComputerStopPhase {
    /// run_cancel 已派出，等待回执。
    Dispatching,
    /// 回执已到：note=None 为 Host 受理取消；note=Some 为结构化拒绝
    ///（如 unknown_run——可能本就已终态）。两者都等待 run 终态事件
    /// 确认；重查列表只用于「被拒且仍持有」回落错误与停止记录退役。
    Confirming { note: Option<String> },
    /// 已停止：收到本 run 的终态事件（Cancelled / Completed / Failed /
    /// Interrupted）。终态，迟到回执不得改写。
    Stopped,
    /// 派发后观测失败：回执后的持有核对查询失败（可刷新重试；终态
    /// 事件到达仍会转 Stopped）。
    ObserveFailed { reason: String },
    /// 结果未知：确认前断线（重连后重查收敛）。
    Unknown,
}

/// 一次停止任务的全部本地事实（面板单槽：同一时刻只追一个停止）。
#[derive(Debug, Clone)]
pub(super) struct ComputerStop {
    /// 发起的目标卡 key（展示锚点；卡片消失后状态条仍在）。
    pub key: String,
    pub run_id: String,
    /// 派出代次：迟到回执不收敛新停止。
    pub action_id: u64,
    pub phase: ComputerStopPhase,
}

/// 单条观测图像的分块读取状态（一次一张；换目标即放弃在途）。
#[derive(Debug, Clone)]
pub(super) struct EvidenceLoad {
    pub key: EvidenceKey,
    pub request_id: u64,
    pub next_offset: u64,
    pub received: u64,
    /// 条目申报的解码总字节数（Host 同源；超限 fail-closed）。
    pub total: u64,
    pub buffer: Vec<u8>,
}

/// 收齐待解码的证据字节（eof 后由 AppView 认领，交后台执行器做真实
/// 解码校验；解码是 CPU 密集工作，不得在 UI 线程同步执行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EvidenceDecode {
    pub key: EvidenceKey,
    pub request_id: u64,
    pub format: gpui::ImageFormat,
    pub bytes: Vec<u8>,
}

/// observation_image_read 单块回执的处理结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ObservationChunkOutcome {
    /// 校验通过，续读下一块（返回回显偏移）。
    Continue(EvidenceKey, u64),
    /// 收齐（字节总量 + 魔数嗅探已过）：移交后台解码校验。
    Decode(EvidenceDecode),
    /// 无在途 / 已终止 / 校验失败（失败路径清在途并记录原因）。
    Settled,
}

/// 后台解码校验的失败类别（UI 侧映射为本地化文案）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ObservationDecodeError {
    /// 超出像素 / 帧数 / 累计解码预算（数据未损坏，只是不接受）。
    Budget,
    /// 真实解码失败（损坏或截断）。
    Decode,
}

/// 「后台操作」面板状态。数据绑定查询时的 session；切换活动会话即随
/// begin_loading 复位，不跨会话展示。断线保留旧数据并标记 stale（与
/// Resources / 子代理卡同口径），重连后由 refresh 重查收敛。
pub(super) struct ComputerPanelState {
    session_id: Option<String>,
    request_id: u64,
    pub fetch: ComputerFetch,
    pub fetch_error: Option<String>,
    /// Host 回执的会话 workspace 归属（None = 无项目会话，恒空列表）。
    pub workspace_id: Option<String>,
    pub targets: Vec<ComputerTargetState>,
    pub stale_reason: Option<String>,
    /// 在途授权动作的（目标 key, 派出代次）（一次一个，防双击重复派出；
    /// 代次随切会话 / 断线失效，迟到回执不覆盖新动作）。
    pub action_in_flight: Option<(String, u64)>,
    /// approve / revoke 失败原因（wire 原文，不翻译）；成功回执清除。
    pub action_error: Option<String>,
    /// 持久化观测证据（新→旧：sequence 降序，part_index 次之）。
    pub observations: Vec<ObservationEvidenceEntry>,
    /// 证据查询代次（与目标列表代次独立；断线归零防迟到落地）。
    obs_request_id: u64,
    /// 首查为 head 探测页（limit=1）；!complete 时同代次续读尾部窗口。
    obs_probe: bool,
    /// 本轮查询已消费的尾窗 / 续读数据页数（探测页不计；0 = 尾窗首页）。
    obs_pages: u32,
    /// 本轮查询开始前的最新证据身份（跟随最新判定用；多页累积期间
    /// self.observations 会被逐页改写，起点须在 begin 时钉住）。
    obs_previous_newest: Option<EvidenceKey>,
    pub obs_fetch: ComputerFetch,
    pub obs_error: Option<String>,
    /// 会话 head 事件序（探测页习得；后续刷新直接跳尾部窗口）。
    pub obs_head: u64,
    /// true = 只保证覆盖最近事件窗口（更早历史被截断，如实标注）。
    pub obs_truncated: bool,
    /// 当前选中 / 预览的证据身份（默认最新一条）。
    pub evidence_selected: Option<EvidenceKey>,
    /// 解码图像缓存（有界张数；只在内存）。
    pub evidence_images: std::collections::HashMap<EvidenceKey, std::sync::Arc<gpui::Image>>,
    evidence_order: Vec<EvidenceKey>,
    /// 待移除 GPUI 全局 asset（解码结果缓存）的图像；状态层只入队，
    /// AppView 侧 drain 时调用 remove_asset（面板状态无 App 上下文）。
    pub evidence_evicted: Vec<std::sync::Arc<gpui::Image>>,
    /// 在途分块读取（一次一张；None = 空闲 / 已终止）。
    pub evidence_load: Option<EvidenceLoad>,
    /// 在途后台解码（一次一张；(身份, 请求代次) 匹配回执；迟到 / 断线
    /// 后的解码结果不落地）。
    pub evidence_decoding: Option<(EvidenceKey, u64)>,
    pub evidence_error: Option<String>,
    /// 停止状态机（面板单槽）。
    pub stop: Option<ComputerStop>,
    pub scroll: ScrollHandle,
}

impl Default for ComputerPanelState {
    fn default() -> Self {
        Self {
            session_id: None,
            request_id: 0,
            fetch: ComputerFetch::Idle,
            fetch_error: None,
            workspace_id: None,
            targets: Vec::new(),
            stale_reason: None,
            action_in_flight: None,
            action_error: None,
            observations: Vec::new(),
            obs_request_id: 0,
            obs_probe: false,
            obs_pages: 0,
            obs_previous_newest: None,
            obs_fetch: ComputerFetch::Idle,
            obs_error: None,
            obs_head: 0,
            obs_truncated: false,
            evidence_selected: None,
            evidence_images: std::collections::HashMap::new(),
            evidence_order: Vec::new(),
            evidence_evicted: Vec::new(),
            evidence_load: None,
            evidence_decoding: None,
            evidence_error: None,
            stop: None,
            scroll: ScrollHandle::new(),
        }
    }
}

impl ComputerPanelState {
    /// 开始一次列表查询；返回请求代次。换会话即复位（旧会话数据不留）。
    pub(super) fn begin_loading(&mut self, session_id: &str) -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        if self.session_id.as_deref() != Some(session_id) {
            let scroll = std::mem::take(&mut self.scroll);
            // 换会话复位：图像与 GPUI 解码 asset 一并移交清理队列
            //（AppView drain 时 remove_asset，防止全局缓存跨会话累积）。
            let mut evicted = std::mem::take(&mut self.evidence_evicted);
            evicted.extend(self.evidence_images.drain().map(|(_, image)| image));
            *self = Self::default();
            self.scroll = scroll;
            self.evidence_evicted = evicted;
        }
        self.session_id = Some(session_id.to_string());
        self.fetch = ComputerFetch::Fetching;
        self.fetch_error = None;
        self.stale_reason = None;
        self.request_id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.request_id
    }

    pub(super) fn apply_loaded(
        &mut self,
        session_id: &str,
        request_id: u64,
        data: ComputerTargetListData,
    ) -> bool {
        if self.session_id.as_deref() != Some(session_id) || self.request_id != request_id {
            return false;
        }
        self.fetch = ComputerFetch::Ready;
        self.fetch_error = None;
        self.stale_reason = None;
        self.workspace_id = data.workspace_id.clone();
        self.targets = data.targets.clone();
        // CU-19：列表是停止确认的权威观测（真实释放事实）。
        self.evaluate_stop_release();
        true
    }

    pub(super) fn apply_failed(&mut self, session_id: &str, request_id: u64, reason: String) {
        if self.session_id.as_deref() == Some(session_id) && self.request_id == request_id {
            self.fetch = ComputerFetch::Failed;
            self.fetch_error = Some(reason.clone());
            // 派发后观测失败：停止确认所依赖的列表查询失败（可重试）。
            if let Some(stop) = self.stop.as_mut() {
                match stop.phase {
                    ComputerStopPhase::Dispatching | ComputerStopPhase::Confirming { .. } => {
                        stop.phase = ComputerStopPhase::ObserveFailed { reason };
                    }
                    _ => {}
                }
            }
        }
    }

    /// 断线 / 不可查询收口：保留已加载数据，清在途标记（对应请求不会再
    /// 有回执），request_id 归零使迟到回执失效。
    pub(super) fn mark_stale(&mut self, reason: &str) {
        self.request_id = 0;
        if self.fetch == ComputerFetch::Fetching {
            self.fetch = if self.targets.is_empty() {
                ComputerFetch::Idle
            } else {
                ComputerFetch::Ready
            };
        }
        self.action_in_flight = None;
        self.stale_reason = Some(reason.to_string());
        // 证据：在途分块不会再有回执，放弃并归零代次；已加载数据保留。
        self.obs_request_id = 0;
        if self.obs_fetch == ComputerFetch::Fetching {
            self.obs_fetch = if self.observations.is_empty() {
                ComputerFetch::Idle
            } else {
                ComputerFetch::Ready
            };
        }
        self.evidence_load = None;
        // 在途解码同理：结果按代次落地，断线后归零即不匹配、不落地。
        self.evidence_decoding = None;
        // 停止：确认前断线即结果未知（已确认的 Stopped 是既成事实，保留）。
        if let Some(stop) = self.stop.as_mut() {
            match stop.phase {
                ComputerStopPhase::Dispatching | ComputerStopPhase::Confirming { .. } => {
                    stop.phase = ComputerStopPhase::Unknown;
                }
                _ => {}
            }
        }
    }

    /// approve / revoke 回执落地：仅（会话 + 派出代次）双匹配的回执收敛
    /// 状态——A→B→A 切换后旧 A 的迟到回执不清除新动作的在途标记或覆盖
    /// 错误（切会话经 begin_loading 复位、断线经 mark_stale 清在途，两条
    /// 路径都使旧代次失效）。Ok 清在途与错误（权威状态由紧随的重查
    /// 收敛）；Err 清在途并记录原因（面板内呈现，不冒充成功）。返回是否
    /// 匹配落地；未匹配时调用方不触发重查。
    pub(super) fn finish_action(
        &mut self,
        session_id: &str,
        action_id: u64,
        result: Result<(), String>,
    ) -> bool {
        if self.session_id.as_deref() != Some(session_id) {
            return false;
        }
        if self.action_in_flight.as_ref().map(|(_, id)| *id) != Some(action_id) {
            return false;
        }
        self.action_in_flight = None;
        match result {
            Ok(()) => self.action_error = None,
            Err(reason) => self.action_error = Some(reason),
        }
        true
    }

    /// 当前绑定是否即活动会话（断线重连后旧会话数据不冒充当前会话）。
    pub(super) fn matches_session(&self, session_id: Option<&str>) -> bool {
        self.session_id.is_some() && self.session_id.as_deref() == session_id
    }

    // ------------------------------------------------------------------
    // CU-19：观测证据（列表 + 分块图像读取）
    // ------------------------------------------------------------------

    /// 开始一次证据查询；返回请求代次。会话切换由 begin_loading 整体
    /// 复位；断线由 mark_stale 归零代次（迟到回执不落地）。
    pub(super) fn begin_observation_loading(&mut self) -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        self.obs_pages = 0;
        self.obs_previous_newest = self
            .observations
            .first()
            .map(|entry| (entry.sequence, entry.part_index));
        self.obs_fetch = ComputerFetch::Fetching;
        self.obs_error = None;
        self.obs_request_id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.obs_request_id
    }

    /// 记录本轮查询形态（探测页 / 数据页）供回执处理区分。
    pub(super) fn note_observation_dispatch(&mut self, probe: bool) {
        self.obs_probe = probe;
    }

    /// observation_list 回执落地。Host 分页按事件序推进：探测页（limit=1）
    /// 只习得当前 head（条目不落库——limit=1 页不代表尾窗），随后读
    /// [head-窗口, head] 尾窗；尾窗内被单页字节预算截断（complete=false
    /// 且 next_sequence=Some(游标)）时从游标同代次续读（页数有界，耗尽
    /// 即如实标截断）。落库页合并本环全部条目（新→旧），保持选中证据
    /// 身份（失效才回落最新）。返回续读查询参数；None = 本轮已收敛。
    pub(super) fn apply_observations(
        &mut self,
        session_id: &str,
        request_id: u64,
        data: &ObservationListData,
    ) -> Option<(Option<u64>, u32)> {
        if self.session_id.as_deref() != Some(session_id) || self.obs_request_id != request_id {
            return None;
        }
        // 探测页：丢弃条目，只取当前 head 锚定尾窗（head 可能已前进；
        // 旧 head 直接算窗会漏掉新事件）。
        if self.obs_probe {
            self.obs_probe = false;
            let after = data.head_sequence.saturating_sub(OBSERVATION_EVENT_WINDOW);
            // 尾窗外还有更早事件（after ≥ 1）即如实标注截断。
            self.obs_truncated = data.head_sequence > OBSERVATION_EVENT_WINDOW;
            return Some((Some(after), OBSERVATION_EVENT_WINDOW as u32));
        }
        let mut entries = data.entries.clone();
        // 数据页累积：尾窗首页整批替换，续读页追加（同一环请求）。
        if self.obs_pages == 0 {
            self.observations = entries;
        } else {
            self.observations.append(&mut entries);
        }
        if !data.complete {
            if let Some(cursor) = data.next_sequence {
                if self.obs_pages < OBSERVATION_MAX_LIST_PAGES {
                    self.obs_pages += 1;
                    return Some((Some(cursor), OBSERVATION_EVENT_WINDOW as u32));
                }
            }
        }
        // 收敛落库：续读上限耗尽或无游标的不完整页 → 已收条目即全部
        // 结果，截断如实标注（complete 页的截断标记已由探测页锚定）。
        if !data.complete {
            self.obs_truncated = true;
        }
        self.finalize_observations(data.head_sequence);
        None
    }

    /// 本轮查询收敛：排序（新→旧）、落 Ready、按需保持 / 回落 / 跟随
    /// 选中身份（起点为 begin 时钉住的上一轮最新，多页改写不干扰）。
    fn finalize_observations(&mut self, head: u64) {
        self.observations.sort_by(|a, b| {
            b.sequence
                .cmp(&a.sequence)
                .then(b.part_index.cmp(&a.part_index))
        });
        self.obs_fetch = ComputerFetch::Ready;
        self.obs_error = None;
        self.obs_head = head;
        let still_valid = self.evidence_selected.is_some_and(|key| {
            self.observations
                .iter()
                .any(|entry| (entry.sequence, entry.part_index) == key)
        });
        let newest = self
            .observations
            .first()
            .map(|entry| (entry.sequence, entry.part_index));
        // 新截图到达时跟随最新（用户未手动选旧条目）；手动选中的历史
        // 观测保持身份，不被刷新抢走。
        let follow_latest = self.obs_previous_newest.is_some_and(|previous| {
            self.evidence_selected == Some(previous) && self.obs_previous_newest != newest
        });
        if !still_valid || follow_latest {
            self.evidence_selected = newest;
            self.evidence_load = None;
            self.evidence_error = None;
        }
    }

    pub(super) fn apply_observations_failed(
        &mut self,
        session_id: &str,
        request_id: u64,
        reason: String,
    ) {
        if self.session_id.as_deref() == Some(session_id) && self.obs_request_id == request_id {
            self.obs_fetch = ComputerFetch::Failed;
            self.obs_error = Some(reason);
        }
    }

    /// observation_image_read 单块落地：校验（会话 × 代次 × 身份 × 偏移
    /// 连续 × 总量 ≤ 条目申报）后累计；eof 时魔数嗅探通过即收齐，字节
    /// 移交后台解码校验（像素 / 帧预算 + 真实解码不在 UI 线程执行）。
    /// 返回续读偏移 / 解码任务 / 终止；失败路径清在途并记录原因，不
    /// 冒充成功。
    pub(super) fn apply_observation_chunk(
        &mut self,
        session_id: &str,
        request_id: u64,
        data: &ObservationImageData,
    ) -> ObservationChunkOutcome {
        use base64::Engine as _;
        if self.session_id.as_deref() != Some(session_id) {
            return ObservationChunkOutcome::Settled;
        }
        let key: EvidenceKey = (data.sequence, data.part_index);
        if !self
            .evidence_load
            .as_ref()
            .is_some_and(|load| load.request_id == request_id && load.key == key)
        {
            return ObservationChunkOutcome::Settled;
        }
        let corrupt = |state: &mut Self| {
            state.evidence_load = None;
            state.evidence_error = Some(t("computer.observation_corrupt").to_string());
            ObservationChunkOutcome::Settled
        };
        let load = self.evidence_load.as_mut().expect("checked above");
        if data.offset != load.next_offset {
            return corrupt(self);
        }
        let bytes = match base64::engine::general_purpose::STANDARD.decode(data.data.as_bytes()) {
            Ok(bytes) => bytes,
            Err(_) => {
                return corrupt(self);
            }
        };
        load.buffer.extend_from_slice(&bytes);
        load.received = load.received.saturating_add(bytes.len() as u64);
        load.next_offset = data.offset.saturating_add(bytes.len() as u64);
        if load.received > load.total {
            return corrupt(self);
        }
        let next_offset = load.next_offset;
        if data.eof {
            let finished = self.evidence_load.take().expect("checked above");
            if finished.received != finished.total || finished.total == 0 {
                self.evidence_error = Some(t("computer.observation_corrupt").to_string());
                return ObservationChunkOutcome::Settled;
            }
            let format = match super::attachments::sniff_image_format(&finished.buffer) {
                Some(format) => format,
                None => {
                    self.evidence_error = Some(t("computer.observation_corrupt").to_string());
                    return ObservationChunkOutcome::Settled;
                }
            };
            // 收齐：登记在途解码代次（回执按 (身份, 代次) 落地），字节
            // 移交后台执行器做真实解码校验（预算 + 全帧解码）。
            self.evidence_decoding = Some((finished.key, finished.request_id));
            return ObservationChunkOutcome::Decode(EvidenceDecode {
                key: finished.key,
                request_id: finished.request_id,
                format,
                bytes: finished.buffer,
            });
        }
        ObservationChunkOutcome::Continue(key, next_offset)
    }

    /// 后台解码成功回执：按（会话 × 代次 × 身份）匹配落地，重新嗅探
    /// 后包装 gpui::Image 入有界缓存（from_bytes 只包装不解码，渲染期
    /// 才解码——校验已保证可解码）。缓存按身份隔离，不污染当前选择；
    /// 迟到 / 换会话 / 断线后不匹配丢弃。嗅探失败错误只落到选中身份。
    pub(super) fn apply_observation_decoded(
        &mut self,
        session_id: &str,
        request_id: u64,
        key: EvidenceKey,
        bytes: Vec<u8>,
    ) -> bool {
        if self.session_id.as_deref() != Some(session_id)
            || self.evidence_decoding != Some((key, request_id))
        {
            return false;
        }
        self.evidence_decoding = None;
        match super::attachments::sniff_image_format(&bytes) {
            Some(format) => {
                let image = std::sync::Arc::new(gpui::Image::from_bytes(format, bytes));
                self.cache_evidence_image(key, image);
            }
            None => {
                if self.evidence_selected == Some(key) {
                    self.evidence_error = Some(t("computer.observation_corrupt").to_string());
                }
            }
        }
        true
    }

    /// 后台解码失败回执（预算超限 / 解码失败）：同样按代次落地；错误
    /// 按身份隔离——解码期间已切到别的证据时，旧身份的失败不落到
    /// 当前选择（回原身份经重试入口重新加载；预算类失败重试结果
    /// 不变）。
    pub(super) fn apply_observation_decode_failed(
        &mut self,
        session_id: &str,
        request_id: u64,
        key: EvidenceKey,
        reason: String,
    ) -> bool {
        if self.session_id.as_deref() != Some(session_id)
            || self.evidence_decoding != Some((key, request_id))
        {
            return false;
        }
        self.evidence_decoding = None;
        if self.evidence_selected == Some(key) {
            self.evidence_error = Some(reason);
        }
        true
    }

    pub(super) fn apply_observation_image_failed(
        &mut self,
        session_id: &str,
        request_id: u64,
        reason: String,
    ) {
        if self.session_id.as_deref() != Some(session_id) {
            return;
        }
        if self
            .evidence_load
            .as_ref()
            .is_some_and(|load| load.request_id == request_id)
        {
            self.evidence_load = None;
            self.evidence_error = Some(reason);
        }
    }

    /// 有界缓存：超出上限按插入序淘汰（截图只在内存，不写仓库）；
    /// 淘汰连同 GPUI 全局 asset（解码结果）一并移交清理队列，由
    /// AppView drain 时 remove_asset（本地 Arc 丢弃不清全局缓存）。
    fn cache_evidence_image(&mut self, key: EvidenceKey, image: std::sync::Arc<gpui::Image>) {
        if !self.evidence_order.contains(&key) {
            self.evidence_order.push(key);
        }
        self.evidence_images.insert(key, image);
        while self.evidence_order.len() > OBSERVATION_IMAGE_CACHE {
            let evicted = self.evidence_order.remove(0);
            if let Some(image) = self.evidence_images.remove(&evicted) {
                self.evidence_evicted.push(image);
            }
        }
    }

    // ------------------------------------------------------------------
    // CU-19：停止状态机
    // ------------------------------------------------------------------

    /// 是否有停止在途（派发或等待确认）——授权 / 撤销按钮共用 busy。
    pub(super) fn stop_pending(&self) -> bool {
        self.stop.as_ref().is_some_and(|stop| {
            matches!(
                stop.phase,
                ComputerStopPhase::Dispatching | ComputerStopPhase::Confirming { .. }
            )
        })
    }

    /// run_cancel 回执落地：仅（会话 + 派出代次 + run）匹配。Ok / Err
    /// 都只进入确认（unknown_run 等拒绝可能是已终态，终态事件会另行
    /// 收敛）；Err 的 note 在「仍被持有」时回落为操作错误。Stopped 是
    /// 终态：终态事件先到时迟到回执不得改写或重新触发确认。返回是否
    /// 匹配（调用方触发重查）。
    pub(super) fn finish_stop(
        &mut self,
        session_id: &str,
        action_id: u64,
        run_id: &str,
        result: Result<(), String>,
    ) -> bool {
        if self.session_id.as_deref() != Some(session_id) {
            return false;
        }
        let Some(stop) = self.stop.as_mut() else {
            return false;
        };
        if stop.action_id != action_id || stop.run_id != run_id {
            return false;
        }
        if stop.phase == ComputerStopPhase::Stopped {
            return false;
        }
        stop.phase = ComputerStopPhase::Confirming { note: result.err() };
        true
    }

    /// run 终态事件到达：以真实终态确认已停止（唯一确认通道——Host
    /// 取消即从 registry 移除 run、目标列表随之为空，但清理仍在执行，
    /// 空列表不能证明已释放）。
    pub(super) fn confirm_stop_terminal(&mut self, run_id: &str) {
        let Some(stop) = self.stop.as_mut() else {
            return;
        };
        if stop.run_id != run_id {
            return;
        }
        match stop.phase {
            ComputerStopPhase::Dispatching
            | ComputerStopPhase::Confirming { .. }
            | ComputerStopPhase::Unknown
            | ComputerStopPhase::ObserveFailed { .. } => {
                stop.phase = ComputerStopPhase::Stopped;
            }
            ComputerStopPhase::Stopped => {}
        }
    }

    /// 以重查列表核对停止：列表不裁决「已停止」（Host 取消即移除
    /// registry 登记，空列表时清理可能仍在执行）；只处理两件事——
    /// 确认中被拒且仍被持有 → 回落错误并允许重试；已停止记录在新
    /// run 接管面板时退役。
    fn evaluate_stop_release(&mut self) {
        let Some(stop) = self.stop.clone() else {
            return;
        };
        let held = self.targets.iter().any(|state| {
            state.scope.run_id == stop.run_id
                || state
                    .occupied_by
                    .as_ref()
                    .is_some_and(|owner| owner.run_id == stop.run_id)
        });
        match stop.phase {
            ComputerStopPhase::Dispatching
            | ComputerStopPhase::Confirming { .. }
            | ComputerStopPhase::Unknown
            | ComputerStopPhase::ObserveFailed { .. } => {
                if held {
                    if let ComputerStopPhase::Confirming { note: Some(reason) } = stop.phase {
                        self.action_error = Some(reason);
                        self.stop = None;
                    }
                }
            }
            ComputerStopPhase::Stopped => {
                if self.targets.iter().any(|state| state.scope.run_id != stop.run_id) {
                    self.stop = None;
                }
            }
        }
    }
}

/// 目标行身份键：run scope + 目标锚点（同 run 同目标唯一；跨 run 同名
/// 目标不撞键）。仅用于 UI 键 / AX id，不上 wire。
pub(super) fn target_key(state: &ComputerTargetState) -> String {
    let anchor = match &state.target {
        ComputerTargetIdentity::Application { app } => format!("app:{}", app.bundle_id),
        ComputerTargetIdentity::Website { browser, origin } => {
            format!("site:{}:{}", browser.bundle_id, origin)
        }
    };
    format!("{}:{}", state.scope.run_id, anchor)
}

/// 动作 id（dynamic_identifier 转义规则与 dispatch 反查同源）。
fn action_id(prefix: &str, key: &str) -> String {
    dynamic_identifier(prefix.trim_end_matches('-'), key)
}

/// 动作派出代次：进程级 static 计数器（AGENTS.md 工程经验：计数器
/// 必须进程级，不依赖时钟分辨率兜底唯一性）。只用于本地在途匹配，
/// 不上 wire。
fn next_computer_action_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// 证据分块读取的请求代次（进程级 static，与动作代次同口径）。
fn next_evidence_request_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// 撤销按钮可用判定（渲染与 AX 同源）：不要求仍存在 grant——一次授权
/// 消耗后 grant 即消失，但派发许可与占用可能仍在；revoke 即目标级停止，
/// 必须始终可达。
fn computer_revoke_enabled(can_act: bool, busy: bool) -> bool {
    can_act && !busy
}

/// 由动作 id 反查目标（dispatch 只拿到完整 identifier；重算同源 id
/// 比较，不做反向解码）。
pub(super) fn find_target_for_action<'a>(
    targets: &'a [ComputerTargetState],
    prefix: &str,
    identifier: &str,
) -> Option<&'a ComputerTargetState> {
    targets
        .iter()
        .find(|state| action_id(prefix, &target_key(state)) == identifier)
}

/// 证据行 id（dynamic_identifier 转义规则与 dispatch 反查同源）。
fn observation_row_id(key: EvidenceKey) -> String {
    dynamic_identifier("computer-observation", &format!("{}:{}", key.0, key.1))
}

/// 当前 / 历史 chip（字形语义 + 颜色双通道；「当前」= 最新一条观测，
/// 其余为历史——wire 恒 historical，区分由客户端按新近度表达）。
fn observation_chip(is_current: bool) -> gpui::Div {
    let (label, color) = if is_current {
        (t("computer.observation_current"), dark().accent.primary)
    } else {
        (t("computer.observation_history"), dark().text.tertiary)
    };
    div()
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(color)
        .text_size(font::XS)
        .text_color(color)
        .child(label)
}

/// 证据副行归属：只显示观测条目自带的 run 标识。观测证据不携带目标
/// 锚点，按 run 与目标卡对齐是冒充归属（同 run 目标会增删 / 换锚点），
/// 不做匹配。
fn observation_target_label(entry: &ObservationEvidenceEntry) -> String {
    entry
        .run_id
        .as_ref()
        .map(|run| t("computer.observation_run").replace("{}", run))
        .unwrap_or_default()
}

/// 魔数嗅探格式 → image crate 解码格式（后台解码校验用；嗅探只会
/// 返回这四种，无 Svg 分支）。
fn decode_image_format(format: gpui::ImageFormat) -> image::ImageFormat {
    match format {
        gpui::ImageFormat::Png => image::ImageFormat::Png,
        gpui::ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        gpui::ImageFormat::Gif => image::ImageFormat::Gif,
        gpui::ImageFormat::Webp => image::ImageFormat::WebP,
        _ => image::ImageFormat::Png,
    }
}

/// 后台解码校验（CPU 密集，只在后台执行器运行，绝不进 UI 线程）：
/// 压缩字节预算约束不了解码内存，按实际尺寸先查单帧像素，再真实
/// 解码全部帧——GIF 只解首帧会漏「首帧有效、后续帧损坏」的坏图
///（GPUI 渲染时解码全部帧，校验口径必须一致），帧数与累计像素
/// 单独设预算。返回原字节供 UI 线程包装 gpui::Image。
fn validate_observation_image(
    format: gpui::ImageFormat,
    bytes: Vec<u8>,
) -> Result<Vec<u8>, ObservationDecodeError> {
    use image::{AnimationDecoder as _, ImageDecoder as _};

    if decode_image_format(format) == image::ImageFormat::Gif {
        let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&bytes))
            .map_err(|_| ObservationDecodeError::Decode)?;
        let (width, height) = decoder.dimensions();
        let frame_pixels = u64::from(width) * u64::from(height);
        if frame_pixels > OBSERVATION_IMAGE_MAX_PIXELS {
            return Err(ObservationDecodeError::Budget);
        }
        let mut frames = decoder.into_frames();
        let mut count: u32 = 0;
        let mut total: u64 = 0;
        while let Some(frame) = frames.next() {
            frame.map_err(|_| ObservationDecodeError::Decode)?;
            count = count.saturating_add(1);
            if count > OBSERVATION_IMAGE_MAX_FRAMES {
                return Err(ObservationDecodeError::Budget);
            }
            total = total.saturating_add(frame_pixels);
            if total > OBSERVATION_IMAGE_MAX_TOTAL_PIXELS {
                return Err(ObservationDecodeError::Budget);
            }
        }
        if count == 0 {
            return Err(ObservationDecodeError::Decode);
        }
    } else {
        let image_format = decode_image_format(format);
        // 先读头部尺寸（不解码），超预算的大尺寸图直接拒绝。
        let reader =
            image::ImageReader::with_format(std::io::Cursor::new(&bytes), image_format);
        let decoder = reader
            .into_decoder()
            .map_err(|_| ObservationDecodeError::Decode)?;
        let (width, height) = decoder.dimensions();
        if u64::from(width) * u64::from(height) > OBSERVATION_IMAGE_MAX_PIXELS {
            return Err(ObservationDecodeError::Budget);
        }
        drop(decoder);
        // 预算内再完整解码一次（静图即单帧），坏图 fail-closed。
        image::load_from_memory_with_format(&bytes, image_format)
            .map_err(|_| ObservationDecodeError::Decode)?;
    }
    Ok(bytes)
}

/// 预览元信息：run · 像素尺寸 · 字节量 · 工具。
fn observation_meta(entry: &ObservationEvidenceEntry) -> String {
    let mut parts = vec![observation_target_label(entry)];
    if let (Some(width), Some(height)) = (entry.image_width, entry.image_height) {
        parts.push(format!("{width}×{height}"));
    }
    parts.push(super::attachments::format_byte_size(
        entry.byte_length as usize,
    ));
    if let Some(tool) = &entry.tool_name {
        parts.push(tool.clone());
    }
    parts.join(" · ")
}

/// 分块加载进度文案（已收 / 总量）。
fn observation_progress(received: u64, total: u64) -> String {
    t("computer.observation_loading")
        .replacen("{}", &super::attachments::format_byte_size(received as usize), 1)
        .replacen("{}", &super::attachments::format_byte_size(total as usize), 1)
}

/// 停止状态条文案（渲染与 AX 同源）：只描述已确认的事实——派发 /
/// 等待释放确认 / 已停止（释放或终态确认）/ 确认失败 / 结果未知。
fn computer_stop_status(stop: &Option<ComputerStop>) -> Option<(String, gpui::Rgba)> {
    let stop = stop.as_ref()?;
    let run = t("computer.stop_run").replace("{}", &stop.run_id);
    match &stop.phase {
        ComputerStopPhase::Dispatching => Some((
            format!("{} · {run}", t("computer.stop_dispatching")),
            dark().semantic.warning_text,
        )),
        ComputerStopPhase::Confirming { note: None } => Some((
            format!("{} · {run}", t("computer.stop_confirming")),
            dark().semantic.warning_text,
        )),
        ComputerStopPhase::Confirming { note: Some(reason) } => Some((
            format!(
                "{} · {run}",
                t("computer.stop_confirming_note").replace("{}", reason)
            ),
            dark().semantic.warning_text,
        )),
        ComputerStopPhase::Stopped => Some((
            format!("{} · {run}", t("computer.stop_stopped")),
            dark().semantic.success_fg,
        )),
        ComputerStopPhase::ObserveFailed { reason } => Some((
            format!(
                "{} · {run}",
                t("computer.stop_observe_failed").replace("{}", reason)
            ),
            dark().semantic.danger_text,
        )),
        ComputerStopPhase::Unknown => Some((
            format!("{} · {run}", t("computer.stop_unknown")),
            dark().semantic.warning_text,
        )),
    }
}

/// 目标标题（数据原文，不翻译）：应用显示 bundle id，网站显示 origin。
fn target_title(state: &ComputerTargetState) -> String {
    match &state.target {
        ComputerTargetIdentity::Application { app } => app.bundle_id.clone(),
        ComputerTargetIdentity::Website { origin, .. } => origin.clone(),
    }
}

/// 目标副行：本地化族名（网站目标附浏览器 bundle id 原文）。
fn target_subtitle(state: &ComputerTargetState) -> String {
    let app = match &state.target {
        ComputerTargetIdentity::Application { app } => app,
        ComputerTargetIdentity::Website { browser, .. } => browser,
    };
    let family = match app.family {
        ComputerAppFamily::Appkit => t("computer.family_appkit"),
        ComputerAppFamily::Chromium => t("computer.family_chromium"),
        ComputerAppFamily::Browser => t("computer.family_browser"),
    };
    match &state.target {
        ComputerTargetIdentity::Application { .. } => family.to_string(),
        ComputerTargetIdentity::Website { browser, .. } => format!(
            "{} · {}",
            t("computer.website_in").replace("{}", &browser.bundle_id),
            family
        ),
    }
}

/// 授权状态词（render 与 AX 同源）。
fn grant_label(grant: Option<ComputerGrantKind>) -> &'static str {
    match grant {
        Some(ComputerGrantKind::ForRun) => t("computer.grant_for_run"),
        Some(ComputerGrantKind::Once) => t("computer.grant_once"),
        None => t("computer.grant_none"),
    }
}

fn grant_color(grant: Option<ComputerGrantKind>) -> gpui::Rgba {
    match grant {
        Some(ComputerGrantKind::ForRun) => dark().semantic.success_fg,
        Some(ComputerGrantKind::Once) => dark().accent.primary,
        None => dark().semantic.warning_text,
    }
}

/// 目标状态摘要行（grant + 绑定 / 占用 / 暂停事实；数据段不翻译）。
fn target_state_text(state: &ComputerTargetState) -> String {
    let mut parts = vec![grant_label(state.grant).to_string()];
    if let Some(handle) = &state.bound_window {
        parts.push(format!("{} · {handle}", t("computer.bound_window")));
    }
    if state
        .occupied_by
        .as_ref()
        .is_some_and(|owner| *owner != state.scope)
    {
        parts.push(t("computer.occupied_other").to_string());
    }
    if state.dispatch_paused {
        parts.push(t("computer.paused").to_string());
    }
    parts.join(" · ")
}

fn action_name(action: ComputerActionKind) -> &'static str {
    t(match action {
        ComputerActionKind::CaptureWindow => "computer.action_capture_window",
        ComputerActionKind::AxRead => "computer.action_ax_read",
        ComputerActionKind::AxSemanticWrite => "computer.action_ax_semantic_write",
        ComputerActionKind::AxPress => "computer.action_ax_press",
        ComputerActionKind::TargetedKeys => "computer.action_targeted_keys",
        ComputerActionKind::MenuShortcut => "computer.action_menu_shortcut",
        ComputerActionKind::MenubarPress => "computer.action_menubar_press",
        ComputerActionKind::Pointer => "computer.action_pointer",
        ComputerActionKind::WindowMove => "computer.action_window_move",
        ComputerActionKind::WindowResize => "computer.action_window_resize",
    })
}

fn support_label(support: ComputerCapabilitySupport) -> &'static str {
    t(match support {
        ComputerCapabilitySupport::Supported => "computer.cap_supported",
        ComputerCapabilitySupport::Unverified => "computer.cap_unverified",
        ComputerCapabilitySupport::Unsupported => "computer.cap_unsupported",
    })
}

/// 能力 chip 字形与颜色（不靠颜色单独表达状态：✓/?/× 前缀同源进 AX）。
fn support_glyph(support: ComputerCapabilitySupport) -> (&'static str, gpui::Rgba) {
    match support {
        ComputerCapabilitySupport::Supported => ("✓", dark().semantic.success_fg),
        ComputerCapabilitySupport::Unverified => ("?", dark().semantic.warning_text),
        ComputerCapabilitySupport::Unsupported => ("×", dark().text.tertiary),
    }
}

/// AX description 的完整能力明细（十动作 × background / minimized）。
fn capability_detail(caps: &[ComputerTargetCapability]) -> String {
    caps.iter()
        .map(|cap| {
            format!(
                "{}: {} / {}",
                action_name(cap.action),
                support_label(cap.background),
                support_label(cap.minimized),
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// 是否存在未验证能力（驱动系统权限指引行）。
fn has_unverified(caps: &[ComputerTargetCapability]) -> bool {
    caps.iter().any(|cap| {
        cap.background == ComputerCapabilitySupport::Unverified
            || cap.minimized == ComputerCapabilitySupport::Unverified
    })
}

/// 最小化态说明：与后台一致时不占行；收窄时列出仍支持的动作。
fn minimized_note(caps: &[ComputerTargetCapability]) -> Option<String> {
    let background: Vec<ComputerActionKind> = caps
        .iter()
        .filter(|cap| cap.background == ComputerCapabilitySupport::Supported)
        .map(|cap| cap.action)
        .collect();
    let minimized: Vec<ComputerActionKind> = caps
        .iter()
        .filter(|cap| cap.minimized == ComputerCapabilitySupport::Supported)
        .map(|cap| cap.action)
        .collect();
    if minimized == background {
        return None;
    }
    if minimized.is_empty() {
        return Some(t("computer.minimized_none").to_string());
    }
    let names = minimized
        .iter()
        .map(|action| action_name(*action))
        .collect::<Vec<_>>()
        .join(", ");
    Some(t("computer.minimized_some").replace("{}", &names))
}

impl AppView {
    /// 授权 / 撤销动作 gate：连接 × Host 支持 × 活动 run × 面板绑定当前
    /// 会话 × 非查询失败态 × 无在途动作。渲染 disabled 与 AX enabled 同源；
    /// 鼠标 / 键盘 / AX 三通道都经此 gate，不存在自动批准路径；失败占位
    /// 期间（按钮已隐藏）AX Press 也不得派出。
    pub(super) fn computer_can_act(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.controller.supports_computer_targets()
            && self.projection.active_run_id.is_some()
            && self
                .computer
                .matches_session(self.projection.active_session_id.as_deref())
            && self.computer.fetch != ComputerFetch::Failed
            && self.computer.action_in_flight.is_none()
            && !self.computer.stop_pending()
    }

    /// 目标列表可见条件（渲染与 AX 同源）：连接 × 有会话 × 面板已绑定
    /// 该会话 × 非失败态 × 持有数据。失败占位与列表互斥——AX 不得凭
    /// 保留数据发布视觉界面已隐藏的行与按钮。
    pub(super) fn computer_targets_visible(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.active_session_id.is_some()
            && self
                .computer
                .matches_session(self.projection.active_session_id.as_deref())
            && self.computer.fetch != ComputerFetch::Failed
            && !self.computer.targets.is_empty()
    }

    pub(super) fn computer_can_refresh(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.controller.supports_computer_targets()
            && self.projection.active_session_id.is_some()
            && self.computer.fetch != ComputerFetch::Fetching
    }

    /// 拉取 / 重查目标列表（打开页签、切会话、授权回执收敛、手动刷新
    /// 共用）。断线保留旧数据并标记 stale；旧 Host 由 version_required
    /// 占位如实呈现，不派出查询。
    pub(super) fn refresh_computer_targets(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.projection.active_session_id.clone() else {
            return;
        };
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            self.computer.mark_stale("not connected");
            cx.notify();
            return;
        }
        if !self.controller.supports_computer_targets() {
            cx.notify();
            return;
        }
        let request_id = self.computer.begin_loading(&session_id);
        // 换会话复位会把旧图像全部移交清理队列：随即 drain 掉对应
        // GPUI 全局 asset（不等下一次图像回执）。
        self.computer_drain_evicted_evidence(cx);
        if !self.controller.computer_target_list(session_id, request_id) {
            self.computer.mark_stale("computer target list unavailable");
        }
        // CU-19：同批刷新观测证据（同一面板绑定与版本门）。
        self.refresh_computer_observations(cx);
        cx.notify();
    }

    /// 拉取 / 重查观测证据（有界）：恒以 limit=1 探测页锚定当前 head
    ///（head 可能已前进，按旧 head 直接算窗会漏新事件；探测页条目与
    /// 游标不落库），回执处理按分页语义续读尾窗。断线 / 旧 Host 不
    /// 派出，保留旧数据（stale 由 mark_stale 统一标注）。
    fn refresh_computer_observations(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.computer_bound_session() else {
            return;
        };
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) || !self.controller.supports_computer_targets()
        {
            return;
        }
        let request_id = self.computer.begin_observation_loading();
        self.computer.note_observation_dispatch(true);
        if !self
            .controller
            .observation_list(session_id, request_id, None, Some(1))
        {
            self.computer.obs_fetch = ComputerFetch::Idle;
        }
        cx.notify();
    }

    /// 移交队列中的淘汰图像：调用 gpui remove_asset 清理全局解码缓存
    ///（丢弃本地 Arc 不清全局缓存；面板状态层无 App 上下文，只能入队
    /// 由 AppView 在此 drain）。空队列为 no-op。
    pub(super) fn computer_drain_evicted_evidence(&mut self, cx: &mut Context<Self>) {
        for image in std::mem::take(&mut self.computer.evidence_evicted) {
            image.remove_asset(&mut *cx);
        }
    }

    /// 面板绑定的会话（仅当它就是当前活动会话）——授权命令只发向当前
    /// 会话，旧会话数据不冒充。
    fn computer_bound_session(&self) -> Option<String> {
        let active = self.projection.active_session_id.as_deref()?;
        self.computer
            .matches_session(Some(active))
            .then(|| active.to_string())
    }

    /// 派出成功：登记在途（目标 key, 派出代次）（一次一个，防双击重复
    /// 派出）；派出失败（断线 / 旧 Host 竞态）：记录通用不可用原因
    ///（本地事实，不伪造 wire 错误），代次废弃。
    fn note_computer_action_dispatched(
        &mut self,
        key: &str,
        action_id: u64,
        dispatched: bool,
        cx: &mut Context<Self>,
    ) {
        if dispatched {
            self.computer.action_in_flight = Some((key.to_string(), action_id));
            self.computer.action_error = None;
        } else {
            self.computer.action_error = Some(t("computer.action_unavailable").to_string());
        }
        cx.notify();
    }

    fn computer_grant(&mut self, key: &str, kind: ComputerGrantKind, cx: &mut Context<Self>) {
        if !self.computer_can_act() {
            return;
        }
        let Some((run_id, target)) = self
            .computer
            .targets
            .iter()
            .find(|state| target_key(state) == key)
            .map(|state| (state.scope.run_id.clone(), state.target.clone()))
        else {
            return;
        };
        let Some(session_id) = self.computer_bound_session() else {
            return;
        };
        let action_id = next_computer_action_id();
        let dispatched =
            self.controller
                .computer_target_approve(session_id, action_id, run_id, target, kind);
        self.note_computer_action_dispatched(key, action_id, dispatched, cx);
    }

    fn computer_revoke(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.computer_can_act() {
            return;
        }
        let Some((run_id, target)) = self
            .computer
            .targets
            .iter()
            .find(|state| target_key(state) == key)
            .map(|state| (state.scope.run_id.clone(), state.target.clone()))
        else {
            return;
        };
        let Some(session_id) = self.computer_bound_session() else {
            return;
        };
        let action_id = next_computer_action_id();
        let dispatched =
            self.controller
                .computer_target_revoke(session_id, action_id, run_id, target);
        self.note_computer_action_dispatched(key, action_id, dispatched, cx);
    }

    /// 按钮共用入口：前缀决定动作（once / for_run / revoke）。
    fn computer_action_for_prefix(&mut self, prefix: &str, key: &str, cx: &mut Context<Self>) {
        match prefix {
            COMPUTER_APPROVE_ONCE_PREFIX => self.computer_grant(key, ComputerGrantKind::Once, cx),
            COMPUTER_APPROVE_RUN_PREFIX => self.computer_grant(key, ComputerGrantKind::ForRun, cx),
            COMPUTER_STOP_PREFIX => self.computer_stop(key, cx),
            _ => self.computer_revoke(key, cx),
        }
    }

    /// AX dispatch 入口：按三个动作前缀反查目标后执行（渲染与 AX 同源
    /// id，不反向解码 key）；找不到目标即列表已刷新，不动作。
    pub(super) fn computer_action_by_id(&mut self, identifier: &str, cx: &mut Context<Self>) {
        for (prefix, kind) in [
            (COMPUTER_APPROVE_ONCE_PREFIX, Some(ComputerGrantKind::Once)),
            (COMPUTER_APPROVE_RUN_PREFIX, Some(ComputerGrantKind::ForRun)),
            (COMPUTER_REVOKE_PREFIX, None),
            (COMPUTER_STOP_PREFIX, None),
        ] {
            if !identifier.starts_with(prefix) {
                continue;
            }
            let Some(key) = find_target_for_action(&self.computer.targets, prefix, identifier)
                .map(target_key)
            else {
                return;
            };
            match kind {
                Some(kind) => self.computer_grant(&key, kind, cx),
                None if prefix == COMPUTER_REVOKE_PREFIX => self.computer_revoke(&key, cx),
                None => self.computer_stop(&key, cx),
            }
            return;
        }
    }

    /// AX dispatch：观测行按同源 id 反查（重算比较，不反向解码）；
    /// 查看 / 重试作用于当前选中证据。找不到即列表已刷新，不动作
    ///（fail-closed；enabled 复核在 AX 层与入口 gate 双保险）。
    pub(super) fn computer_observation_ax_action(
        &mut self,
        identifier: &str,
        cx: &mut Context<Self>,
    ) {
        if identifier == "computer-observation-view" || identifier == "computer-observation-retry" {
            if let Some(key) = self.computer.evidence_selected {
                self.computer_retry_evidence(key, cx);
            }
            return;
        }
        let found = self
            .computer
            .observations
            .iter()
            .map(|entry| (entry.sequence, entry.part_index))
            .find(|key| observation_row_id(*key) == identifier);
        if let Some(key) = found {
            self.computer_select_observation(key, cx);
        }
    }

    /// 停止后台任务（CU-19）：只取消属于当前会话活动 run 的目标卡；
    /// run_cancel 经 Host 执行，GUI 关闭不取消 Core Run（既有合同）。
    /// 点击只进入「停止中」，已停止由释放 / 终态确认，不冒充成功。
    fn computer_stop(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.computer_can_act() {
            return;
        }
        let Some(run_id) = self
            .computer
            .targets
            .iter()
            .find(|state| target_key(state) == key)
            .map(|state| state.scope.run_id.clone())
        else {
            return;
        };
        if self.projection.active_run_id.as_deref() != Some(run_id.as_str()) {
            return;
        }
        let Some(session_id) = self.computer_bound_session() else {
            return;
        };
        let action_id = next_computer_action_id();
        let dispatched = self
            .controller
            .computer_stop_run(session_id, action_id, run_id.clone());
        if dispatched {
            self.computer.stop = Some(ComputerStop {
                key: key.to_string(),
                run_id,
                action_id,
                phase: ComputerStopPhase::Dispatching,
            });
            self.computer.action_error = None;
        } else {
            self.computer.action_error = Some(t("computer.action_unavailable").to_string());
        }
        cx.notify();
    }

    /// 停止按钮可用判定（渲染与 AX 同源）：授权 gate + 无在途授权 +
    /// 该卡的 run 就是当前活动 run（其它 run 的目标不经本面板取消）。
    fn computer_stop_button_enabled(&self, can_act: bool, card_run: &str) -> bool {
        can_act
            && self.computer.action_in_flight.is_none()
            && self.projection.active_run_id.as_deref() == Some(card_run)
    }

    /// 选中一条观测证据并按需开始加载（换目标即放弃在途读取，迟到块
    /// 不落地）；键盘与鼠标共用入口。
    fn computer_select_observation(&mut self, key: EvidenceKey, cx: &mut Context<Self>) {
        if !self.computer_targets_visible() && !self.computer_observations_visible() {
            return;
        }
        self.computer.evidence_selected = Some(key);
        self.computer.evidence_error = None;
        self.computer.evidence_load = None;
        self.computer_start_evidence_load(cx);
    }

    /// 开始（或继续）加载选中证据：已缓存 / 在途 / 已失败不重复派出；
    /// 单块上限 MAX_OBSERVATION_READ_BYTES，整图以条目申报字节为界。
    pub(super) fn computer_start_evidence_load(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.computer.evidence_selected else {
            return;
        };
        if self.computer.evidence_images.contains_key(&key)
            || self.computer.evidence_load.is_some()
            || self.computer.evidence_decoding.is_some()
            || self.computer.evidence_error.is_some()
        {
            return;
        }
        let Some(entry) = self
            .computer
            .observations
            .iter()
            .find(|entry| (entry.sequence, entry.part_index) == key)
            .cloned()
        else {
            return;
        };
        // 整图字节预算：超限不派出读取（解码像素内存远超字节量；条目
        // 仍在列表可见，仅预览 fail-closed）。
        if entry.byte_length > OBSERVATION_IMAGE_MAX_BYTES {
            self.computer.evidence_error = Some(t("computer.observation_too_large").to_string());
            cx.notify();
            return;
        }
        let Some(session_id) = self.computer_bound_session() else {
            return;
        };
        let dispatchable = matches!(self.projection.connection, ConnectionState::Connected { .. })
            && self.controller.supports_computer_targets();
        let request_id = next_evidence_request_id();
        if dispatchable
            && self.controller.observation_image_read(
            session_id,
            request_id,
            key.0,
            key.1,
            0,
            MAX_OBSERVATION_READ_BYTES,
        ) {
            self.computer.evidence_load = Some(EvidenceLoad {
                key,
                request_id,
                next_offset: 0,
                received: 0,
                total: entry.byte_length,
                buffer: Vec::new(),
            });
        } else {
            // 断线 / 旧 Host / 派出竞态：如实记录不可加载（本地事实，
            // 不伪造 wire 错误），可经重试入口再次尝试。
            self.computer.evidence_error = Some(t("computer.action_unavailable").to_string());
        }
        cx.notify();
    }

    /// 收齐的证据字节交后台执行器做真实解码校验（像素 / 帧数 / 累计
    /// 像素预算在 CPU 线程裁决，不阻塞 UI 线程），结果回 UI 线程按
    /// （会话 × 请求代次 × 证据身份）落地；断线 / 换会话后的迟到结果
    /// 不匹配当前解码代次，直接丢弃。
    pub(super) fn computer_dispatch_evidence_decode(
        &mut self,
        session_id: String,
        task: EvidenceDecode,
        cx: &mut Context<Self>,
    ) {
        let EvidenceDecode {
            key,
            request_id,
            format,
            bytes,
        } = task;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { validate_observation_image(format, bytes) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.computer_finish_evidence_decode(&session_id, request_id, key, result, cx);
            });
        })
        .detach();
    }

    /// 后台解码回执落地（成功 / 失败共用）：成功按身份入缓存（与当前
    /// 选择无关），失败错误只落到选中身份；回执落地后补启动当前选中
    /// 项的加载——解码占位期间选中 / 刷新跟随到的新证据此前被单槽
    /// 在途挡住，此处续载（已缓存 / 已出错的选择在 gate 处自然短路）。
    /// 回执未落地（迟到 / 断线 / 换会话）不续载，不制造新状态。
    pub(super) fn computer_finish_evidence_decode(
        &mut self,
        session_id: &str,
        request_id: u64,
        key: EvidenceKey,
        result: Result<Vec<u8>, ObservationDecodeError>,
        cx: &mut Context<Self>,
    ) {
        let applied = match result {
            Ok(bytes) => {
                let applied = self
                    .computer
                    .apply_observation_decoded(session_id, request_id, key, bytes);
                self.computer_drain_evicted_evidence(cx);
                applied
            }
            Err(kind) => {
                let reason = match kind {
                    ObservationDecodeError::Budget => t("computer.observation_too_large").to_string(),
                    ObservationDecodeError::Decode => t("computer.observation_corrupt").to_string(),
                };
                self.computer
                    .apply_observation_decode_failed(session_id, request_id, key, reason)
            }
        };
        if applied {
            self.computer_start_evidence_load(cx);
        }
        cx.notify();
    }

    pub(super) fn computer_element(
        &mut self,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let supported = self.controller.supports_computer_targets();
        let has_session = self.projection.active_session_id.is_some();
        let bound = self
            .computer
            .matches_session(self.projection.active_session_id.as_deref());
        let can_refresh = self.computer_can_refresh();
        let can_act = self.computer_can_act();
        let busy = self.computer.action_in_flight.is_some() || self.computer.stop_pending();
        // 渲染期收集 owned 副本，切断对 self 的借用（与 subagent_panel
        // 同口径），后续 entry() 需要 &mut self。
        let targets = self.computer.targets.clone();
        // 焦点句柄 / 实测布局随可见键收敛，不跨会话累积；与 AX 发布
        // 共用列表可见条件（失败态不保留按钮布局，杜绝隐藏按钮可 Press）。
        let mut visible: std::collections::HashSet<String> = std::collections::HashSet::new();
        visible.insert("computer-refresh".to_string());
        if self.computer_targets_visible() {
            for state in &targets {
                let key = target_key(state);
                visible.insert(dynamic_identifier("computer-target", &key));
                visible.insert(action_id(COMPUTER_APPROVE_ONCE_PREFIX, &key));
                visible.insert(action_id(COMPUTER_APPROVE_RUN_PREFIX, &key));
                visible.insert(action_id(COMPUTER_REVOKE_PREFIX, &key));
                visible.insert(action_id(COMPUTER_STOP_PREFIX, &key));
            }
        }
        let observations_visible = self.computer_observations_visible();
        let observation_rows = if observations_visible {
            self.computer
                .observations
                .iter()
                .take(OBSERVATION_MAX_ROWS)
                .map(|entry| (entry.sequence, entry.part_index))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if observations_visible {
            for key in &observation_rows {
                visible.insert(observation_row_id(*key));
            }
            if !self.computer.observations.is_empty() {
                visible.insert("computer-observation-preview".to_string());
                visible.insert("computer-observation-view".to_string());
                visible.insert("computer-observation-retry".to_string());
            }
        }
        self.computer_item_focus.retain(|id, _| visible.contains(id));
        self.computer_item_layouts.retain(|id, _| visible.contains(id));

        let refresh_layout = self
            .computer_item_layouts
            .entry("computer-refresh".to_string())
            .or_insert_with(ScrollHandle::new)
            .clone();
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(dark().border.subtle)
            .child(
                Label::new(t("computer.title"))
                    .size(font::XS)
                    .color(dark().text.tertiary),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("computer-refresh-layout")
                    .track_scroll(&refresh_layout)
                    .child(
                        Button::new("computer-refresh")
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                            .text_color(dark().text.secondary)
                            .disabled(!can_refresh)
                            .child(icon_sized(Icon::Refresh, px(metrics::ICON_SM)))
                            .tooltip(t("computer.tooltip_refresh"))
                            .track_focus(&self.computer_refresh_focus)
                            .on_click(cx.listener(|view, event, _window, cx| {
                                if view.consume_button_key_click("computer-refresh", event) {
                                    return;
                                }
                                view.refresh_computer_targets(cx);
                            }))
                            .on_activate(cx.listener(|view, _event, _window, cx| {
                                view.note_button_key_activate("computer-refresh");
                                view.refresh_computer_targets(cx);
                                cx.stop_propagation();
                            })),
                    ),
            );
        let body = if !connected {
            computer_placeholder("computer.unavailable").into_any_element()
        } else if !has_session {
            computer_placeholder("computer.no_task").into_any_element()
        } else if !bound {
            // 切会话后的短暂间隙：refresh 即 begin_loading 复位；旧会话
            // 数据不跨会话展示。旧 Host（minor < 27）永不派出查询、面板
            // 保持未绑定——直接给版本要求提示，不卡在 loading。
            if supported {
                computer_placeholder("computer.loading").into_any_element()
            } else {
                computer_placeholder("computer.version_required").into_any_element()
            }
        } else if self.computer.fetch == ComputerFetch::Failed {
            computer_placeholder_colored(self.computer.fetch_error.clone().unwrap_or_default())
                .into_any_element()
        } else {
            // CU-19：目标区与观测证据区共栏——停止后 run 结束（目标清
            // 空）仍能核对历史证据；目标空态在栏内如实分层。
            self.computer_panel_body(&targets, can_act, busy, cx)
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(header)
            .when_some(self.computer.stale_reason.clone(), |block, reason| {
                block.child(
                    div()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(dark().semantic.warning_text)
                        .text_size(font::XS)
                        .text_color(dark().semantic.warning_text)
                        .child(format!("{} · {reason}", t("common.stale_data"))),
                )
            })
            .when_some(self.computer.action_error.clone(), |block, reason| {
                block.child(
                    div()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(dark().semantic.danger_text)
                        .text_size(font::XS)
                        .text_color(dark().semantic.danger_text)
                        .child(format!("{} · {reason}", t("computer.action_failed"))),
                )
            })
            .when_some(computer_stop_status(&self.computer.stop), |block, (text, color)| {
                block.child(
                    div()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(color)
                        .text_size(font::XS)
                        .text_color(color)
                        .child(text),
                )
            })
            .child(body)
    }

    /// 面板正文：目标区 + 观测证据区共栏（单一滚动容器；停止后目标清
    /// 空时历史证据仍在，供用户核对操作前后状态）。
    fn computer_panel_body(
        &mut self,
        targets: &[ComputerTargetState],
        can_act: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut list = div()
            .id("computer-panel-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .p_2()
            .gap_2()
            .track_scroll(&self.computer.scroll)
            .overflow_y_scroll();
        if self.computer_targets_visible() {
            for state in targets {
                list = list.child(self.computer_target_card(state, can_act, busy, cx));
            }
            list = list.child(
                div()
                    .px_1()
                    .py_1()
                    .text_size(font::XS)
                    .text_color(dark().text.tertiary)
                    .child(t("computer.scope_note")),
            );
        } else {
            list = list.child(self.computer_targets_placeholder());
        }
        if self.computer_observations_visible() {
            list = list.child(self.computer_observation_section(cx));
        }
        list
    }

    /// 目标区空态（栏内分层，与 CU-18 全栏占位同一组文案）：version_
    /// required 只兜底「旧 Host 必然无数据」；证据区在其下继续呈现。
    fn computer_targets_placeholder(&mut self) -> gpui::AnyElement {
        if !self.controller.supports_computer_targets() {
            computer_placeholder("computer.version_required").into_any_element()
        } else if self.computer.fetch == ComputerFetch::Idle {
            computer_placeholder("computer.unavailable").into_any_element()
        } else if self.computer.fetch == ComputerFetch::Fetching {
            computer_placeholder("computer.loading").into_any_element()
        } else if self.computer.workspace_id.is_none() {
            // Host 权威：无项目会话恒空列表。
            computer_placeholder("computer.no_workspace").into_any_element()
        } else if self.projection.active_run_id.is_none() {
            computer_placeholder("computer.no_run").into_any_element()
        } else {
            computer_placeholder("computer.empty").into_any_element()
        }
    }

    /// 证据区可见条件（渲染与 AX 同源）：连接 × 有会话 × 面板绑定该
    /// 会话 × 目标查询非失败态 ×（已有证据，或 Host 支持观测 API——
    /// 旧 Host 恒无数据，不展示误导性空态；数据本身只能由 1.27 Host
    /// 提供，被动展示不叠加版本门，与 CU-18 行可见 / 动作 gate 分离
    /// 同构）。
    fn computer_observations_visible(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.active_session_id.is_some()
            && self
                .computer
                .matches_session(self.projection.active_session_id.as_deref())
            && self.computer.fetch != ComputerFetch::Failed
            && (!self.computer.observations.is_empty()
                || self.controller.supports_computer_targets())
    }

    /// 观测证据区：标题 + 选中预览 + 新→旧行列表（有界渲染）；当前
    ///（最新）与历史以 chip 区分。
    fn computer_observation_section(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let mut section = div()
            .flex()
            .flex_col()
            .gap_2()
            .pt_2()
            .mt_1()
            .border_t_1()
            .border_color(dark().border.subtle);
        let count = t("computer.observations_count")
            .replace("{}", &self.computer.observations.len().to_string());
        section = section.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    Label::new(t("computer.observations_title"))
                        .size(font::XS)
                        .color(dark().text.tertiary),
                )
                .child(div().flex_1())
                .child(Label::new(count).size(font::XS).color(dark().text.tertiary)),
        );
        match self.computer.obs_fetch {
            ComputerFetch::Failed => {
                section = section.child(
                    div()
                        .text_size(font::XS)
                        .text_color(dark().semantic.danger_text)
                        .child(format!(
                            "{} · {}",
                            t("computer.observations_failed"),
                            self.computer.obs_error.clone().unwrap_or_default()
                        )),
                );
            }
            ComputerFetch::Fetching => {
                section = section.child(
                    div()
                        .text_size(font::XS)
                        .text_color(dark().text.tertiary)
                        .child(t("computer.observations_loading")),
                );
            }
            _ if self.computer.observations.is_empty() => {
                section = section.child(
                    div()
                        .text_size(font::XS)
                        .text_color(dark().text.tertiary)
                        .child(t("computer.observations_empty")),
                );
            }
            _ => {
                section = section.child(self.computer_observation_preview(cx));
                let rows = self
                    .computer
                    .observations
                    .iter()
                    .take(OBSERVATION_MAX_ROWS)
                    .cloned()
                    .collect::<Vec<_>>();
                for entry in &rows {
                    section = section.child(self.computer_observation_row(entry, cx));
                }
                let hidden = self
                    .computer
                    .observations
                    .len()
                    .saturating_sub(OBSERVATION_MAX_ROWS);
                if hidden > 0 {
                    section = section.child(
                        div()
                            .text_size(font::XS)
                            .text_color(dark().text.tertiary)
                            .child(t("computer.observations_hidden").replace("{}", &hidden.to_string())),
                    );
                }
            }
        }
        if self.computer.obs_truncated {
            section = section.child(
                div()
                    .text_size(font::XS)
                    .text_color(dark().text.tertiary)
                    .child(t("computer.observations_truncated")),
            );
        }
        section
    }

    /// 选中证据的预览卡：chip（当前 / 历史）+ 时间 + run / 尺寸元信息 +
    /// 图像（缓存 / 加载进度 / 失败重试 / 查看入口）。
    fn computer_observation_preview(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let selected = self.computer.evidence_selected;
        let entry = selected.and_then(|key| {
            self.computer
                .observations
                .iter()
                .find(|entry| (entry.sequence, entry.part_index) == key)
                .cloned()
        });
        let layout = self
            .computer_item_layouts
            .entry("computer-observation-preview".to_string())
            .or_insert_with(ScrollHandle::new)
            .clone();
        let mut card = div()
            .id("computer-observation-preview-layout")
            .track_scroll(&layout)
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded(px(8.0))
            .border_1()
            .border_color(dark().border.subtle);
        let Some(entry) = entry else {
            return card;
        };
        let key = (entry.sequence, entry.part_index);
        let is_current = self
            .computer
            .observations
            .first()
            .map(|first| (first.sequence, first.part_index))
            == Some(key);
        card = card
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(observation_chip(is_current))
                    .child(div().flex_1())
                    .child(
                        div().text_size(font::XS).text_color(dark().text.tertiary).child(
                            super::timeline_entry::display_time(
                                &entry.captured_at_ms.to_string(),
                                super::now_unix_ms(),
                            ),
                        ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .text_size(font::XS)
                    .text_color(dark().text.secondary)
                    .child(observation_meta(&entry)),
            );
        if let Some(image) = self.computer.evidence_images.get(&key).cloned() {
            card = card.child(
                gpui::img(gpui::ImageSource::Image(image))
                    .w_full()
                    .h(px(180.0))
                    .object_fit(gpui::ObjectFit::Contain)
                    .rounded(px(metrics::CONTROL_RADIUS)),
            );
        } else if self
            .computer
            .evidence_load
            .as_ref()
            .is_some_and(|load| load.key == key)
        {
            let load = self.computer.evidence_load.as_ref().expect("checked above");
            card = card.child(
                div()
                    .id("computer-observation-loading")
                    .w_full()
                    .h(px(180.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(metrics::CONTROL_RADIUS))
                    .border_1()
                    .border_color(dark().border.subtle)
                    .text_size(font::XS)
                    .text_color(dark().text.tertiary)
                    .child(observation_progress(load.received, load.total)),
            );
        } else if self.computer.evidence_error.is_some() {
            let reason = self.computer.evidence_error.clone().unwrap_or_default();
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(font::XS)
                            .text_color(dark().semantic.danger_text)
                            .child(format!(
                                "{} · {reason}",
                                t("computer.observation_load_failed")
                            )),
                    )
                    .child(self.computer_observation_action_button(
                        "computer-observation-retry",
                        t("computer.observation_retry").to_string(),
                        key,
                        cx,
                    )),
            );
        } else {
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .child(self.computer_observation_action_button(
                        "computer-observation-view",
                        t("computer.observation_view").to_string(),
                        key,
                        cx,
                    )),
            );
        }
        card
    }

    /// 预览卡内的小动作按钮（查看 / 重试）：与证据选择共用入口。
    fn computer_observation_action_button(
        &mut self,
        id: &str,
        label: String,
        key: EvidenceKey,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let focus = self
            .computer_item_focus
            .entry(id.to_string())
            .or_insert_with(|| cx.focus_handle().tab_stop(true).tab_index(INSPECTOR_TAB_INDEX))
            .clone();
        let layout = self
            .computer_item_layouts
            .entry(id.to_string())
            .or_insert_with(ScrollHandle::new)
            .clone();
        let click_id = id.to_string();
        let activate_id = id.to_string();
        let button_id = id.to_string();
        div()
            .id(SharedString::from(format!("{id}-layout")))
            .track_scroll(&layout)
            .child(
                Button::new(button_id)
                    .variant(ButtonVariant::Ghost)
                    .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                    .text_color(dark().text.secondary)
                    .label(SharedString::from(label))
                    .text_size(font::SM)
                    .height(px(28.0))
                    .radius(6.0)
                    .track_focus(&focus)
                    .on_click(cx.listener(move |view, event, _window, cx| {
                        if view.consume_button_key_click(&click_id, event) {
                            return;
                        }
                        view.computer_retry_evidence(key, cx);
                    }))
                    .on_activate(cx.listener(move |view, _event, _window, cx| {
                        view.note_button_key_activate(&activate_id);
                        view.computer_retry_evidence(key, cx);
                        cx.stop_propagation();
                    })),
            )
    }

    /// 重试 / 查看共用：清错误后重新按需加载（证据身份不变）。
    fn computer_retry_evidence(&mut self, key: EvidenceKey, cx: &mut Context<Self>) {
        if self.computer.evidence_images.contains_key(&key) {
            self.computer.evidence_selected = Some(key);
            cx.notify();
            return;
        }
        self.computer.evidence_error = None;
        self.computer.evidence_selected = Some(key);
        self.computer.evidence_load = None;
        self.computer_start_evidence_load(cx);
    }

    /// 单条证据行：Button 承载鼠标 / 键盘 / AX 三通道；选中态以描边
    /// 表达。主行 = 时间 · 工具；副行 = run 标识（观测自带事实，不与
    /// 目标卡对齐冒充归属）。
    fn computer_observation_row(
        &mut self,
        entry: &ObservationEvidenceEntry,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let key = (entry.sequence, entry.part_index);
        let id = observation_row_id(key);
        let focus = self
            .computer_item_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true).tab_index(INSPECTOR_TAB_INDEX))
            .clone();
        let layout = self
            .computer_item_layouts
            .entry(id.clone())
            .or_insert_with(ScrollHandle::new)
            .clone();
        let selected = self.computer.evidence_selected == Some(key);
        let is_current = self
            .computer
            .observations
            .first()
            .map(|first| (first.sequence, first.part_index))
            == Some(key);
        let label = SharedString::from(format!(
            "{} · {}",
            super::timeline_entry::display_time(
                &entry.captured_at_ms.to_string(),
                super::now_unix_ms()
            ),
            entry
                .tool_name
                .clone()
                .unwrap_or_else(|| entry.tool_call_id.clone())
        ));
        let target_line = observation_target_label(entry);
        let click_key = key;
        let activate_key = key;
        let can_load =
            self.computer_observations_visible() && self.computer.obs_fetch != ComputerFetch::Failed;
        div()
            .id(SharedString::from(format!("{id}-layout")))
            .track_scroll(&layout)
            .rounded(px(6.0))
            .when(selected, |row| {
                row.border_1().border_color(dark().accent.primary)
            })
            .child(
                Button::new(id)
                    .variant(ButtonVariant::Ghost)
                    .disabled(!can_load)
                    .label(label)
                    .text_size(font::XS)
                    .height(px(24.0))
                    .track_focus(&focus)
                    .on_click(cx.listener(move |view, event, _window, cx| {
                        let id = observation_row_id(click_key);
                        if view.consume_button_key_click(&id, event) {
                            return;
                        }
                        view.computer_select_observation(click_key, cx);
                    }))
                    .on_activate(cx.listener(move |view, _event, _window, cx| {
                        let id = observation_row_id(activate_key);
                        view.note_button_key_activate(&id);
                        view.computer_select_observation(activate_key, cx);
                        cx.stop_propagation();
                    })),
            )
            .child(
                div()
                    .px_2()
                    .text_size(font::XS)
                    .text_color(dark().text.tertiary)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(observation_chip(is_current))
                            .child(
                                div().flex_1().min_w_0().truncate().child(target_line),
                            ),
                    ),
            )
    }

    /// 单目标卡：标题 + 授权状态 + 副行（族 / 浏览器）+ 状态行 + 能力
    /// chips + 授权按钮行。卡片外层 div 承载实测布局（AX 行几何同源）。
    fn computer_target_card(
        &mut self,
        state: &ComputerTargetState,
        can_act: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let key = target_key(state);
        let row_id = dynamic_identifier("computer-target", &key);
        let row_layout = self
            .computer_item_layouts
            .entry(row_id.clone())
            .or_insert_with(ScrollHandle::new)
            .clone();
        let enabled = can_act && !busy;
        // 能力 chips：✓/?/× 字形 + 颜色双通道（不靠颜色单独表达状态）。
        let mut caps_row = div().flex().flex_row().flex_wrap().gap_1();
        for cap in &state.capabilities {
            let (glyph, color) = support_glyph(cap.background);
            caps_row = caps_row.child(
                div()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(dark().border.subtle)
                    .text_size(font::XS)
                    .text_color(color)
                    .child(format!("{glyph} {}", action_name(cap.action))),
            );
        }
        let mut card = div()
            .id(SharedString::from(format!("{row_id}-layout")))
            .track_scroll(&row_layout)
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded(px(8.0))
            .border_1()
            .border_color(dark().border.subtle)
            // 标题行：flex_row + flex_1 + min_w_0 同层承载 truncate
            //（min_w_0 截断投毒修法，见 AGENTS.md 工程经验）。
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().child(
                            div()
                                .truncate()
                                .text_size(font::SM)
                                .text_color(dark().text.primary)
                                .child(target_title(state)),
                        ),
                    )
                    .child(
                        Label::new(grant_label(state.grant))
                            .size(font::XS)
                            .color(grant_color(state.grant)),
                    ),
            )
            .child(
                div().flex().flex_row().child(
                    div().flex_1().min_w_0().child(
                        div()
                            .truncate()
                            .text_size(font::XS)
                            .text_color(dark().text.tertiary)
                            .child(target_subtitle(state)),
                    ),
                ),
            )
            .child(
                div()
                    .w_full()
                    .text_size(font::XS)
                    .text_color(dark().text.secondary)
                    .child(target_state_text(state)),
            )
            .child(caps_row);
        if let Some(note) = minimized_note(&state.capabilities) {
            card = card.child(
                div()
                    .w_full()
                    .text_size(font::XS)
                    .text_color(dark().text.tertiary)
                    .child(note),
            );
        }
        if has_unverified(&state.capabilities) {
            card = card.child(
                div()
                    .w_full()
                    .text_size(font::XS)
                    .text_color(dark().semantic.warning_text)
                    .child(t("computer.permission_note")),
            );
        }
        card.child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .pt_1()
                .child(self.computer_action_button(
                    COMPUTER_APPROVE_ONCE_PREFIX,
                    &key,
                    t("computer.allow_once"),
                    ButtonVariant::Primary,
                    enabled,
                    cx,
                ))
                .child(self.computer_action_button(
                    COMPUTER_APPROVE_RUN_PREFIX,
                    &key,
                    t("computer.allow_for_run"),
                    ButtonVariant::Success,
                    enabled,
                    cx,
                ))
                .child(self.computer_action_button(
                    COMPUTER_REVOKE_PREFIX,
                    &key,
                    t("computer.revoke"),
                    ButtonVariant::Danger,
                    computer_revoke_enabled(can_act, busy),
                    cx,
                ))
                .child(self.computer_action_button(
                    COMPUTER_STOP_PREFIX,
                    &key,
                    t("computer.stop_task"),
                    ButtonVariant::Danger,
                    self.computer_stop_button_enabled(can_act, &state.scope.run_id),
                    cx,
                )),
        )
    }

    /// 卡片授权按钮：id = 前缀 + 目标 key（dynamic_identifier 转义）；
    /// 鼠标 / 键盘 / AX 三通道共用同一 dispatch 与 gate。外层 div 承载
    /// 实测布局供 AX 发布。
    fn computer_action_button(
        &mut self,
        prefix: &'static str,
        key: &str,
        label: &'static str,
        variant: ButtonVariant,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = action_id(prefix, key);
        let focus = self
            .computer_item_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true).tab_index(INSPECTOR_TAB_INDEX))
            .clone();
        let layout = self
            .computer_item_layouts
            .entry(id.clone())
            .or_insert_with(ScrollHandle::new)
            .clone();
        let click_id = id.clone();
        let activate_id = id.clone();
        let click_key = key.to_string();
        let activate_key = key.to_string();
        div()
            .id(SharedString::from(format!("{id}-layout")))
            .track_scroll(&layout)
            .child(
                Button::new(id)
                    .variant(variant)
                    .disabled(!enabled)
                    .label(label)
                    .text_size(font::SM)
                    .height(px(28.0))
                    .radius(6.0)
                    .track_focus(&focus)
                    .on_click(cx.listener(move |view, event, _window, cx| {
                        if view.consume_button_key_click(&click_id, event) {
                            return;
                        }
                        view.computer_action_for_prefix(prefix, &click_key, cx);
                    }))
                    .on_activate(cx.listener(move |view, _event, _window, cx| {
                        view.note_button_key_activate(&activate_id);
                        view.computer_action_for_prefix(prefix, &activate_key, cx);
                        cx.stop_propagation();
                    })),
            )
    }
}

/// 占位文案按 i18n key 取主文案与说明（同源；不能按翻译后的串匹配）。
fn computer_placeholder(key: &'static str) -> EmptyState {
    let description = match key {
        "computer.unavailable" => t("computer.unavailable_desc"),
        "computer.version_required" => t("computer.version_required_desc"),
        "computer.no_task" => t("computer.no_task_desc"),
        "computer.no_workspace" => t("computer.no_workspace_desc"),
        "computer.no_run" => t("computer.no_run_desc"),
        "computer.empty" => t("computer.empty_desc"),
        "computer.loading" => t("computer.loading_desc"),
        _ => t("common.placeholder_no_details"),
    };
    let mut placeholder = computer_placeholder_content(
        t(key).to_string(),
        description.to_string(),
        dark().text.primary,
    );
    if key == "computer.loading" {
        placeholder = placeholder.child(loading_skeleton("computer.loading-skeleton"));
    }
    placeholder
}

/// 失败占位：标题本地化；reason 为 wire 数据，不翻译。
fn computer_placeholder_colored(reason: String) -> EmptyState {
    computer_placeholder_content(
        t("computer.error_title").into(),
        reason,
        dark().semantic.danger_text,
    )
}

fn computer_placeholder_content(title: String, description: String, color: gpui::Rgba) -> EmptyState {
    EmptyState::new()
        .icon(Icon::Approvals)
        .icon_size(px(metrics::ICON_SIZE))
        .child(Label::new(title).size(font::BASE).color(color))
        .child(
            Label::new(description)
                .size(font::SM)
                .color(dark().text.secondary),
        )
}

/// tracked 布局的 AX 矩形（可选按滚动视口裁剪）；未测量 / 零面积不发布
///（RV-07：几何不伪造）。
fn measured_ax_rect(
    view: &AppView,
    id: &str,
    viewport: Option<&gpui::Bounds<gpui::Pixels>>,
) -> Option<AxRect> {
    let bounds = view.computer_item_layouts.get(id)?.bounds();
    let rect = match viewport {
        Some(viewport) => bounds.intersect(viewport),
        None => bounds,
    };
    if rect.size.width <= px(0.0) || rect.size.height <= px(0.0) {
        return None;
    }
    Some(AxRect::new(
        rect.origin.x.into(),
        rect.origin.y.into(),
        rect.size.width.into(),
        rect.size.height.into(),
    ))
}

/// 「后台操作」页 AX 投影：组 description 承载 fetch / stale / 动作错误
/// 状态；行与按钮只发布实测几何（视口裁剪）。enabled 与渲染 disabled
/// 同源（computer_can_refresh / computer_can_act）。
pub(super) fn computer_ax(view: &AppView, window: &Window, frame: AxRect) -> AxNode {
    let fetch_state = match view.computer.fetch {
        ComputerFetch::Idle => "idle".to_string(),
        ComputerFetch::Fetching => "loading".to_string(),
        ComputerFetch::Ready => format!("ready · {} targets", view.computer.targets.len()),
        ComputerFetch::Failed => format!(
            "failed · {}",
            view.computer.fetch_error.as_deref().unwrap_or_default()
        ),
    };
    let mut description = format!("Computer use targets · {fetch_state}");
    if let Some(reason) = &view.computer.stale_reason {
        description.push_str(&format!(" · stale · {reason}"));
    }
    if let Some(error) = &view.computer.action_error {
        description.push_str(&format!(" · {error}"));
    }
    if let Some((stop, _)) = computer_stop_status(&view.computer.stop) {
        description.push_str(&format!(" · {stop}"));
    }
    description.push_str(&format!(
        " · {} observations",
        view.computer.observations.len()
    ));
    let mut node = AxNode::new("computer", AxRole::Group, t("inspector.tab_computer"), frame)
        .description(description);
    if let Some(rect) = measured_ax_rect(view, "computer-refresh", None) {
        node = node.child(
            AxNode::new("computer-refresh", AxRole::Button, t("computer.tooltip_refresh"), rect)
                .enabled(view.computer_can_refresh())
                .focused(
                    view.open_menu.is_none() && view.computer_refresh_focus.is_focused(window),
                )
                .action(AxAction::Press),
        );
    }
    let can_act = view.computer_can_act();
    let busy = view.computer.action_in_flight.is_some() || view.computer.stop_pending();
    // 与渲染共用列表可见条件：失败占位期间不凭保留数据发布行与按钮。
    if view.computer_targets_visible() {
        let viewport = view.computer.scroll.bounds();
        if viewport.size.width > px(0.0) && viewport.size.height > px(0.0) {
            let mut list = AxNode::new(
                "computer-target-list",
                AxRole::List,
                t("computer.title"),
                AxRect::new(
                    viewport.origin.x.into(),
                    viewport.origin.y.into(),
                    viewport.size.width.into(),
                    viewport.size.height.into(),
                ),
            );
            for state in &view.computer.targets {
                let key = target_key(state);
                let row_id = dynamic_identifier("computer-target", &key);
                let Some(rect) = measured_ax_rect(view, &row_id, Some(&viewport)) else {
                    continue;
                };
                let mut item = AxNode::new(row_id, AxRole::ListItem, target_title(state), rect)
                    .value(format!(
                        "{} · {}",
                        target_subtitle(state),
                        target_state_text(state)
                    ))
                    .description(capability_detail(&state.capabilities));
                for (prefix, label, enabled) in [
                    (
                        COMPUTER_APPROVE_ONCE_PREFIX,
                        t("computer.allow_once"),
                        can_act && !busy,
                    ),
                    (
                        COMPUTER_APPROVE_RUN_PREFIX,
                        t("computer.allow_for_run"),
                        can_act && !busy,
                    ),
                    (
                        COMPUTER_REVOKE_PREFIX,
                        t("computer.revoke"),
                        computer_revoke_enabled(can_act, busy),
                    ),
                    (
                        COMPUTER_STOP_PREFIX,
                        t("computer.stop_task"),
                        view.computer_stop_button_enabled(can_act, &state.scope.run_id),
                    ),
                ] {
                    let id = action_id(prefix, &key);
                    let Some(rect) = measured_ax_rect(view, &id, Some(&viewport)) else {
                        continue;
                    };
                    item = item.child(
                        AxNode::new(id.clone(), AxRole::Button, label, rect)
                            .enabled(enabled)
                            .focused(
                                view.open_menu.is_none()
                                    && view
                                        .computer_item_focus
                                        .get(&id)
                                        .is_some_and(|focus| focus.is_focused(window)),
                            )
                            .action(AxAction::Press),
                    );
                }
                list = list.child(item);
            }
            node = node.child(list);
        }
    }
    // CU-19：观测证据（渲染可见条件同源；行 / 预览 / 按钮只发实测
    // 几何，视口裁剪）。行 Press 即选择并加载（与渲染 Button 同一入口）。
    if view.computer_observations_visible() && !view.computer.observations.is_empty() {
        let viewport = view.computer.scroll.bounds();
        if viewport.size.width > px(0.0) && viewport.size.height > px(0.0) {
            let mut list = AxNode::new(
                "computer-observation-list",
                AxRole::List,
                t("computer.observations_title"),
                AxRect::new(
                    viewport.origin.x.into(),
                    viewport.origin.y.into(),
                    viewport.size.width.into(),
                    viewport.size.height.into(),
                ),
            );
            for entry in view.computer.observations.iter().take(OBSERVATION_MAX_ROWS) {
                let key = (entry.sequence, entry.part_index);
                let row_id = observation_row_id(key);
                let focus_row_id = row_id.clone();
                let Some(rect) = measured_ax_rect(view, &row_id, Some(&viewport)) else {
                    continue;
                };
                let selected = view.computer.evidence_selected == Some(key);
                let mut item = AxNode::new(
                    row_id,
                    AxRole::Button,
                    format!(
                        "{} · {}",
                        super::timeline_entry::display_time(
                            &entry.captured_at_ms.to_string(),
                            super::now_unix_ms()
                        ),
                        entry
                            .tool_name
                            .clone()
                            .unwrap_or_else(|| entry.tool_call_id.clone())
                    ),
                    rect,
                )
                .value(format!(
                    "{} · {}",
                    observation_target_label(entry),
                    if view
                        .computer
                        .observations
                        .first()
                        .map(|first| (first.sequence, first.part_index))
                        == Some(key)
                    {
                        t("computer.observation_current")
                    } else {
                        t("computer.observation_history")
                    }
                ))
                .enabled(
                    view.computer_observations_visible()
                        && view.computer.obs_fetch != ComputerFetch::Failed,
                )
                .focused(
                    view.open_menu.is_none()
                        && view
                            .computer_item_focus
                            .get(&focus_row_id)
                            .is_some_and(|focus| focus.is_focused(window)),
                )
                .action(AxAction::Press);
                if selected {
                    item = item.description(t("computer.observation_selected"));
                }
                list = list.child(item);
            }
            node = node.child(list);
            if let Some(rect) = measured_ax_rect(view, "computer-observation-preview", Some(&viewport)) {
                let mut value = match view.computer.evidence_selected.and_then(|key| view
                    .computer
                    .observations
                    .iter()
                    .find(|entry| (entry.sequence, entry.part_index) == key))
                {
                    Some(entry) => observation_meta(entry),
                    None => t("computer.observations_empty").to_string(),
                };
                if let Some(load) = &view.computer.evidence_load {
                    value.push_str(" · ");
                    value.push_str(&observation_progress(load.received, load.total));
                }
                if let Some(error) = &view.computer.evidence_error {
                    value.push_str(&format!(" · {error}"));
                }
                node = node.child(
                    AxNode::new(
                        "computer-observation-preview",
                        AxRole::StaticText,
                        t("computer.observations_title"),
                        rect,
                    )
                    .value(value),
                );
            }
            for (id, label) in [
                ("computer-observation-view", t("computer.observation_view")),
                ("computer-observation-retry", t("computer.observation_retry")),
            ] {
                if let Some(rect) = measured_ax_rect(view, id, Some(&viewport)) {
                    let enabled = view.computer.evidence_error.is_some()
                        || view
                            .computer
                            .evidence_selected
                            .is_some_and(|key| !view.computer.evidence_images.contains_key(&key));
                    let action_key = view.computer.evidence_selected;
                    node = node.child(
                        AxNode::new(id, AxRole::Button, label, rect)
                            .enabled(enabled && action_key.is_some())
                            .focused(
                                view.open_menu.is_none()
                                    && view
                                        .computer_item_focus
                                        .get(id)
                                        .is_some_and(|focus| focus.is_focused(window)),
                            )
                            .action(AxAction::Press),
                    );
                }
            }
        }
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_client::{ComputerAppIdentity, ComputerTargetScope};

    /// 有效 1×1 RGBA PNG（zlib IDAT + CRC 全真）：分块重组与后台解码
    /// 校验共用。
    const VALID_PNG: [u8; 70] = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49,
        0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06,
        0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44,
        0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00,
        0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
        0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn caps(background: ComputerCapabilitySupport) -> Vec<ComputerTargetCapability> {
        [
            ComputerActionKind::CaptureWindow,
            ComputerActionKind::AxRead,
            ComputerActionKind::AxSemanticWrite,
            ComputerActionKind::AxPress,
            ComputerActionKind::TargetedKeys,
            ComputerActionKind::MenuShortcut,
            ComputerActionKind::MenubarPress,
            ComputerActionKind::Pointer,
            ComputerActionKind::WindowMove,
            ComputerActionKind::WindowResize,
        ]
        .into_iter()
        .map(|action| ComputerTargetCapability {
            action,
            background,
            minimized: background,
        })
        .collect()
    }

    fn fixture_targets(run: &str) -> Vec<ComputerTargetState> {
        let scope = || ComputerTargetScope {
            workspace_id: "ws-1".into(),
            run_id: run.into(),
        };
        vec![
            ComputerTargetState {
                target: ComputerTargetIdentity::Application {
                    app: ComputerAppIdentity {
                        bundle_id: "com.apple.TextEdit".into(),
                        family: ComputerAppFamily::Appkit,
                    },
                },
                scope: scope(),
                grant: Some(ComputerGrantKind::ForRun),
                bound_window: Some("window-7".into()),
                occupied_by: None,
                dispatch_paused: false,
                capabilities: caps(ComputerCapabilitySupport::Supported),
            },
            ComputerTargetState {
                target: ComputerTargetIdentity::Website {
                    browser: ComputerAppIdentity {
                        bundle_id: "com.microsoft.edgemac".into(),
                        family: ComputerAppFamily::Browser,
                    },
                    origin: "https://example.test".into(),
                },
                scope: scope(),
                grant: None,
                bound_window: None,
                occupied_by: None,
                dispatch_paused: true,
                capabilities: caps(ComputerCapabilitySupport::Unverified),
            },
        ]
    }

    /// SessionId 未从 client re-export：Data 经冻结 serde 形状构造
    ///（与 controller 的构造口径同族）。
    fn list_data(session: &str, run: &str) -> ComputerTargetListData {
        serde_json::from_value(serde_json::json!({
            "session_id": session,
            "workspace_id": "ws-1",
            "targets": serde_json::to_value(fixture_targets(run)).expect("targets serialize"),
        }))
        .expect("frozen ComputerTargetListData shape")
    }

    fn observation_entry(sequence: u64, part_index: u32, run: Option<&str>) -> ObservationEvidenceEntry {
        serde_json::from_value(serde_json::json!({
            "sequence": sequence,
            "event_id": format!("evt-{sequence}"),
            "run_id": run,
            "tool_call_id": format!("call-{sequence}"),
            "tool_name": "computer",
            "part_index": part_index,
            "media_type": "image/png",
            "byte_length": 8,
            "image_width": 4,
            "image_height": 4,
            "captured_at_ms": 1000,
            "status": "historical",
        }))
        .expect("frozen ObservationEvidenceEntry shape")
    }

    fn observation_list_data(
        entries: Vec<ObservationEvidenceEntry>,
        head: u64,
        complete: bool,
        next: Option<u64>,
    ) -> ObservationListData {
        let mut value = serde_json::json!({
            "session_id": "s1",
            "entries": serde_json::to_value(entries).expect("entries serialize"),
            "head_sequence": head,
            "complete": complete,
        });
        if let Some(cursor) = next {
            value["next_sequence"] = serde_json::json!(cursor);
        }
        serde_json::from_value(value).expect("frozen ObservationListData shape")
    }

    fn observation_chunk(
        sequence: u64,
        part_index: u32,
        offset: u64,
        data: &str,
        eof: bool,
        byte_length: u64,
    ) -> ObservationImageData {
        serde_json::from_value(serde_json::json!({
            "session_id": "s1",
            "sequence": sequence,
            "event_id": format!("evt-{sequence}"),
            "part_index": part_index,
            "media_type": "image/png",
            "byte_length": byte_length,
            "offset": offset,
            "data": data,
            "eof": eof,
            "captured_at_ms": 1000,
            "image_width": 4,
            "image_height": 4,
            "status": "historical",
        }))
        .expect("frozen ObservationImageData shape")
    }

    #[test]
    fn computer_panel_rejects_stale_receipts_and_resets_on_session_switch() {
        let mut state = ComputerPanelState::default();
        let first = state.begin_loading("s1");
        let second = state.begin_loading("s1");
        assert!(!state.apply_loaded("s1", first, list_data("s1", "r1")));
        assert!(state.apply_loaded("s1", second, list_data("s1", "r1")));
        assert_eq!(state.targets.len(), 2);
        // 换会话复位：旧会话数据不跨会话展示。
        let third = state.begin_loading("s2");
        assert!(state.targets.is_empty());
        assert!(state.workspace_id.is_none());
        assert!(!state.apply_loaded("s1", third, list_data("s1", "r1")));
    }

    #[test]
    fn computer_panel_mark_stale_keeps_data_and_invalidates_old_responses() {
        let mut state = ComputerPanelState::default();
        let request = state.begin_loading("s1");
        assert!(state.apply_loaded("s1", request, list_data("s1", "r1")));
        state.action_in_flight = Some(("key".into(), 3));
        state.mark_stale("connection lost");
        assert_eq!(state.fetch, ComputerFetch::Ready);
        assert_eq!(state.targets.len(), 2);
        assert!(state.action_in_flight.is_none(), "在途动作不会再有回执");
        assert_eq!(state.stale_reason.as_deref(), Some("connection lost"));
        // request_id 归零：断线前排出的请求迟到回执失效。
        assert!(!state.apply_loaded("s1", request, list_data("s1", "r1")));
    }

    /// 回执收敛要求（会话 + 派出代次）双匹配：A→B→A 切换后旧 A 的
    /// 迟到回执不清除新动作的在途标记或覆盖错误；只有匹配回执落地
    ///（返回 true，调用方才触发重查）。
    #[test]
    fn computer_panel_finish_action_requires_matching_generation() {
        let mut state = ComputerPanelState::default();
        state.begin_loading("s1");
        state.action_in_flight = Some(("key".into(), 7));
        // 跨会话回执不落地。
        assert!(!state.finish_action("s2", 7, Ok(())));
        assert_eq!(state.action_in_flight, Some(("key".into(), 7)));
        // 同会话但代次不匹配（旧 A 回执迟到）不落地。
        assert!(!state.finish_action("s1", 6, Ok(())));
        assert_eq!(state.action_in_flight, Some(("key".into(), 7)));
        // 匹配回执收敛：Err 落在面板呈现，Ok 清错误。
        assert!(state.finish_action("s1", 7, Err("unknown_run".into())));
        assert!(state.action_in_flight.is_none());
        assert_eq!(state.action_error.as_deref(), Some("unknown_run"));
        state.action_in_flight = Some(("key".into(), 9));
        assert!(state.finish_action("s1", 9, Ok(())));
        assert!(state.action_in_flight.is_none());
        assert!(state.action_error.is_none());
        // 新一轮在途登记后，上一轮的迟到回执不再收敛。
        state.action_in_flight = Some(("key".into(), 11));
        assert!(!state.finish_action("s1", 9, Ok(())));
        assert_eq!(state.action_in_flight, Some(("key".into(), 11)));
    }

    #[test]
    fn computer_action_id_roundtrip_uses_dynamic_escape() {
        let targets = fixture_targets("r1");
        let site = &targets[1];
        let key = target_key(site);
        assert!(key.contains("https://example.test"));
        for prefix in [
            COMPUTER_APPROVE_ONCE_PREFIX,
            COMPUTER_APPROVE_RUN_PREFIX,
            COMPUTER_REVOKE_PREFIX,
        ] {
            let id = action_id(prefix, &key);
            assert!(id.starts_with(prefix));
            assert_eq!(
                find_target_for_action(&targets, prefix, &id).map(target_key),
                Some(key.clone()),
                "同源 id 反查命中同一目标"
            );
        }
        // 其他前缀的 id 不在本前缀命名空间命中。
        let once_id = action_id(COMPUTER_APPROVE_ONCE_PREFIX, &key);
        assert!(find_target_for_action(&targets, COMPUTER_REVOKE_PREFIX, &once_id).is_none());
    }

    /// 观测分页（探测 → 尾窗 → 预算续读）与证据身份：探测页只习得
    /// head（条目与游标均不消费）；尾窗被单页字节预算截断（!complete +
    /// 游标）时同代次续读合并落库；新截图到达时跟随最新，手动选中的
    /// 历史观测保持身份；跨会话 / 旧代次回执不落地。
    #[test]
    fn observation_tail_fetch_keeps_identity_and_follows_latest() {
        let mut state = ComputerPanelState::default();
        state.begin_loading("s1");
        // 探测页（Host 真实形状：from=1 单条目页带 next_sequence 游标）
        // 只取 head=1000 锚定尾窗（after=700），条目与游标不落库。
        let request = state.begin_observation_loading();
        state.note_observation_dispatch(true);
        let follow = state.apply_observations(
            "s1",
            request,
            &observation_list_data(
                vec![observation_entry(1, 0, Some("r1"))],
                1000,
                false,
                Some(1),
            ),
        );
        assert_eq!(
            follow,
            Some((Some(1000 - OBSERVATION_EVENT_WINDOW), OBSERVATION_EVENT_WINDOW as u32))
        );
        assert!(state.obs_truncated, "head 超出尾窗即如实标注截断");
        // 尾窗首页被字节预算截断（页内 oldest→newest，游标=最后纳入
        // 事件）→ 返回续读参数（after=cursor，Host from=cursor+1）。
        state.note_observation_dispatch(false);
        assert_eq!(
            state.apply_observations(
                "s1",
                request,
                &observation_list_data(
                    vec![
                        observation_entry(950, 0, Some("r1")),
                        observation_entry(990, 0, Some("r1")),
                    ],
                    1000,
                    false,
                    Some(990),
                )
            ),
            Some((Some(990), OBSERVATION_EVENT_WINDOW as u32))
        );
        // 续读完整页：两页合并落库（新→旧排序），选中最新一条。
        state.note_observation_dispatch(false);
        assert_eq!(
            state.apply_observations(
                "s1",
                request,
                &observation_list_data(
                    vec![
                        observation_entry(995, 0, Some("r1")),
                        observation_entry(1000, 0, Some("r1")),
                    ],
                    1000,
                    true,
                    None,
                )
            ),
            None
        );
        assert_eq!(state.obs_head, 1000);
        assert_eq!(state.evidence_selected, Some((1000, 0)));
        assert_eq!(
            state
                .observations
                .iter()
                .map(|entry| entry.sequence)
                .collect::<Vec<_>>(),
            vec![1000, 995, 990, 950],
            "多页合并后按新→旧排序"
        );
        assert!(state.obs_truncated, "截断标注跨页保持（尾窗外仍有历史）");
        // 新截图（1005）到达：用户停在旧最新（1000）上 → 跟随最新。
        let next = state.begin_observation_loading();
        state.note_observation_dispatch(true);
        state.apply_observations(
            "s1",
            next,
            &observation_list_data(Vec::new(), 1005, true, None),
        );
        state.note_observation_dispatch(false);
        state.apply_observations(
            "s1",
            next,
            &observation_list_data(
                vec![
                    observation_entry(1000, 0, Some("r1")),
                    observation_entry(1005, 0, Some("r1")),
                ],
                1005,
                true,
                None,
            ),
        );
        assert_eq!(state.evidence_selected, Some((1005, 0)));
        // 手动选中的历史观测保持身份，不被刷新抢走。
        state.evidence_selected = Some((990, 0));
        let third = state.begin_observation_loading();
        state.note_observation_dispatch(true);
        state.apply_observations(
            "s1",
            third,
            &observation_list_data(Vec::new(), 1005, true, None),
        );
        state.note_observation_dispatch(false);
        state.apply_observations(
            "s1",
            third,
            &observation_list_data(
                vec![
                    observation_entry(990, 0, Some("r1")),
                    observation_entry(1005, 0, Some("r1")),
                ],
                1005,
                true,
                None,
            ),
        );
        assert_eq!(state.evidence_selected, Some((990, 0)));
        // 跨会话 / 旧代次回执不落地。
        assert_eq!(
            state.apply_observations(
                "s2",
                third,
                &observation_list_data(Vec::new(), 1, true, None)
            ),
            None
        );
        assert_eq!(state.obs_head, 1005);
    }

    /// 图像分块重组：偏移连续、base64 合法、总量 ≤ 条目申报；eof 时
    /// 魔数嗅探通过即收齐移交后台解码（在途以 (身份, 代次) 标记，回执
    /// 匹配落地入缓存 / 记错误）；任一校验失败即清在途并记录错误
    ///（fail-closed，不冒充成功）。
    #[test]
    fn observation_image_chunks_reassemble_and_fail_closed() {
        use base64::Engine as _;
        let mut state = ComputerPanelState::default();
        state.begin_loading("s1");
        state.evidence_selected = Some((5, 0));
        state.evidence_load = Some(EvidenceLoad {
            key: (5, 0),
            request_id: 9,
            next_offset: 0,
            received: 0,
            total: VALID_PNG.len() as u64,
            buffer: Vec::new(),
        });
        let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let first = encode(&VALID_PNG[..8]);
        let rest = encode(&VALID_PNG[8..]);
        // 首块：offset 0（魔数 8 字节），非 eof → 返回续读偏移。
        assert_eq!(
            state.apply_observation_chunk(
                "s1",
                9,
                &observation_chunk(5, 0, 0, &first, false, VALID_PNG.len() as u64)
            ),
            ObservationChunkOutcome::Continue((5, 0), 8)
        );
        // 末块：剩余字节 + eof → 收齐移交后台解码（不在此解码，UI 线程
        // 不做 CPU 密集工作）。
        assert_eq!(
            state.apply_observation_chunk(
                "s1",
                9,
                &observation_chunk(5, 0, 8, &rest, true, VALID_PNG.len() as u64)
            ),
            ObservationChunkOutcome::Decode(EvidenceDecode {
                key: (5, 0),
                request_id: 9,
                format: gpui::ImageFormat::Png,
                bytes: VALID_PNG.to_vec(),
            })
        );
        assert_eq!(state.evidence_decoding, Some(((5, 0), 9)));
        // 后台解码成功回执：代次匹配 → 入缓存；迟到 / 旧代次不落地。
        assert!(!state.apply_observation_decoded("s1", 8, (5, 0), VALID_PNG.to_vec()));
        assert!(!state.evidence_images.contains_key(&(5, 0)));
        assert!(state.apply_observation_decoded("s1", 9, (5, 0), VALID_PNG.to_vec()));
        assert!(state.evidence_images.contains_key(&(5, 0)));
        assert!(state.evidence_decoding.is_none());
        assert!(state.evidence_load.is_none());
        assert!(state.evidence_error.is_none());
        // 成功回执按身份入缓存：当前选中别的证据同样落地，不产生错误。
        state.evidence_decoding = Some(((16, 0), 19));
        state.evidence_selected = Some((5, 0));
        assert!(state.apply_observation_decoded("s1", 19, (16, 0), VALID_PNG.to_vec()));
        assert!(state.evidence_images.contains_key(&(16, 0)));
        assert!(state.evidence_error.is_none());

        // 后台解码失败回执（预算 / 解码失败）：代次匹配 → 记错误可重试。
        state.evidence_load = Some(EvidenceLoad {
            key: (10, 0),
            request_id: 14,
            next_offset: 0,
            received: 0,
            total: VALID_PNG.len() as u64,
            buffer: Vec::new(),
        });
        let whole = encode(&VALID_PNG);
        assert_eq!(
            state.apply_observation_chunk(
                "s1",
                14,
                &observation_chunk(10, 0, 0, &whole, true, VALID_PNG.len() as u64)
            ),
            ObservationChunkOutcome::Decode(EvidenceDecode {
                key: (10, 0),
                request_id: 14,
                format: gpui::ImageFormat::Png,
                bytes: VALID_PNG.to_vec(),
            })
        );
        state.evidence_selected = Some((10, 0));
        assert!(state.apply_observation_decode_failed(
            "s1",
            14,
            (10, 0),
            t("computer.observation_too_large").to_string()
        ));
        assert!(state.evidence_decoding.is_none());
        assert_eq!(
            state.evidence_error.as_deref(),
            Some(t("computer.observation_too_large"))
        );
        // 解码期间切到别的证据：旧身份的失败不污染当前选择。
        state.evidence_error = None;
        state.evidence_decoding = Some(((10, 0), 15));
        state.evidence_selected = Some((11, 0));
        assert!(state.apply_observation_decode_failed(
            "s1",
            15,
            (10, 0),
            t("computer.observation_corrupt").to_string()
        ));
        assert!(state.evidence_decoding.is_none());
        assert!(state.evidence_error.is_none());

        // 魔数嗅探不过（eof 收齐但非图像字节）→ fail-closed。
        state.evidence_error = None;
        let garbage = b"not-an-image".to_vec();
        state.evidence_load = Some(EvidenceLoad {
            key: (9, 0),
            request_id: 13,
            next_offset: 0,
            received: 0,
            total: garbage.len() as u64,
            buffer: Vec::new(),
        });
        assert_eq!(
            state.apply_observation_chunk(
                "s1",
                13,
                &observation_chunk(9, 0, 0, &encode(&garbage), true, garbage.len() as u64)
            ),
            ObservationChunkOutcome::Settled
        );
        assert!(!state.evidence_images.contains_key(&(9, 0)));
        assert!(state.evidence_load.is_none());
        assert!(state.evidence_error.is_some());

        // 偏移跳变 → fail-closed。
        state.evidence_error = None;
        state.evidence_load = Some(EvidenceLoad {
            key: (6, 0),
            request_id: 10,
            next_offset: 0,
            received: 0,
            total: 8,
            buffer: Vec::new(),
        });
        assert_eq!(
            state.apply_observation_chunk("s1", 10, &observation_chunk(6, 0, 4, &first, false, 70)),
            ObservationChunkOutcome::Settled
        );
        assert!(state.evidence_load.is_none());
        assert!(state.evidence_error.is_some());

        // 非法 base64 → fail-closed。
        state.evidence_error = None;
        state.evidence_load = Some(EvidenceLoad {
            key: (7, 0),
            request_id: 11,
            next_offset: 0,
            received: 0,
            total: 4,
            buffer: Vec::new(),
        });
        assert_eq!(
            state.apply_observation_chunk("s1", 11, &observation_chunk(7, 0, 0, "!!!!", false, 4)),
            ObservationChunkOutcome::Settled
        );
        assert!(state.evidence_load.is_none());
        assert!(state.evidence_error.is_some());

        // eof 时字节数与申报不符 → fail-closed。
        state.evidence_error = None;
        state.evidence_load = Some(EvidenceLoad {
            key: (8, 0),
            request_id: 12,
            next_offset: 0,
            received: 0,
            total: 16,
            buffer: Vec::new(),
        });
        assert_eq!(
            state.apply_observation_chunk("s1", 12, &observation_chunk(8, 0, 0, &first, true, 16)),
            ObservationChunkOutcome::Settled
        );
        assert!(state.evidence_load.is_none());
        assert!(state.evidence_error.is_some());
    }

    /// 解码回执与当前选择的竞态：A 解码期间选中 B（被单槽在途挡住）
    /// → A 的成功按身份入缓存、不产生错误，回执落地后自动续载 B
    ///（以 B 超字节预算被本地拒收为证：续载确实发生且错误只属于 B）。
    #[gpui::test]
    fn evidence_decode_receipt_isolated_and_loads_selection(cx: &mut gpui::TestAppContext) {
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("cu19-decode-receipt.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                view.projection.active_session_id = Some("s1".into());
                let request = view.computer.begin_loading("s1");
                assert!(view.computer.apply_loaded("s1", request, list_data("s1", "r1")));
                let mut oversize = observation_entry(2, 0, Some("r1"));
                oversize.byte_length = OBSERVATION_IMAGE_MAX_BYTES + 1;
                view.computer.observations = vec![oversize, observation_entry(1, 0, Some("r1"))];
                view.computer.obs_fetch = ComputerFetch::Ready;
                // A 已收齐移交解码；用户切到 B（在途解码挡住读取）。
                view.computer.evidence_decoding = Some(((1, 0), 42));
                view.computer.evidence_selected = Some((2, 0));
                view.computer_finish_evidence_decode("s1", 42, (1, 0), Ok(VALID_PNG.to_vec()), cx);
                assert!(
                    view.computer.evidence_images.contains_key(&(1, 0)),
                    "A 的解码成功按身份入缓存"
                );
                assert!(view.computer.evidence_decoding.is_none());
                assert_eq!(
                    view.computer.evidence_error.as_deref(),
                    Some(t("computer.observation_too_large")),
                    "回执落地后自动续载当前选中 B（超预算本地拒收即证续载发生）"
                );
                assert!(view.computer.evidence_load.is_none());
            });
        });
    }

    /// 后台解码校验：压缩字节预算之外按实际尺寸限制单帧像素（小体积
    /// 大尺寸图不放大解码内存）；GIF 与 GPUI 渲染同口径解码全部帧——
    /// 首帧有效、后续帧损坏的 GIF 不得放行——并限制帧数与累计像素。
    #[test]
    fn observation_image_validation_budgets_and_full_frame_gif() {
        // 有效 PNG / 双帧 GIF 通过。
        assert!(validate_observation_image(gpui::ImageFormat::Png, VALID_PNG.to_vec()).is_ok());
        let valid_gif: &[u8] = &[
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x81, 0x00, 0x00,
            0xC8, 0xC8, 0xC8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21,
            0xFF, 0x0B, 0x4E, 0x45, 0x54, 0x53, 0x43, 0x41, 0x50, 0x45, 0x32, 0x2E, 0x30,
            0x03, 0x01, 0x00, 0x00, 0x00, 0x21, 0xF9, 0x04, 0x00, 0x01, 0x00, 0x00, 0x00,
            0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x08, 0x04, 0x00,
            0x01, 0x04, 0x04, 0x00, 0x21, 0xF9, 0x04, 0x01, 0x01, 0x00, 0x01, 0x00, 0x2C,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x81, 0x64, 0x64, 0x64, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x04, 0x00, 0x01, 0x04,
            0x04, 0x00, 0x3B,
        ];
        assert!(
            validate_observation_image(gpui::ImageFormat::Gif, valid_gif.to_vec()).is_ok(),
            "双帧均有效的 GIF 通过全帧校验"
        );
        // 首帧有效、第二帧 LZW 数据损坏：只解首帧会误放行，全帧校验
        //（与 GPUI 渲染一致）必须拒绝。
        let corrupt_gif: Vec<u8> = {
            let mut gif = valid_gif.to_vec();
            let lzw = [0x08u8, 0x04, 0x00, 0x01, 0x04, 0x04, 0x00];
            let second = gif
                .windows(lzw.len())
                .rposition(|window| window == lzw)
                .expect("second frame LZW block");
            for byte in &mut gif[second + 2..second + 6] {
                *byte = 0xFF;
            }
            gif
        };
        assert_eq!(
            validate_observation_image(gpui::ImageFormat::Gif, corrupt_gif),
            Err(ObservationDecodeError::Decode),
            "首帧有效、后续帧损坏的 GIF 不得入缓存"
        );
        // 帧数预算：60 帧通过，61 帧拒绝（累计解码内存有界）。
        let gif_frames = |count: usize| {
            let mut bytes = Vec::new();
            {
                let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
                for i in 0..count {
                    let buffer = image::RgbaImage::from_pixel(1, 1, image::Rgba([i as u8, 0, 0, 255]));
                    encoder
                        .encode_frame(image::Frame::from_parts(
                            buffer,
                            0,
                            0,
                            image::Delay::from_numer_denom_ms(10, 1),
                        ))
                        .expect("encode gif frame");
                }
            }
            bytes
        };
        assert!(validate_observation_image(gpui::ImageFormat::Gif, gif_frames(OBSERVATION_IMAGE_MAX_FRAMES as usize)).is_ok());
        assert_eq!(
            validate_observation_image(gpui::ImageFormat::Gif, gif_frames(OBSERVATION_IMAGE_MAX_FRAMES as usize + 1)),
            Err(ObservationDecodeError::Budget)
        );
        // 静图像素预算：小体积大尺寸（IHDR 声明 100000×100000）先查
        // 头部尺寸再拒绝，不进入完整解码分配。
        fn crc32(bytes: &[u8]) -> u32 {
            let mut value = 0xFFFF_FFFFu32;
            for &byte in bytes {
                value ^= u32::from(byte);
                for _ in 0..8 {
                    let lsb = value & 1;
                    value >>= 1;
                    if lsb == 1 {
                        value ^= 0xEDB8_8320;
                    }
                }
            }
            !value
        }
        let mut huge = VALID_PNG;
        huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        let ihdr_crc = crc32(&huge[12..29]).to_be_bytes();
        huge[29..33].copy_from_slice(&ihdr_crc);
        assert_eq!(
            validate_observation_image(gpui::ImageFormat::Png, huge.to_vec()),
            Err(ObservationDecodeError::Budget)
        );
        // 帧体损坏（IDAT 字节翻转）→ 真实解码失败拒绝。
        let mut broken = VALID_PNG;
        broken[45] ^= 0xFF;
        assert_eq!(
            validate_observation_image(gpui::ImageFormat::Png, broken.to_vec()),
            Err(ObservationDecodeError::Decode)
        );
    }

    /// 停止状态机：回执（Ok / Err）都只进入确认；已停止只能由 run 终态
    /// 事件确认（空列表不裁决释放——Host 取消即移除 registry 登记，
    /// 清理可能仍在执行）；点击与回执永不直接显示成功。
    #[test]
    fn computer_stop_requires_real_release_or_terminal_state() {
        let mut state = ComputerPanelState::default();
        let request = state.begin_loading("s1");
        assert!(state.apply_loaded("s1", request, list_data("s1", "r1")));
        state.stop = Some(ComputerStop {
            key: "k".into(),
            run_id: "r1".into(),
            action_id: 7,
            phase: ComputerStopPhase::Dispatching,
        });
        // 代次不匹配的迟到回执不收敛。
        assert!(!state.finish_stop("s1", 6, "r1", Ok(())));
        assert_eq!(state.stop.as_ref().unwrap().phase, ComputerStopPhase::Dispatching);
        // Ok 回执只进入确认；目标仍被本 run 持有 → 不得显示已停止。
        assert!(state.finish_stop("s1", 7, "r1", Ok(())));
        assert_eq!(
            state.stop.as_ref().unwrap().phase,
            ComputerStopPhase::Confirming { note: None }
        );
        let request = state.begin_loading("s1");
        assert!(state.apply_loaded("s1", request, list_data("s1", "r1")));
        assert_eq!(
            state.stop.as_ref().unwrap().phase,
            ComputerStopPhase::Confirming { note: None },
            "仍被持有时不得标记已停止"
        );
        // 空列表 ≠ 已释放：Host 取消即移除 registry 登记，清理可能仍在
        // 执行 → 仍停在确认，不得冒充已停止。
        let request = state.begin_loading("s1");
        assert!(state.apply_loaded(
            "s1",
            request,
            serde_json::from_value(serde_json::json!({
                "session_id": "s1",
                "workspace_id": "ws-1",
                "targets": [],
            }))
            .expect("empty list shape")
        ));
        assert_eq!(
            state.stop.as_ref().unwrap().phase,
            ComputerStopPhase::Confirming { note: None },
            "空列表不是释放事实，不得标记已停止"
        );
        // run 终态事件（唯一确认通道）→ 已停止；随后迟到回执不回退。
        state.confirm_stop_terminal("r1");
        assert_eq!(state.stop.as_ref().unwrap().phase, ComputerStopPhase::Stopped);
        assert!(!state.finish_stop("s1", 7, "r1", Ok(())));
        assert_eq!(state.stop.as_ref().unwrap().phase, ComputerStopPhase::Stopped);
        // 确认前断线 → 结果未知；run 终态事件到达 → 已停止。
        state.stop = Some(ComputerStop {
            key: "k".into(),
            run_id: "r2".into(),
            action_id: 8,
            phase: ComputerStopPhase::Dispatching,
        });
        state.mark_stale("connection lost");
        assert_eq!(state.stop.as_ref().unwrap().phase, ComputerStopPhase::Unknown);
        state.confirm_stop_terminal("r2");
        assert_eq!(state.stop.as_ref().unwrap().phase, ComputerStopPhase::Stopped);
        // 确认查询失败 → 派发后观测失败；终态事件仍会收敛为已停止。
        state.stop = Some(ComputerStop {
            key: "k".into(),
            run_id: "r3".into(),
            action_id: 9,
            phase: ComputerStopPhase::Confirming { note: None },
        });
        let failed_request = state.begin_loading("s1");
        state.apply_failed("s1", failed_request, "query failed".into());
        assert_eq!(
            state.stop.as_ref().unwrap().phase,
            ComputerStopPhase::ObserveFailed {
                reason: "query failed".into()
            }
        );
        state.confirm_stop_terminal("r3");
        assert_eq!(state.stop.as_ref().unwrap().phase, ComputerStopPhase::Stopped);
        // 取消被拒且仍被持有 → 回落为操作错误并允许重试（不冒充停止）。
        state.stop = Some(ComputerStop {
            key: "k".into(),
            run_id: "r4".into(),
            action_id: 10,
            phase: ComputerStopPhase::Confirming {
                note: Some("unknown_run".into()),
            },
        });
        let request = state.begin_loading("s1");
        assert!(state.apply_loaded("s1", request, list_data("s1", "r4")));
        assert!(state.stop.is_none());
        assert_eq!(state.action_error.as_deref(), Some("unknown_run"));
        // 已停止记录在新 run 接管面板时退役（不跨 run 残留）。
        state.stop = Some(ComputerStop {
            key: "k".into(),
            run_id: "r5".into(),
            action_id: 11,
            phase: ComputerStopPhase::Stopped,
        });
        let request = state.begin_loading("s1");
        assert!(state.apply_loaded("s1", request, list_data("s1", "r6")));
        assert!(state.stop.is_none(), "新 run 接管时已停止记录退役");
    }

    /// AX 主路径：行 / 按钮按实测几何发布；无 client 的测试环境授权
    /// 动作禁用（gate 含 Host 支持判定），被拒的 Press 不产生在途 /
    /// 错误状态。
    #[gpui::test]
    fn computer_panel_ax_publishes_targets_and_gates_actions(cx: &mut gpui::TestAppContext) {
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("cu18-computer-panel.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        let app_key = target_key(&fixture_targets("r1")[0]);
        let site_key = target_key(&fixture_targets("r1")[1]);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                view.projection.active_session_id = Some("s1".into());
                view.projection.active_run_id = Some("r1".into());
                let request_id = view.computer.begin_loading("s1");
                assert!(view.computer.apply_loaded("s1", request_id, list_data("s1", "r1")));
                view.inspector_open = true;
                view.inspector_motion.width(
                    true,
                    true,
                    std::time::Instant::now() - std::time::Duration::from_secs(1),
                );
                view.inspector_tab = crate::ui::inspector::InspectorTab::Computer;
                view.close_open_menu(cx);
                cx.notify();
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                tree.validate().unwrap();
                let group = tree.find("computer").expect("computer group");
                assert!(
                    group
                        .description
                        .as_deref()
                        .unwrap_or_default()
                        .contains("2 targets"),
                    "group description carries fetch state: {:?}",
                    group.description
                );
                let app_row_id = dynamic_identifier("computer-target", &app_key);
                let app_row = tree.find(&app_row_id).expect("application row");
                assert_eq!(app_row.label, "com.apple.TextEdit");
                let approve_id = action_id(COMPUTER_APPROVE_ONCE_PREFIX, &app_key);
                let approve = tree.find(&approve_id).expect("approve once button");
                assert!(!approve.enabled, "无 client 时授权动作禁用");
                // 撤销按钮与授权按钮同一 gate（撤销不要求仍存在 grant：
                // 一次授权消耗后许可与占用可能仍在，revoke 即目标级停止）。
                let revoke_site_id = action_id(COMPUTER_REVOKE_PREFIX, &site_key);
                assert!(tree
                    .find(&revoke_site_id)
                    .is_some_and(|node| !node.enabled));
                // 禁用节点的 Press 被 permits 复核拒绝（
                // handle_accessibility_request 内部按当前树重核），不产生
                // 任何在途 / 错误状态。
                let request = crate::ui::accessibility::AxRequest {
                    identifier: approve_id,
                    action: AxAction::Press,
                    value: None,
                };
                view.handle_accessibility_request(request, window, cx);
                assert!(view.computer.action_in_flight.is_none());
                assert!(view.computer.action_error.is_none());
            });
        });
        // 查询失败态：AX 与渲染共用列表可见条件——行与按钮不再发布，
        // 凭旧 id 的迟到 Press 不派出（隐藏按钮不可经 AX 绕过）。
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.computer.fetch = ComputerFetch::Failed;
                view.computer.fetch_error = Some("boom".into());
                cx.notify();
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                tree.validate().unwrap();
                let app_row_id = dynamic_identifier("computer-target", &app_key);
                let approve_id = action_id(COMPUTER_APPROVE_ONCE_PREFIX, &app_key);
                assert!(tree.find(&app_row_id).is_none(), "失败态不发布目标行");
                assert!(tree.find(&approve_id).is_none(), "失败态不发布动作按钮");
                let request = crate::ui::accessibility::AxRequest {
                    identifier: approve_id,
                    action: AxAction::Press,
                    value: None,
                };
                view.handle_accessibility_request(request, window, cx);
                assert!(view.computer.action_in_flight.is_none());
                assert!(view.computer.action_error.is_none());
            });
        });
    }

    /// CU-19 AX 主路径：观测行与停止按钮按实测几何发布；无 client 的
    /// 测试环境停止按钮禁用、被拒的 Press 不产生停止状态；行 Press 走
    /// 同一选择入口（无 client 时如实记录不可加载，不崩溃）。
    #[gpui::test]
    fn computer_panel_ax_publishes_observations_and_stop_gating(
        cx: &mut gpui::TestAppContext,
    ) {
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("cu19-computer-panel.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        let app_key = target_key(&fixture_targets("r1")[0]);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                view.projection.active_session_id = Some("s1".into());
                view.projection.active_run_id = Some("r1".into());
                let request_id = view.computer.begin_loading("s1");
                assert!(view.computer.apply_loaded("s1", request_id, list_data("s1", "r1")));
                let obs_request = view.computer.begin_observation_loading();
                view.computer.note_observation_dispatch(false);
                assert_eq!(
                    view.computer.apply_observations(
                        "s1",
                        obs_request,
                        &observation_list_data(
                            vec![
                                observation_entry(990, 0, Some("r1")),
                                observation_entry(800, 0, Some("r1")),
                            ],
                            990,
                            true,
                            None
                        )
                    ),
                    None
                );
                view.inspector_open = true;
                view.inspector_motion.width(
                    true,
                    true,
                    std::time::Instant::now() - std::time::Duration::from_secs(1),
                );
                view.inspector_tab = crate::ui::inspector::InspectorTab::Computer;
                view.close_open_menu(cx);
                cx.notify();
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                tree.validate().unwrap();
                let group = tree.find("computer").expect("computer group");
                let description = group.description.as_deref().unwrap_or_default();
                assert!(
                    description.contains("2 observations"),
                    "group carries observation count: {description}"
                );
                // 观测行：最新（990）与历史（800）都按实测几何发布，
                // value 携带 run 标识（观测自带事实，不与目标卡对齐）。
                let newest_id = observation_row_id((990, 0));
                let newest = tree.find(&newest_id).expect("newest observation row");
                assert!(newest.label.contains("computer"));
                assert!(newest
                    .value
                    .as_deref()
                    .unwrap_or_default()
                    .contains("run r1"));
                assert!(tree.find(&observation_row_id((800, 0))).is_some());
                // 停止按钮与授权按钮同 gate：无 client 时禁用。
                let stop_id = action_id(COMPUTER_STOP_PREFIX, &app_key);
                let stop = tree.find(&stop_id).expect("stop button");
                assert!(!stop.enabled, "无 client 时停止按钮禁用");
                let request = crate::ui::accessibility::AxRequest {
                    identifier: stop_id,
                    action: AxAction::Press,
                    value: None,
                };
                view.handle_accessibility_request(request, window, cx);
                assert!(view.computer.stop.is_none(), "禁用 Press 不产生停止状态");
                // 行 Press（enabled）走选择入口：无 client 时加载不可用
                // 如实记录错误，不崩溃、不冒充已加载。
                let request = crate::ui::accessibility::AxRequest {
                    identifier: newest_id,
                    action: AxAction::Press,
                    value: None,
                };
                view.handle_accessibility_request(request, window, cx);
                assert_eq!(view.computer.evidence_selected, Some((990, 0)));
                assert!(view.computer.evidence_error.is_some());
            });
        });
    }
}
