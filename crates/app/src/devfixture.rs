//! R1 Wave B UI fixture 种子器（dev-only，非公开 API）。
//!
//! 本模块只服务 `examples/ui_fixture.rs` 与集成测试：把声明式数据集
//!（`fixtures/ui/seed.json`，schema v1）经 **SessionStore / CheckpointService
//! 公开 API** 与真实文件系统 / git 写入隔离的 fixture root，供 GUI fixture
//! host 以真实协议路径重放。它不依赖 `pawork-testkit`（testkit 永不进
//! 生产闭包），也不被任何生产模块引用。
//!
//! 确定性口径：固定 `now_ms` 下，事件 payload 字节与 snapshot-dump 输出
//! 逐字节一致；sqlite / checkpoint 状态文件本身不承诺逐字节稳定。

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use pawork_domain::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, ArtifactId, ContentPart, ErrorCategory,
    ErrorContext, EventId, EventSequence, Message, MessageId, MessageMetadata, MessageRole,
    ProviderId, RequestId, RunId, SessionId, StopReason, TextContent, Timestamp, TokenUsage,
    ToolCallId, ToolOutputStream, ToolResultContent, WorkspaceId,
};
use pawork_storage::blob::{ArtifactStore, CheckpointService};
use pawork_policy::ApprovalMode;
use pawork_storage::session::SessionStore;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::extensions::McpServerSlot;
use crate::services::extension::ExtensionService;
use crate::{AppCore, AppError, DenyAllApprovals};
use pawork_workspace::WorkspaceService;

/// 固定时间锚点：2026-01-01T00:00:00Z。
pub const FIXTURE_NOW_MS: i64 = 1_767_225_600_000;
/// fixture root 标记文件：clean / 整体重建只认它，防误删。
pub const FIXTURE_MARKER_FILE: &str = ".pawork-ui-fixture";
#[cfg(unix)]
const MAX_FIXTURE_SOCKET_PATH_BYTES: usize = 103;
const MAX_FIXTURE_TIMESTAMP_MS: i64 = 253_402_300_799_999;

/// seed.json schema v1（声明式数据集定义）。
#[derive(Clone, Debug, Deserialize)]
pub struct SeedSpec {
    pub fixture_version: u32,
    pub now_ms: i64,
    #[serde(default)]
    pub workspaces: Vec<SeedWorkspace>,
    #[serde(default)]
    pub sessions: Vec<SeedSession>,
    #[serde(default)]
    pub diffs: Vec<SeedDiff>,
    #[serde(default)]
    pub pty: Option<SeedPty>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SeedWorkspace {
    pub id: String,
    pub name: String,
    /// 含 `${ROOT}` 占位符的绝对路径。
    pub path: String,
    #[serde(default)]
    pub git: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SeedSession {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub created_offset_ms: i64,
    pub updated_offset_ms: i64,
    pub state: String,
    #[serde(default)]
    pub turns: Vec<SeedTurn>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SeedTurn {
    pub user: String,
    #[serde(default)]
    pub assistant: Vec<String>,
    #[serde(default = "default_stream_chunks")]
    pub stream_chunks: usize,
    #[serde(default)]
    pub tools: Vec<SeedTool>,
    #[serde(default)]
    pub usage: SeedUsage,
    pub stop: String,
}

fn default_stream_chunks() -> usize {
    3
}

#[derive(Clone, Debug, Deserialize)]
pub struct SeedTool {
    pub name: String,
    /// pending | running | succeeded | failed。
    pub status: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SeedUsage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SeedDiff {
    pub workspace_id: String,
    pub session_id: String,
    pub files: Vec<SeedDiffFile>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SeedDiffFile {
    pub path: String,
    /// modified | added | deleted。
    pub action: String,
    #[serde(default)]
    pub long_line: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SeedPty {
    pub script: String,
}

/// serve / snapshot-dump 复用的 workspace 装配条目。
#[derive(Clone, Debug)]
pub struct FixtureWorkspace {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

/// seed 结果摘要。
#[derive(Clone, Debug)]
pub struct SeedOutcome {
    pub root: PathBuf,
    pub now_ms: i64,
    pub workspaces: usize,
    pub sessions: usize,
    pub events: usize,
    pub checkpoints: usize,
    pub manifest: PathBuf,
}

/// `ui_fixture serve --profile` 的 dev-only Host 装配档。
///
/// profile 只改变隔离 fixture Host 的审批/trust 与 MCP 状态输入，不进入
/// seed 数据库、生产配置或 GUI wire。R6 用三个互斥档分别证明终端成功、
/// Resources ready/failed 与 read_only fail-closed，避免把互相冲突的状态
/// 伪造在同一 Host 生命周期里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixtureHostProfile {
    Default,
    R6Terminal,
    R6Resources,
    R6ReadOnly,
}

impl FixtureHostProfile {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "default" => Ok(Self::Default),
            "r6-terminal" => Ok(Self::R6Terminal),
            "r6-resources" => Ok(Self::R6Resources),
            "r6-read-only" => Ok(Self::R6ReadOnly),
            other => Err(format!(
                "未知 UI fixture Host profile {other:?}（可选 default、r6-terminal、r6-resources、r6-read-only）"
            )),
        }
    }

    fn approval(self) -> Option<(ApprovalMode, bool)> {
        match self {
            Self::Default => None,
            Self::R6Terminal | Self::R6Resources => {
                Some((ApprovalMode::AskForDangerous, true))
            }
            Self::R6ReadOnly => Some((ApprovalMode::ReadOnly, true)),
        }
    }
}

fn fixture_mcp_slots(profile: FixtureHostProfile) -> Vec<McpServerSlot> {
    if profile != FixtureHostProfile::R6Resources {
        return Vec::new();
    }
    vec![
        McpServerSlot {
            name: "fixture-files".into(),
            transport: "stdio".into(),
            state: "connected".into(),
            last_error: None,
            tools: vec!["fixture_files.read".into(), "fixture_files.list".into()],
            client: None,
        },
        McpServerSlot {
            name: "fixture-broken".into(),
            transport: "stdio".into(),
            state: "failed".into(),
            last_error: Some("fixture scripted MCP startup failure".into()),
            tools: Vec::new(),
            client: None,
        },
    ]
}

/// 在 attach workspace 之前应用选定的 dev-only Host profile。
pub fn configure_fixture_host_profile(
    core: &mut AppCore,
    profile: &str,
) -> Result<FixtureHostProfile, String> {
    let profile = FixtureHostProfile::parse(profile)?;
    if let Some((mode, trusted)) = profile.approval() {
        core.configure_approval(mode, trusted, Arc::new(DenyAllApprovals));
    }
    core.extensions.mcp_servers = fixture_mcp_slots(profile);
    Ok(profile)
}

/// 校验 fixture root：只接受隔离目录，拒绝默认数据目录、仓库与其
/// 祖先/内部。比较前解析现存祖先与 symlink，避免词法前缀绕过。
pub fn validate_root(root: &Path) -> Result<(), String> {
    if !root.is_absolute() {
        return Err(format!("fixture root 必须是绝对路径：{}", root.display()));
    }
    if root
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(format!(
            "fixture root 不得含 . 或 .. 路径段：{}",
            root.display()
        ));
    }
    let root = resolve_for_compare(root)?;
    let data_dir = resolve_for_compare(&absolute_path(crate::default_data_dir())?)?;
    let repo_root = resolve_for_compare(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))?;
    if root == Path::new("/") {
        return Err("拒绝把 / 作为 fixture root".into());
    }
    if root == data_dir || root.starts_with(&data_dir) {
        return Err(format!(
            "fixture root 不得位于默认数据目录内：{}（data dir {}）",
            root.display(),
            data_dir.display()
        ));
    }
    if data_dir.starts_with(&root) {
        return Err(format!(
            "fixture root 不得包含默认数据目录：{}（data dir {}）",
            root.display(),
            data_dir.display()
        ));
    }
    if root == repo_root || root.starts_with(&repo_root) || repo_root.starts_with(&root) {
        return Err(format!(
            "fixture root 不得位于仓库内或包含仓库：{}（repo {}）",
            root.display(),
            repo_root.display()
        ));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let home = resolve_for_compare(&absolute_path(home)?)?;
        if root == home || home.starts_with(&root) {
            return Err(format!(
                "fixture root 不得是（或包含）用户 home：{}",
                root.display()
            ));
        }
    }
    #[cfg(unix)]
    {
        let socket_path = root.join("data/pawork-gui.sock");
        let socket_path_len = socket_path.to_string_lossy().as_bytes().len();
        if socket_path_len > MAX_FIXTURE_SOCKET_PATH_BYTES {
            return Err(format!(
                "fixture Unix socket 路径过长（{socket_path_len} bytes，最大 {MAX_FIXTURE_SOCKET_PATH_BYTES}）：{}；请改用 /tmp 下的短 root",
                socket_path.display()
            ));
        }
    }
    Ok(())
}

fn absolute_path(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|current| current.join(path))
        .map_err(|error| format!("解析绝对路径失败：{error}"))
}

/// canonicalize 不接受尚不存在的最终路径；因此先找到最近的现存祖先，
/// 解析其 symlink，再逐段补回缺失后缀。
fn resolve_for_compare(path: &Path) -> Result<PathBuf, String> {
    let mut cursor = path;
    let mut suffix: Vec<OsString> = Vec::new();
    while !cursor.exists() {
        let name = cursor
            .file_name()
            .ok_or_else(|| format!("无法解析 fixture 路径的现存祖先：{}", path.display()))?;
        suffix.push(name.to_os_string());
        cursor = cursor
            .parent()
            .ok_or_else(|| format!("无法解析 fixture 路径的父目录：{}", path.display()))?;
    }
    let mut resolved = std::fs::canonicalize(cursor)
        .map_err(|error| format!("解析路径 {} 失败：{error}", cursor.display()))?;
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn safe_relative_path(value: &str, label: &str) -> Result<PathBuf, String> {
    if value.is_empty() {
        return Err(format!("{label} 不得为空"));
    }
    let mut relative = PathBuf::new();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            _ => {
                return Err(format!("{label} 必须是无 . / .. 的相对路径：{value:?}"));
            }
        }
    }
    if relative.as_os_str().is_empty() {
        return Err(format!("{label} 不得为空"));
    }
    Ok(relative)
}

fn workspace_relative_path(workspace: &SeedWorkspace) -> Result<PathBuf, String> {
    let relative = workspace.path.strip_prefix("${ROOT}/").ok_or_else(|| {
        format!(
            "workspace {} 路径必须以 ${{ROOT}}/ 开头：{}",
            workspace.id, workspace.path
        )
    })?;
    if relative.contains("${ROOT}") {
        return Err(format!(
            "workspace {} 路径只能含一个起始 ${{ROOT}} 占位符：{}",
            workspace.id, workspace.path
        ));
    }
    safe_relative_path(relative, &format!("workspace {} 路径", workspace.id))
}

/// 在任何文件系统写入前校验 seed schema 的引用、枚举与全部相对路径。
pub fn validate_spec(spec: &SeedSpec) -> Result<(), String> {
    if spec.fixture_version != 1 {
        return Err(format!(
            "不支持的 fixture_version {}（期望 1）",
            spec.fixture_version
        ));
    }
    if spec.workspaces.is_empty() || spec.sessions.is_empty() {
        return Err("fixture 至少需要一个 workspace 与一个 session".into());
    }

    let mut workspace_ids = BTreeMap::new();
    let mut workspace_paths = BTreeSet::new();
    for workspace in &spec.workspaces {
        if workspace.id.is_empty() {
            return Err("workspace id 不得为空".into());
        }
        if workspace_ids
            .insert(workspace.id.as_str(), workspace.git)
            .is_some()
        {
            return Err(format!("重复 workspace id {}", workspace.id));
        }
        let path = workspace_relative_path(workspace)?;
        if !workspace_paths.insert(path) {
            return Err(format!("workspace {} 路径重复", workspace.id));
        }
    }

    let mut session_workspaces = BTreeMap::new();
    let mut successful_writes: BTreeMap<(String, String), usize> = BTreeMap::new();
    for session in &spec.sessions {
        if session.id.is_empty() {
            return Err("session id 不得为空".into());
        }
        if session_workspaces
            .insert(session.id.as_str(), session.workspace_id.as_str())
            .is_some()
        {
            return Err(format!("重复 session id {}", session.id));
        }
        if !workspace_ids.contains_key(session.workspace_id.as_str()) {
            return Err(format!(
                "session {} 引用未知 workspace {}",
                session.id, session.workspace_id
            ));
        }
        if session.updated_offset_ms <= session.created_offset_ms {
            return Err(format!(
                "session {} updated_offset 必须晚于 created_offset",
                session.id
            ));
        }
        if !matches!(
            session.state.as_str(),
            "completed" | "failed" | "cancelled" | "pending_approval" | "tool_failed"
        ) {
            return Err(format!(
                "未知 session state {:?}（session {}）",
                session.state, session.id
            ));
        }
        for (turn_index, turn) in session.turns.iter().enumerate() {
            if !matches!(turn.stop.as_str(), "completed" | "failed" | "cancelled") {
                return Err(format!(
                    "未知 turn stop {:?}（session {} turn {}）",
                    turn.stop, session.id, turn_index
                ));
            }
            for (tool_index, tool) in turn.tools.iter().enumerate() {
                if !matches!(
                    tool.status.as_str(),
                    "pending" | "running" | "succeeded" | "failed"
                ) {
                    return Err(format!(
                        "未知 tool status {:?}（session {} turn {} tool {}）",
                        tool.status, session.id, turn_index, tool_index
                    ));
                }
                if matches!(tool.status.as_str(), "succeeded" | "failed") && tool.path.is_none() {
                    return Err(format!(
                        "session {} turn {} tool {}（{}）缺少 path",
                        session.id, turn_index, tool_index, tool.name
                    ));
                }
                if let Some(path) = tool.path.as_deref() {
                    safe_relative_path(
                        path,
                        &format!(
                            "session {} turn {} tool {} path",
                            session.id, turn_index, tool_index
                        ),
                    )?;
                    if tool.status == "succeeded" && is_write_tool(&tool.name) {
                        *successful_writes
                            .entry((session.id.clone(), path.to_string()))
                            .or_default() += 1;
                    }
                }
            }
        }
    }

    let mut diff_keys = BTreeSet::new();
    for diff in &spec.diffs {
        let Some(session_workspace) = session_workspaces.get(diff.session_id.as_str()) else {
            return Err(format!("diff 引用未知 session {}", diff.session_id));
        };
        if !workspace_ids.contains_key(diff.workspace_id.as_str()) {
            return Err(format!("diff 引用未知 workspace {}", diff.workspace_id));
        }
        if *session_workspace != diff.workspace_id {
            return Err(format!(
                "diff session {} 属于 workspace {}，不能写入 {}",
                diff.session_id, session_workspace, diff.workspace_id
            ));
        }
        if workspace_ids.get(diff.workspace_id.as_str()) != Some(&true) {
            return Err(format!(
                "diff workspace {} 必须启用 git 基线",
                diff.workspace_id
            ));
        }
        for file in &diff.files {
            safe_relative_path(&file.path, &format!("diff {} path", diff.session_id))?;
            if !matches!(file.action.as_str(), "modified" | "added" | "deleted") {
                return Err(format!("未知 diff action {:?}", file.action));
            }
            let key = (diff.session_id.clone(), file.path.clone());
            if !diff_keys.insert(key.clone()) {
                return Err(format!(
                    "重复 diff 路径 {}（session {}）",
                    file.path, diff.session_id
                ));
            }
            if successful_writes.get(&key) != Some(&1) {
                return Err(format!(
                    "diff {}:{} 必须恰由一个 succeeded 写工具生成",
                    diff.session_id, file.path
                ));
            }
        }
    }
    if let Some(pty) = &spec.pty {
        safe_relative_path(&pty.script, "pty.script")?;
    }
    Ok(())
}

/// 校验可由 CLI 覆盖的绝对时间锚点；必须在 prepare_root 前完成，避免
/// 极值 i64 在 git 日期或 session 时间戳运算中溢出并留下半成品。
fn validate_seed_timestamps(spec: &SeedSpec, now_ms: i64) -> Result<(), String> {
    let baseline_ms = now_ms
        .checked_sub(86_400_000)
        .ok_or("fixture now_ms 计算 git 基线日期时溢出")?;
    validate_timestamp_range(baseline_ms, "git baseline")?;
    for session in &spec.sessions {
        let created = now_ms
            .checked_add(session.created_offset_ms)
            .ok_or_else(|| format!("session {} created_at 溢出", session.id))?;
        let updated = now_ms
            .checked_add(session.updated_offset_ms)
            .ok_or_else(|| format!("session {} updated_at 溢出", session.id))?;
        validate_timestamp_range(created, &format!("session {} created_at", session.id))?;
        validate_timestamp_range(updated, &format!("session {} updated_at", session.id))?;
    }
    Ok(())
}

fn validate_timestamp_range(value: i64, label: &str) -> Result<(), String> {
    if !(0..=MAX_FIXTURE_TIMESTAMP_MS).contains(&value) {
        return Err(format!(
            "{label} 超出支持范围 1970-01-01..9999-12-31：{value}"
        ));
    }
    Ok(())
}

pub fn marker_path(root: &Path) -> PathBuf {
    root.join(FIXTURE_MARKER_FILE)
}

pub fn fixture_marker_present(root: &Path) -> bool {
    marker_path(root).is_file()
}

/// 只有 seed 完整收口后写出的 ready marker 才允许 serve / self-check。
pub fn fixture_marker_ready(root: &Path) -> bool {
    std::fs::read(marker_path(root))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|marker| marker.get("state").and_then(Value::as_str) == Some("ready"))
}

/// 把 seed.json 的 `${ROOT}` 占位符解析为真实 workspace 装配条目。
pub fn resolve_workspaces(spec: &SeedSpec, root: &Path) -> Result<Vec<FixtureWorkspace>, String> {
    spec.workspaces
        .iter()
        .map(|workspace| {
            let relative = workspace_relative_path(workspace)?;
            Ok(FixtureWorkspace {
                id: workspace.id.clone(),
                name: workspace.name.clone(),
                path: root.join(relative),
            })
        })
        .collect()
}

/// 把 seed 声明的 workspace 全量装配进 AppCore（多 workspace 注册表 +
/// 内建工具 + 资源加载器）。生产 `attach_workspace` 只支持单 root，
/// fixture 需要一次登记三个 workspace。
pub fn attach_fixture_workspaces(
    core: &mut AppCore,
    entries: &[FixtureWorkspace],
) -> Result<(), AppError> {
    let Some(primary) = entries.first() else {
        return Ok(());
    };
    let service = WorkspaceService::new();
    for entry in entries {
        service.add(
            WorkspaceId::from(entry.id.as_str()),
            entry.name.as_str(),
            [entry.path.clone()],
        )?;
    }
    core.install_builtin_tools(&service)?;
    core.extensions.resource_loader = Some(ExtensionService::resource_loader_for(service.clone()));
    core.extensions.file_index = ExtensionService::new_file_index();
    core.extensions.workspaces = service;
    core.extensions.workspace_id = WorkspaceId::from(primary.id.as_str());
    core.extensions.workspace_name = primary.name.clone();
    core.extensions.workspace_roots = entries.iter().map(|entry| entry.path.clone()).collect();
    Ok(())
}

/// serve 重启后重建进程内 session → workspace 绑定（映射不落库）。
pub fn bind_fixture_sessions(core: &AppCore, spec: &SeedSpec) {
    for session in &spec.sessions {
        core.bind_session_workspace(
            &SessionId::from(session.id.as_str()),
            WorkspaceId::from(session.workspace_id.as_str()),
        );
    }
}

struct DiffPlanEntry {
    workspace_root: PathBuf,
    action: String,
    long_line: bool,
}

/// 执行 seed：整体重建 fixture root（marker 在场时先清理已知子树）。
pub async fn seed(
    root: &Path,
    now_ms_override: Option<i64>,
    spec: &SeedSpec,
    pty_source: &Path,
) -> Result<SeedOutcome, String> {
    validate_spec(spec)?;
    validate_root(root)?;
    let now_ms = now_ms_override.unwrap_or(spec.now_ms);
    validate_seed_timestamps(spec, now_ms)?;

    prepare_root(root)?;
    // 先写 preparing marker：后续任一步失败时，root 仍可被安全重试或 clean；
    // serve 只接受最终 ready marker，绝不会消费半成品。
    write_marker(root, spec, now_ms, "preparing")?;
    std::fs::create_dir_all(root.join("data/checkpoints"))
        .map_err(|error| format!("创建 checkpoints 目录失败：{error}"))?;
    std::fs::create_dir_all(root.join("pty"))
        .map_err(|error| format!("创建 pty 目录失败：{error}"))?;
    std::fs::create_dir_all(root.join("barriers"))
        .map_err(|error| format!("创建 barriers 目录失败：{error}"))?;
    std::fs::create_dir_all(root.join("logs"))
        .map_err(|error| format!("创建 logs 目录失败：{error}"))?;

    let workspaces = resolve_workspaces(spec, root)?;
    for workspace in &workspaces {
        let spec_workspace = spec
            .workspaces
            .iter()
            .find(|item| item.id == workspace.id)
            .expect("resolve_workspaces keeps spec entries");
        seed_workspace_files(spec, spec_workspace, &workspace.path)?;
        if spec_workspace.git {
            git_commit_baseline(&workspace.path, now_ms)?;
        }
    }

    let (store, _) = SessionStore::open(root.join("data/session.db"))
        .await
        .map_err(|error| format!("打开 session.db 失败：{error}"))?;
    let artifacts = ArtifactStore::open(root.join("data/checkpoints"))
        .await
        .map_err(|error| format!("打开 checkpoint 存储失败：{error}"))?;
    let checkpoints = CheckpointService::open(artifacts.clone())
        .await
        .map_err(|error| format!("打开 checkpoint 服务失败：{error}"))?;

    // (session_id, path) -> diff 计划：写工具事件生成时先做真实写前快照，
    // 再落工作区改动，保证 working tree 与事件流一致。
    let mut diff_plan: BTreeMap<(String, String), DiffPlanEntry> = BTreeMap::new();
    for diff in &spec.diffs {
        let Some(workspace) = workspaces.iter().find(|item| item.id == diff.workspace_id) else {
            return Err(format!("diff 引用未知 workspace {}", diff.workspace_id));
        };
        for file in &diff.files {
            diff_plan.insert(
                (diff.session_id.clone(), file.path.clone()),
                DiffPlanEntry {
                    workspace_root: workspace.path.clone(),
                    action: file.action.clone(),
                    long_line: file.long_line,
                },
            );
        }
    }

    let mut events_total = 0usize;
    let mut checkpoints_total = 0usize;
    for session in &spec.sessions {
        events_total += seed_session(&store, &checkpoints, session, now_ms, &diff_plan)
            .await
            .map_err(|error| format!("种子 session {} 失败：{error}", session.id))?;
        checkpoints_total += diff_plan
            .range((session.id.clone(), String::new())..(session.id.clone(), char::MAX.to_string()))
            .count();
    }

    if let Some(pty) = &spec.pty {
        let target = root
            .join("pty")
            .join(safe_relative_path(&pty.script, "pty.script")?);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建 PTY 脚本目录失败：{error}"))?;
        }
        std::fs::copy(pty_source, &target)
            .map_err(|error| format!("拷贝 PTY 脚本失败：{error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
                .map_err(|error| format!("设置 PTY 脚本权限失败：{error}"))?;
        }
    }

    store
        .shutdown()
        .await
        .map_err(|error| format!("关闭 session store 失败：{error}"))?;
    artifacts
        .shutdown()
        .await
        .map_err(|error| format!("关闭 artifact store 失败：{error}"))?;

    let manifest = write_manifest(root, spec, now_ms)?;
    write_marker(root, spec, now_ms, "ready")?;

    Ok(SeedOutcome {
        root: root.to_path_buf(),
        now_ms,
        workspaces: workspaces.len(),
        sessions: spec.sessions.len(),
        events: events_total,
        checkpoints: checkpoints_total,
        manifest,
    })
}

/// marker 在场 → 清理已知子树后整体重建；目录存在但无 marker → fail-closed。
fn prepare_root(root: &Path) -> Result<(), String> {
    if root.exists() {
        if !fixture_marker_present(root) {
            let has_content = root
                .read_dir()
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(true);
            if has_content {
                return Err(format!(
                    "{} 已存在但缺少 fixture marker {}，拒绝写入",
                    root.display(),
                    FIXTURE_MARKER_FILE
                ));
            }
        } else {
            for name in ["data", "workspaces", "pty", "barriers", "logs"] {
                let path = root.join(name);
                if path.exists() {
                    std::fs::remove_dir_all(&path)
                        .map_err(|error| format!("重建清理 {} 失败：{error}", path.display()))?;
                }
            }
            for name in ["manifest.json", "host.pid", "desktop.pid"] {
                let path = root.join(name);
                if path.exists() {
                    std::fs::remove_file(&path)
                        .map_err(|error| format!("重建清理 {} 失败：{error}", path.display()))?;
                }
            }
        }
    }
    std::fs::create_dir_all(root).map_err(|error| format!("创建 fixture root 失败：{error}"))
}

fn write_marker(root: &Path, spec: &SeedSpec, now_ms: i64, state: &str) -> Result<(), String> {
    let path = marker_path(root);
    let mut bytes = serde_json::to_vec_pretty(&json!({
        "fixture_version": spec.fixture_version,
        "now_ms": now_ms,
        "state": state,
        "updated_at_ms": wall_now_ms(),
    }))
    .expect("serialize marker");
    bytes.push(b'\n');
    // marker 自身是恢复哨兵：即使写入中断留下非 ready 内容，存在性仍允许
    // seed/clean 重试；serve 则因 JSON/state 校验 fail-closed。
    std::fs::write(&path, bytes).map_err(|error| format!("写 marker 失败：{error}"))
}

fn seed_workspace_files(
    spec: &SeedSpec,
    workspace: &SeedWorkspace,
    root: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|error| format!("创建 workspace 失败：{error}"))?;
    let readme = root.join("README.md");
    if !readme.exists() {
        std::fs::write(
            &readme,
            format!(
                "# {}\n\nPawork UI fixture workspace（{}）。\n",
                workspace.name,
                if workspace.git { "git" } else { "非 git" }
            ),
        )
        .map_err(|error| format!("写 README 失败：{error}"))?;
    }
    if !workspace.git {
        std::fs::write(
            root.join("notes.txt"),
            "gamma-notes 手写笔记。\n用于 Changes 空态与项目列表边界。\n",
        )
        .map_err(|error| format!("写 notes.txt 失败：{error}"))?;
    }
    // diff 基线：modified / deleted / long_line 的文件必须先进入基线提交。
    for diff in &spec.diffs {
        if diff.workspace_id != workspace.id {
            continue;
        }
        for file in &diff.files {
            if file.action == "added" {
                continue;
            }
            let path = root.join(safe_relative_path(
                &file.path,
                &format!("diff {} path", diff.session_id),
            )?);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("创建 diff 基线目录失败：{error}"))?;
            }
            std::fs::write(&path, baseline_content(&workspace.name, &file.path))
                .map_err(|error| format!("写 diff 基线失败：{error}"))?;
        }
    }
    Ok(())
}

fn baseline_content(workspace_name: &str, path: &str) -> String {
    format!(
        "# {workspace_name} · {path}\n\nPawork UI fixture baseline.\nline-2 stable\nline-3 stable\n"
    )
}

fn modified_content(path: &str, long_line: bool) -> String {
    if long_line {
        format!(
            "# {path} modified\n{}\ntrailing stable line\n",
            "L".repeat(240)
        )
    } else {
        format!("# {path} modified\nline-2 edited by fixture\nline-3 stable\n")
    }
}

fn added_content(path: &str) -> String {
    format!("# {path}\n\nadded by Pawork UI fixture.\n")
}

/// 确定性基线提交：固定作者、日期与消息；隔离全局 git 配置与路由环境。
fn git_commit_baseline(root: &Path, now_ms: i64) -> Result<(), String> {
    let date = unix_ms_to_git_date(now_ms - 86_400_000);
    run_git(root, &["init", "--initial-branch=main"], &date)?;
    run_git(root, &["add", "--all"], &date)?;
    run_git(root, &["commit", "--message=fixture baseline"], &date)?;
    Ok(())
}

fn unix_ms_to_git_date(unix_ms: i64) -> String {
    // git 的 ISO 8601 固定锚点：直接从 FIXTURE_NOW_MS 派生，避免本地时区影响。
    let seconds = unix_ms.div_euclid(1000);
    let (year, month, day, hour, minute, second) = civil_from_unix(seconds);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_unix(seconds: i64) -> (i64, u32, u32, u32, u32, u32) {
    // days since epoch → civil（Howard Hinnant 算法，无外部依赖）。
    let days = seconds.div_euclid(86_400);
    let secs_of_day = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        m as u32,
        d as u32,
        (secs_of_day / 3600) as u32,
        ((secs_of_day % 3600) / 60) as u32,
        (secs_of_day % 60) as u32,
    )
}

fn run_git(root: &Path, args: &[&str], date: &str) -> Result<std::process::Output, String> {
    let isolated_global_config = root.join(".git/pawork-no-global-config");
    let mut command = std::process::Command::new("git");
    command
        .args(args)
        .current_dir(root)
        // 不读取用户 / 系统配置：全局 commit.gpgsign、init.templateDir、
        // core.hooksPath 等均不得改变 fixture 行为或执行外部 hook。
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", isolated_global_config)
        .env("GIT_TERMINAL_PROMPT", "0")
        // 不允许调用方的 Git 路由变量把操作重定向到 fixture root 外。
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_NAMESPACE")
        .env_remove("GIT_TEMPLATE_DIR")
        .env_remove("GIT_EXEC_PATH")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env("GIT_AUTHOR_NAME", "Pawork Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@pawork.invalid")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_NAME", "Pawork Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@pawork.invalid")
        .env("GIT_COMMITTER_DATE", date);
    let output = command
        .output()
        .map_err(|error| format!("启动 git 失败：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} 失败：{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(output)
}

/// 种子单个 session：显式 created_at + 事件时间戳，最后一条事件时间戳
/// 精确落在 now+updated_offset（决定 Today/Yesterday/Earlier 分桶）。
async fn seed_session(
    store: &SessionStore,
    checkpoints: &CheckpointService,
    session: &SeedSession,
    now_ms: i64,
    diff_plan: &BTreeMap<(String, String), DiffPlanEntry>,
) -> Result<usize, String> {
    let session_id = SessionId::from(session.id.as_str());
    let created_ts = now_ms + session.created_offset_ms;
    let updated_ts = now_ms + session.updated_offset_ms;
    if updated_ts <= created_ts {
        return Err(format!(
            "session {} updated_offset 必须晚于 created_offset",
            session.id
        ));
    }
    let created_at = u64::try_from(created_ts)
        .map_err(|_| format!("session {} created_at 为负数", session.id))?;
    store
        .create_session(
            &session_id,
            session.title.as_str(),
            Timestamp::from_unix_millis(created_at),
        )
        .await
        .map_err(|error| format!("create_session 失败：{error}"))?;
    let branch = store
        .get_session(&session_id)
        .await
        .map_err(|error| format!("get_session 失败：{error}"))?
        .active_branch;

    // (run_id, payload)：run_id 随 turn 递增，checkpoint 记录与事件流共用。
    let mut payloads: Vec<(String, AgentEvent)> = Vec::new();
    for (turn_idx, turn) in session.turns.iter().enumerate() {
        build_turn(
            &mut payloads,
            session,
            turn_idx,
            turn,
            checkpoints,
            diff_plan,
        )
        .await?;
    }

    let total = payloads.len();
    for (index, (run_id, payload)) in payloads.into_iter().enumerate() {
        let sequence = EventSequence::new((index + 1) as u64);
        let timestamp = updated_ts - ((total - 1 - index) as i64) * 10;
        if timestamp <= created_ts {
            return Err(format!(
                "session {} 事件时间戳与 created_at 冲突（缩短 turns 或拉大 offset 差）",
                session.id
            ));
        }
        let envelope = AgentEventEnvelope::new(
            EventId::from(format!("evt-{}-{}", session.id, index + 1)),
            session_id.clone(),
            RunId::from(run_id.as_str()),
            sequence,
            Timestamp::from_unix_millis(
                u64::try_from(timestamp)
                    .map_err(|_| format!("session {} 事件时间戳为负数", session.id))?,
            ),
            payload,
        );
        store
            .append_event(&branch, envelope)
            .await
            .map_err(|error| format!("append_event 失败：{error}"))?;
    }
    Ok(total)
}

async fn build_turn(
    out: &mut Vec<(String, AgentEvent)>,
    session: &SeedSession,
    turn_idx: usize,
    turn: &SeedTurn,
    checkpoints: &CheckpointService,
    diff_plan: &BTreeMap<(String, String), DiffPlanEntry>,
) -> Result<(), String> {
    let run_id = format!("run-{}-{}", session.id, turn_idx + 1);
    let user_message_id = MessageId::from(format!("msg-{}-{}-u", session.id, turn_idx));
    let assistant_message_id = MessageId::from(format!("msg-{}-{}-a", session.id, turn_idx));
    let usage = TokenUsage {
        input_tokens: turn.usage.input,
        output_tokens: turn.usage.output,
        ..TokenUsage::default()
    };

    let mut push = |event: AgentEvent| out.push((run_id.clone(), event));

    push(AgentEvent::MessageCommitted {
        message: Message {
            id: user_message_id.clone(),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: turn.user.clone(),
            })],
            metadata: MessageMetadata::default(),
        },
    });
    push(AgentEvent::RunStarted {
        trigger_message_id: user_message_id,
    });
    push(AgentEvent::ContextPrepared {
        message_count: ((turn_idx + 1) * 2) as u64,
        estimated_input_tokens: usage.input_tokens.max(1),
    });
    push(AgentEvent::ProviderRequestStarted {
        request_id: RequestId::from(format!("req-{}-{}", session.id, turn_idx)),
        provider_id: ProviderId::from("mock"),
        model: "fixture-model".into(),
    });
    push(AgentEvent::UsageUpdated {
        usage: usage.clone(),
    });

    // 流式片段：把 assistant 全文按 stream_chunks 均分（char 边界安全）。
    let assistant_text: String = turn.assistant.concat();
    if !assistant_text.is_empty() {
        let chunks = split_chunks(&assistant_text, turn.stream_chunks.max(1));
        for chunk in chunks {
            push(AgentEvent::AssistantTextDelta {
                message_id: assistant_message_id.clone(),
                delta: chunk,
            });
        }
        push(AgentEvent::MessageCommitted {
            message: Message {
                id: assistant_message_id,
                role: MessageRole::Assistant,
                content: turn
                    .assistant
                    .iter()
                    .map(|part| ContentPart::Text(TextContent { text: part.clone() }))
                    .collect(),
                metadata: MessageMetadata::default(),
            },
        });
    }

    for (tool_idx, tool) in turn.tools.iter().enumerate() {
        let tool_call_id =
            ToolCallId::from(format!("call-{}-{}-{}", session.id, turn_idx, tool_idx));
        let arguments = json!({"path": tool.path.clone().unwrap_or_default()});
        push(AgentEvent::ToolCallStarted {
            tool_call_id: tool_call_id.clone(),
            name: tool.name.clone(),
        });
        push(AgentEvent::ToolCallArgumentsDelta {
            tool_call_id: tool_call_id.clone(),
            json_delta: serde_json::to_string(&arguments).expect("serialize tool args"),
        });
        match tool.status.as_str() {
            "pending" => {
                push(AgentEvent::ToolApprovalRequested {
                    tool_call_id,
                    reason: format!("{} 需要审批（fixture pending）", tool.name),
                });
            }
            "running" => {
                push(AgentEvent::ToolExecutionStarted { tool_call_id });
            }
            "succeeded" | "failed" => {
                let is_error = tool.status == "failed";
                let Some(path) = tool.path.as_deref() else {
                    return Err(format!(
                        "session {} turn {} tool {}（{}）缺少 path",
                        session.id, turn_idx, tool_idx, tool.name
                    ));
                };
                // 写工具：真实写前快照 → 工作区改动 → CheckpointCreated 事件，
                // 保证 session_diff 的路径过滤与 git working tree 一致。
                let mut artifacts = Vec::new();
                if !is_error {
                    if let Some(plan) = diff_plan.get(&(session.id.clone(), path.to_string())) {
                        let snapshot = checkpoints
                            .snapshot_before_write(
                                &run_id,
                                tool_call_id.as_str(),
                                &[plan.workspace_root.clone()],
                                path,
                            )
                            .await
                            .map_err(|error| format!("写前快照失败：{error}"))?;
                        if let Some(blob) = snapshot.pre_blob {
                            artifacts.push(ArtifactId::from(blob.as_str()));
                        }
                        apply_diff_change(
                            &plan.workspace_root,
                            path,
                            &plan.action,
                            plan.long_line,
                        )?;
                    }
                }
                if is_write_tool(&tool.name) {
                    push(AgentEvent::ToolApprovalRequested {
                        tool_call_id: tool_call_id.clone(),
                        reason: format!("{} 将写入 {}", tool.name, path),
                    });
                    push(AgentEvent::ToolApprovalResponded {
                        tool_call_id: tool_call_id.clone(),
                        decision: ApprovalDecision::ApprovedOnce,
                        comment: None,
                    });
                }
                if !artifacts.is_empty() {
                    push(AgentEvent::CheckpointCreated {
                        checkpoint_id: pawork_domain::CheckpointId::from(format!(
                            "{}/{}",
                            run_id,
                            tool_call_id.as_str()
                        )),
                        artifacts,
                    });
                }
                push(AgentEvent::ToolExecutionStarted {
                    tool_call_id: tool_call_id.clone(),
                });
                let output_text = if is_error {
                    tool.error
                        .clone()
                        .unwrap_or_else(|| "fixture tool failure".into())
                } else {
                    format!("{} 完成（fixture）", tool.name)
                };
                push(AgentEvent::ToolOutputDelta {
                    tool_call_id: tool_call_id.clone(),
                    stream: ToolOutputStream::Stdout,
                    delta: output_text.clone(),
                });
                let result = ToolResultContent {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: Some(tool.name.clone()),
                    content: vec![ContentPart::Text(TextContent { text: output_text })],
                    is_error,
                    metadata: Value::Null,
                    artifacts: Vec::new(),
                };
                push(AgentEvent::ToolExecutionCompleted {
                    tool_call_id: tool_call_id.clone(),
                    result: result.clone(),
                });
                push(AgentEvent::MessageCommitted {
                    message: Message {
                        id: MessageId::from(format!(
                            "msg-{}-{}-t{}",
                            session.id, turn_idx, tool_idx
                        )),
                        role: MessageRole::Tool,
                        content: vec![ContentPart::ToolResult(result)],
                        metadata: MessageMetadata::default(),
                    },
                });
            }
            other => {
                return Err(format!(
                    "未知 tool status {other:?}（session {}）",
                    session.id
                ))
            }
        }
    }

    // pending_approval 的会话停在审批请求上，不落终态事件。
    if session.state == "pending_approval" {
        return Ok(());
    }
    match turn.stop.as_str() {
        "completed" => push(AgentEvent::RunCompleted {
            stop_reason: StopReason::Completed,
            usage,
        }),
        "failed" => push(AgentEvent::RunFailed {
            error: ErrorContext {
                category: ErrorCategory::Provider,
                message: "fixture scripted provider failure".into(),
                retryable: false,
                retry_after_ms: None,
                diagnostics: BTreeMap::new(),
            },
            usage: Some(usage),
        }),
        "cancelled" => push(AgentEvent::RunCancelled {
            reason: Some("cancelled by fixture user".into()),
            usage: Some(usage),
        }),
        other => {
            return Err(format!(
                "未知 turn stop {other:?}（session {}）",
                session.id
            ))
        }
    }
    Ok(())
}

fn is_write_tool(name: &str) -> bool {
    matches!(name, "write_file" | "edit_file" | "apply_patch")
}

fn apply_diff_change(
    workspace_root: &Path,
    path: &str,
    action: &str,
    long_line: bool,
) -> Result<(), String> {
    let target = workspace_root.join(safe_relative_path(path, "diff path")?);
    match action {
        "modified" => {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("创建父目录失败：{error}"))?;
            }
            std::fs::write(&target, modified_content(path, long_line))
                .map_err(|error| format!("应用 modified 失败：{error}"))
        }
        "added" => {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("创建父目录失败：{error}"))?;
            }
            std::fs::write(&target, added_content(path))
                .map_err(|error| format!("应用 added 失败：{error}"))
        }
        "deleted" => {
            std::fs::remove_file(&target).map_err(|error| format!("应用 deleted 失败：{error}"))
        }
        other => Err(format!("未知 diff action {other:?}")),
    }
}

fn split_chunks(text: &str, chunks: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chunks == 0 || chars.is_empty() {
        return vec![text.to_string()];
    }
    let size = chars.len().div_ceil(chunks);
    chars
        .chunks(size)
        .map(|part| part.iter().collect())
        .collect()
}

fn wall_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn append_newline(path: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(b"\n")
}

fn write_manifest(root: &Path, spec: &SeedSpec, now_ms: i64) -> Result<PathBuf, String> {
    let mut files = BTreeMap::new();
    collect_digests(root, root, &mut files)?;
    let manifest = root.join("manifest.json");
    let body = json!({
        "fixture_version": spec.fixture_version,
        "now_ms": now_ms,
        "seeded_at_ms": wall_now_ms(),
        "files": files,
    });
    std::fs::write(
        &manifest,
        serde_json::to_vec_pretty(&body).expect("serialize manifest"),
    )
    .and_then(|_| append_newline(&manifest))
    .map_err(|error| format!("写 manifest 失败：{error}"))?;
    Ok(manifest)
}

/// 递归收集 blake3 摘要；跳过 marker / manifest / logs / barriers（运行期产物）。
fn collect_digests(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, Value>,
) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|error| format!("读目录失败：{error}"))? {
        let entry = entry.map_err(|error| format!("读目录项失败：{error}"))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if name == "logs" || name == "barriers" {
                continue;
            }
            collect_digests(root, &path, out)?;
            continue;
        }
        if name == FIXTURE_MARKER_FILE || name == "manifest.json" {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|error| format!("读文件失败：{error}"))?;
        let relative = path
            .strip_prefix(root)
            .expect("digest path under root")
            .to_string_lossy()
            .replace('\\', "/");
        out.insert(
            relative,
            json!({
                "blake3": blake3::hash(&bytes).to_hex().to_string(),
                "bytes": bytes.len(),
            }),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_spec() -> SeedSpec {
        serde_json::from_value(json!({
            "fixture_version": 1,
            "now_ms": FIXTURE_NOW_MS
        }))
        .expect("minimal seed spec")
    }

    #[test]
    fn relative_fixture_paths_reject_escape_components() {
        for unsafe_path in ["../outside", "/tmp/outside", "a/../../outside", "./file"] {
            assert!(
                safe_relative_path(unsafe_path, "test path").is_err(),
                "must reject {unsafe_path:?}"
            );
        }
        assert_eq!(
            safe_relative_path("src/main.rs", "test path").expect("safe path"),
            PathBuf::from("src/main.rs")
        );
    }

    #[test]
    fn workspace_placeholder_cannot_escape_fixture_root() {
        let mut spec = empty_spec();
        spec.workspaces.push(SeedWorkspace {
            id: "escape".into(),
            name: "escape".into(),
            path: "${ROOT}/../outside".into(),
            git: false,
        });
        let root = tempfile::tempdir().expect("tempdir");
        assert!(resolve_workspaces(&spec, root.path()).is_err());
    }

    #[test]
    fn marker_requires_ready_state() {
        let root = tempfile::tempdir().expect("tempdir");
        let spec = empty_spec();
        write_marker(root.path(), &spec, spec.now_ms, "preparing").expect("preparing marker");
        assert!(fixture_marker_present(root.path()));
        assert!(!fixture_marker_ready(root.path()));
        write_marker(root.path(), &spec, spec.now_ms, "ready").expect("ready marker");
        assert!(fixture_marker_ready(root.path()));
    }

    #[test]
    fn repository_paths_are_never_valid_fixture_roots() {
        let repo = std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .expect("repo root");
        assert!(validate_root(&repo).is_err());
        assert!(validate_root(&repo.join("tmp/ui-fixture")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn overlong_fixture_socket_path_is_rejected_before_seed() {
        let parent = tempfile::tempdir().expect("tempdir");
        let root = parent.path().join("x".repeat(160));
        let error = validate_root(&root).expect_err("overlong socket path must fail");
        assert!(error.contains("Unix socket 路径过长"), "{error}");
    }

    #[test]
    fn extreme_time_anchor_is_rejected_without_overflow() {
        let spec = empty_spec();
        let error = validate_seed_timestamps(&spec, i64::MAX)
            .expect_err("extreme time anchor must fail cleanly");
        assert!(error.contains("支持范围"), "{error}");
    }

    #[test]
    fn fixture_host_profiles_are_closed_and_resource_matrix_is_deterministic() {
        assert_eq!(
            FixtureHostProfile::parse("r6-terminal").expect("terminal profile"),
            FixtureHostProfile::R6Terminal
        );
        assert!(FixtureHostProfile::parse("unknown").is_err());
        assert!(fixture_mcp_slots(FixtureHostProfile::Default).is_empty());

        let slots = fixture_mcp_slots(FixtureHostProfile::R6Resources);
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].name, "fixture-files");
        assert_eq!(slots[0].state, "connected");
        assert_eq!(slots[0].tools.len(), 2);
        assert_eq!(slots[1].name, "fixture-broken");
        assert_eq!(slots[1].state, "failed");
        assert_eq!(
            slots[1].last_error.as_deref(),
            Some("fixture scripted MCP startup failure")
        );

        assert!(matches!(
            FixtureHostProfile::R6Terminal.approval(),
            Some((ApprovalMode::AskForDangerous, true))
        ));
        assert!(matches!(
            FixtureHostProfile::R6ReadOnly.approval(),
            Some((ApprovalMode::ReadOnly, true))
        ));
    }
}
