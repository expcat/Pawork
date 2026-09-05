# pawork-workspace

> Workspace 根目录管理、相对路径解析、文件索引、六层配置、确定性资源加载与跨 Agent 兼容导入：位于 `pawork-domain` / `pawork-policy` 之上的工作区事实层，被 `pawork-tools`（文件工具）与 `pawork-app`（宿主装配）消费；不依赖 GUI / SQLite / HTTP Client / Keychain / Git / 任何具体 Provider。

## 1. 职责与边界

- 四个子系统：
  1. **根服务与路径**（`lib.rs` / `path.rs` / `file_index.rs`）：进程内 workspace 登记（roots canonicalize + 去重）、`workspace 相对路径 → 绝对路径` 的安全解析、工作区文件全量索引与增量看护。
  2. **config**：`Builtin < Global < Profile < Workspace < Session < Run` 六层配置发现、剥离与合并，产出带 provenance 与告警的 `ResolvedConfig`。
  3. **resources**：确定性加载 `AGENTS.md` 层级、Skills（manifest + 依赖解析）与 Agent Profile（v1 自动迁 v2），汇总为按优先级排序的注入指令集。
  4. **import**：只读探测 Claude Code / OpenAI Codex / xAI Grok / Cursor / Pi 五个外部来源（P17-13），映射为 canonical 计划；附带本地会话文件的只读发现（`session_scan`）。
- 路径安全内核（symlink 逃逸 / `.git` 段 / TOCTOU 复核）**委托 `pawork-policy`**，本包只补 Windows 盘符 / UNC / 保留设备名检查；`resolve_relative_path` 对外签名保持稳定。
- import 是输入侧 Adapter：只读、绝不执行外部内容、绝不改写源文件；导入结果永远不是运行时事实源。
- 不做持久化（roots 与索引均为进程内状态）、不做网络、不做 Secret 存取（只产生 reference；凭证 env 名与 auth 文件定位归 `pawork-auth`）。

## 2. 模块与文件地图

**根服务与路径（3 文件）**

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~200 | `Workspace{id,name,roots}` / `WorkspaceService::add/get`（roots `fs::canonicalize` + `dunce::simplified` + 平台感知去重，Windows 大小写不敏感）；`canonicalize_root` 公开同一规范化规则（ADR-044 持久登记前的去重键）；`WorkspaceError` 七个 variant；子模块声明与 re-export |
| `src/path.rs` | ~250 | `resolve_relative_path(roots, relative) -> ResolvedPath{absolute, root, relative}`；本层拦截空路径 / 绝对路径 / Windows 盘符 / UNC（`\\`、`//`）/ 保留设备名（CON、PRN、AUX、NUL、COM1-9、LPT1-9，含尾随 `.`/空格变体），其余委托 `pawork_policy::resolve_workspace_path`；`WorkspacePathError` 与 `PathSafetyError` 的一一映射 |
| `src/file_index.rs` | ~1040 | `FileIndex`：`scan_workspace`（`spawn_blocking` 全量扫描 + generation CAS 替换：扫描前采样代次，写回时已变则丢弃，`generation` 递增）、`snapshot` / `search`（子序列模糊匹配）、`apply_changes` 增量、`start_debounced_updates`（有界通道去抖）、`watch_workspace`（`notify` watcher）；`IndexOptions` / `FileKey` / `IndexedFile` / `IndexSnapshot` / `PathChange` / `ChangeKind` / `DebouncedUpdateHandle` / `WorkspaceWatcher` / `FileIndexError` |

**config/（7 文件）**

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/config/mod.rs` | ~90 | `ConfigTier` 六层枚举与 `priority()`（0..5）、`source_key()`、`as_str()`；模块 re-export（注意 `merge_json` 不在公开面，仅 `merge_ordered` / `ConfigValue` / `Merge`） |
| `src/config/schema.rs` | ~400 | `PaworkConfig`（`default_provider` / `default_model` / `naming_provider` / `naming_model`（ADR-054 D4 自动命名对，Option、skip_none，分层同 default 对）/ `profile` / `trust_workspaces` / `approval_mode` / `workspace_trust` / `proxy_url` / `terminal` / `providers` / `profiles` / `extra`）；`TerminalConfig`（ADR-050 D1：`shell`/`columns`/`rows` 均 Option、skip_none，仅 Global 层可写入）；`ProviderConfig`（id / base_url / default / `use_proxy`：供应商级代理开关，Option、skip_none）/ `ModelConfig` / `ProfileConfig` / `ProfileOverrides` / `SessionOverrides` / `RunOverrides`；**schema 无 `api_key` 字段**，未知键落入 `extra`；`proxy_url` 的回环直连语义由 `pawork-providers` 运行时实现 |
| `src/config/paths.rs` | ~140 | 平台定位常量与函数：`APP_QUALIFIER/ORGANIZATION/APPLICATION = dev/pawork/pawork`、`config_dir_for_app`（`directories`）、`global_config_path`（workspace 外的标准用户配置）、`workspace_config_path`（`<root>/.pawork/config.toml`）、`locate_workspace_config`（自起点向上找最近）、`default_search_roots` |
| `src/config/merge.rs` | ~160 | `ConfigValue` 包装与 `Merge` trait；`merge_json`（对象按键递归、标量与数组整体替换）；`merge_ordered`（低→高依序合并） |
| `src/config/error.rs` | ~70 | `ConfigParseError` / `ConfigError`：TOML 语法、schema 不匹配、IO 错误、写回序列化（`Write`）全部携带文件路径，`path()` 访问器 |
| `src/config/loader.rs` | ~1150 | `Loader` 构建器与 `resolve()` 全流程：来源装配、`strip_untrusted_layer` 安全剥离（六种 `ConfigWarning`，ADR-050 起非 Global 层顶层 `terminal` 整段剥离 + `TerminalIgnored` 告警，防仓库投毒默认 shell）、profile 层派生、`api_key` 双点剥除（单文件解析后 + 终值合并后）、确定性排序；`ConfigSource` / `LoadedSource` / `LoadedSourceSpan`；`ResolvedConfig{config, active_profile, sources, warnings}` |
| `src/config/writer.rs` | ~320 | 六个公开入口（`write_default_model_pair` / `write_naming_model_pair`（ADR-054）/ `write_proxy_url` / `write_provider_use_proxy` / `write_mcp_server_remove` / `write_terminal_settings`）只表达键语义；共用 `rmw_global_config`（锁 + `read_table` + 可选 `atomic_write_table`）。`write_mcp_server_remove` 键缺失时不写盘返回 `Ok(false)`。未知字段保留；不触碰六层合并。 |

**resources/（9 文件）**

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/resources/mod.rs` | ~30 | 子模块声明与公开 re-export；模块级安全文档（调用方只能用 `workspace_id + root_index + relative_path`） |
| `src/resources/request.rs` | ~180 | `WorkspaceRelativePath`（构造与 serde 反序列化两处同样拒绝绝对路径 / `..`）；`CurrentPathKind::{Directory, File}`；`ResourceRequest{workspace_id, root_index, current_path, current_path_kind, selection}`；`ResourceSelection`（active/disabled skills、prompt_template + prompt_arguments、profile、session/run instructions）；`ResourceLimits`；`ResourceLoaderOptions` |
| `src/resources/error.rs` | ~50 | `ResourceLoadError`：workspace/root 不存在、`InvalidRelativePath`、文件缺失 / 超限 / 非 UTF-8 / 越界等 |
| `src/resources/source.rs` | ~210 | 溯源与诊断：`ResourceKind` 七类（Instructions / AgentsFile / Skill / PromptTemplate / AgentProfile / LanguageServer / UserHook）；`ResourceOrigin`（Global / Workspace / Session / Run，不暴露宿主绝对路径）；`ResourceProvenance{tier, source_key, origin}`；`ResourceDiagnostics`（`sort_deterministically`）、`ResourceIssue`（warning/error 构造器 + `for_resource`）、`ResourceDiagnosticStatus` / `ResourceDiagnosticEntry` |
| `src/resources/io.rs` | ~260 | 安全 IO：`join_under_root`、crate-private `canonical_within`（两侧先 `pawork_policy::canonicalize_platform`，再 `path_within_root`）、`read_utf8_bounded` / `read_utf8_bounded_within`、`workspace_relative_key`（`relative_to_root`）、`is_safe_relative_reference`。不再自写 canonicalize / within-root。 |
| `src/resources/agents.rs` | ~490 | `AGENTS.md` 层级发现：root → `current_path` 每层目录收集，按深度排序；`AgentsDocument`（`relative_path()`）/ `AgentsHierarchy`（`from_documents` / `documents` / `len` / `nearest`）；symlink 逃逸与损坏文件隔离为诊断 |
| `src/resources/skills.rs` | ~1560 | Skill 装载：`manifest.toml` + 同目录 `SKILL.md` body；`SkillManifest`（id / version / description / parameters / dependencies / conflicts / scripts / assets / permissions）；`SkillParameter` / `SkillScript` / `SkillDependency`；激活集解析：BFS 依赖遍历、semver 兼容检查、循环与显式冲突检测；`LoadedSkill` / `SkillResolution` |
| `src/resources/profiles.rs` | ~1970 | Agent Profile 装载：v1（instructions + 默认 provider/model）与 v2（`pawork_domain::AgentProfileV2` 全维度）双 schema、v1 自动迁 v2、同名冲突与明文 Secret 检测、tool 规则一致性校验、`memory_available=false` 时显式标 `Unavailable`；`resolve_profile_references` 在 bundle 内解析 profile→skill 引用（`agent_profile_ref_id_invalid` / `ref_version_invalid` / `ref_duplicate` 三类诊断）；`AgentProfile` / `LoadedAgentProfileV2` / `InstructionLayer` / `ResolvedInstructions::ordered_layers` |
| `src/resources/loader.rs` | ~630 | `ResourceLoader::load` 聚合入口与 `workspace_resource_dir` 校验；`ResourceInstructionKind` 九类注入指令及 `priority()`；`ResourceInstruction`（中性 DTO，`byte_len()` 供 context 预算）；`ResourceBundle` |

**import/（14 文件）**

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/import/mod.rs` | ~40 | 子模块声明、安全边界文档（不执行 / 无明文 Secret / 非事实源 / 幂等）与公开 re-export |
| `src/import/source.rs` | ~100 | `ExternalSource` 五源与确定性 `rank()`（Claude=1 < Codex=2 < Grok=3 < Cursor=4 < Pi=5，同 tier 冲突时大者胜）；`SourceFileKind` 八类（InstructionsDoc / ClaudeSettings / ConfigToml / McpJson / SkillMarkdown / AgentMarkdown / AgentsJson / PiSettings，pub(crate)）；`GlobalSource{source, root}`（全局来源默认不读，必须显式启用） |
| `src/import/model.rs` | ~280 | canonical 输出模型：`ImportCategory` 六类（Instructions / Skill / McpServer / AgentProfile / UserHook / PermissionRule）；`ImportStatus` 四态（Imported / Disabled / Unsupported / Conflict）；`PermissionDecision`（Allow / Ask / Deny）；`CompatItem`（id + status + source + requires_review + payload + issues）；`CompatPayload` 六变体；`CompatIssue` / `IssueSeverity` / `CredentialReference` / `PendingCredential` / `DetectedSourceSummary`；`CompatPlan`（manifest_version + sources + items + issues + credential_references + fingerprint，`sort_deterministically`） |
| `src/import/limits.rs` | ~30 | `CompatLimits` 硬上限：`max_file_bytes`（1 MiB）/ `max_files_per_kind`（256）/ `max_total_files`（2048）/ `max_scan_depth`（32）/ `max_dir_entries`（4096） |
| `src/import/error.rs` | ~40 | `CompatError`（IO 带路径 / Invalid / UnsafeTarget 等硬错误；与条目级 `CompatIssue` 明确区分） |
| `src/import/detect.rs` | ~590 | 只读探测：静态候选清单 + glob（`CLAUDE.md`、`.claude/settings.json`、`.claude/skills/*/SKILL.md`、`.codex/config.toml`、`.codex/agents.json`、`.cursor/rules/*.mdc`、`.cursor/mcp.json`、`.grok/config.toml`、`.pi/settings.json`、`.mcp.json`…）+ `AGENTS.md` 层级扫描；限额截断记 `CompatIssue`；产出 `DetectedFile{kind, tier, relative_path, sources}` |
| `src/import/io.rs` | ~380 | 导入专用安全 IO：`read_utf8_bounded`（no-follow symlink 检查 + 越界拒绝 + 大小上限）、`atomic_write`（tmp + rename，目标 symlink 拒绝）、`is_symlink`、`fnv64` |
| `src/import/frontmatter.rs` | ~140 | 极简 YAML frontmatter 解析：只取顶层标量键值对，复杂键（嵌套 / 列表）记入 `complex_keys` 供诊断，绝不求值 |
| `src/import/parse.rs` | ~1390 | 八类文件 → canonical 条目（详见 §4.5）：instructions / SKILL.md / agent markdown / agents.json / mcp json / Codex-Grok config.toml / Claude settings.json（permissions + hooks）/ Pi settings.json；`${VAR}` 与 `$VAR` 插值 → `SecretRef`，字面量 Secret 丢弃只留 `PendingCredential` 占位；hook 事件名 snake_case 化后映射 17 种 `TriggerPoint`，不可映射标 Unsupported |
| `src/import/map.rs` | ~70 | 同 `(category, id)` 冲突裁决：`ConfigTier.priority()` 高者胜 → 平手比 `ExternalSource.rank()` → 再平手比相对路径字典序；败者标 `Conflict`、payload 清空、挂 `conflict_loser` issue |
| `src/import/mcp.rs` | ~140 | 导入版 `McpServerConfig`：`TransportSpec::{Stdio{command,args,env}, Http{url,headers}}`、`RestartPolicy`、`McpPermissions`、`auto_start` / `trusted`（导入时恒 false）；Secret 一律 `SecretRef{service, account}`（service 命名 `pawork.mcp.<server>`，`mcp_secret_service`） |
| `src/import/hook.rs` | ~240 | 导入版 `HookConfig{id, trigger, scope, lifecycle, enabled, handler}`：`TriggerPoint` 17 种 / `HookScope::{Global, Workspace}`（`covers()`）/ `HandlerConfig` 六变体（Command / Http / PromptTransform / PromptEval / AgentEval / McpTool）与配套类型（`BudgetLimit` / `EvalFallback` / `McpFallback`）；Secret 只以 `SecretRef` 出现 |
| `src/import/apply.rs` | ~320 | `CompatLoader::scan / dry_run / export_plan`；FNV-64 内容链指纹（混入 `FINGERPRINT_FORMAT_VERSION` 与 manifest_version）；`CompatPlan::preview / select / counts_by_status`；`export_plan` 幂等（指纹 + 磁盘内容身份双校验）、输出目录与目标文件 symlink 拒绝、原子写 `compat-import.json` + `.compat-import-fingerprint` |
| `src/import/session_scan.rs` | ~320 | 本地会话只读发现：`LocalSessionSource::{Claude, Codex}`、`LocalSessionRoots::detect/from_home`（`~/.claude/projects`、`~/.codex/sessions`）、`LocalSessionFile`、`scan_local_sessions`（深度与总量限额、symlink 跳过、Claude 排除 `agent-*.jsonl` subagent sidecar） |

`fixtures/` 为 smoke 测试夹具（五来源目录 `.claude` / `.codex` / `.cursor` / `.grok` / `.pi` + `.mcp.json` + `AGENTS.md` / `CLAUDE.md`），不是文档。

ADR-053（OPT-1）：schema 新增 `approval_mode: Option<pawork_policy::ApprovalMode>`（沿用 Policy 兼容读入 `on_failure`）与 `workspace_trust: BTreeMap<String, bool>`；缺失保持旧安全默认。新增 `write_approval_mode` / `write_workspace_trust` 共用既有 RMW 锁，逐项目信任写入不替换其他根路径。非 Builtin/Global 层两键剥离，分别产出 `ConfigWarning::PermissionsIgnored { key, tier, source_key, path }`。加载 golden `persisted_permissions_are_global_only` 固定 Global 权威、阻止仓库自我提权与非法审批模式拒绝。

## 3. 对外 API 面

**根服务（crate root）**
- `WorkspaceService::add(id, name, roots)`：每个 root `fs::canonicalize` + `dunce::simplified`（去 Windows `\\?\` 前缀）后按平台键去重（Windows 大小写不敏感、`\`→`/`）。空集→`NoRoots`；不存在→`InvalidRoot{path, source}`；非目录→`RootIsNotDirectory`；重复 id→`AlreadyExists`；锁毒化→`Poisoned`。
- `get(id) -> Result<Option<Workspace>, WorkspaceError>`。进程内 `Arc<RwLock<BTreeMap>>`，无持久化（持久注册表在 storage session 库 v14 `workspaces` 表，本包只提供 `canonicalize_root` 规范化）；`Workspace` 可 serde 序列化供上层快照。
- `resolve_relative_path(roots, relative)`：按登记顺序命中第一个 root；错误矩阵见 §4.1。

**文件索引**
- `FileIndex::new(IndexOptions)`。`IndexOptions` 默认值：`excluded_directories` 含 `.git` / `.hg` / `.svn` / `node_modules` / `target` / `.next` / `dist` / `build` / `.cache` / `vendor` 十项；`binary_probe_bytes = 8KB`；`global_ignore_files` / `workspace_ignore_files` 由宿主注入。
- `scan_workspace(&Workspace) -> IndexSnapshot`（async；阻塞池执行，成功后整体替换，`generation` 从 1 递增）；`snapshot(workspace_id)` 读取当前代；`search(workspace_id, query, limit)` ASCII 大小写不敏感的子序列模糊匹配（按得分降序、同分按 key 定序）；`apply_changes(&Workspace, &[PathChange])` 增量应用（`ChangeKind` 增/删/改；命中需要重扫的变更时自动升级为全量 `scan_workspace`）。
- 看护：`watch_workspace` 启动 `notify::RecommendedWatcher` 返回 `WorkspaceWatcher`；`start_debounced_updates` 返回 `DebouncedUpdateHandle::{submit, errors, errors_truncated, dropped_events, shutdown}`。两者的错误缓冲上限 1024 条，溢出丢最旧并插入截断标记。
- 索引键 `FileKey{root_index, relative_path}`；`IndexedFile` 记 size / modified_at_ms / language / binary。不接受绝对路径输入。
- `IndexSnapshot` 携带 `generation`（每次全量扫描 +1）与 `scan_duration_ms`，调用方可据此判断快照新旧；`search` 未索引 workspace 返回错误而非空集。
- `FileIndexError`：workspace 未索引、阻塞任务失败、锁毒化、watcher 启动失败等。

**config**

层级与来源：

| tier | `priority()` | 来源 | 装配方式 |
| --- | --- | --- | --- |
| Builtin | 0 | `PaworkConfig::builtin()`（`trust_workspaces=false`） | `discover*` 自动 / `with_builtin` |
| Global | 1 | 用户全局 `config.toml`（`config_dir_for_app`） | `discover*` 自动 |
| Profile | 2 | 由 `profile = "<name>"` + `[[profiles]]` 派生 | resolve 时自动派生 |
| Workspace | 3 | `<root>/.pawork/config.toml` | `discover*` 自动 |
| Session | 4 | `SessionOverrides`（无文件来源） | 仅 `with_session` |
| Run | 5 | `RunOverrides`（无文件来源） | 仅 `with_run` |

- 构建器：`Loader::discover(workspace_root)` / `discover_from(global_file, workspace_file)`（文件缺失静默跳过）；手动 `with_builtin` / `with_file(tier, source_key, path)` / `with_value(tier, source_key, json)`。
- `resolve() -> Result<ResolvedConfig, ConfigError>`：`config: PaworkConfig`、`active_profile: Option<String>`、`sources: Vec<LoadedSource>`（每层**剥离后**的值 + `LoadedSourceSpan{tier, source_key, path}`，供 provenance 审计）、`warnings: Vec<ConfigWarning>`。
- `PaworkConfig` 字段语义：`default_provider` / `default_model`（缺省选择）、`profile`（激活 profile 名）、`trust_workspaces`（安全开关，仅 Builtin/Global 可设）、`proxy_url`（全局出站代理，仅 Builtin/Global 可设；GUI Network 与手工编辑共用 workspace 外的 Global `config.toml`）、`providers: Vec<ProviderConfig>`（id / base_url / default / `use_proxy`：`None`/`true` 跟随全局代理，显式 `false` 绕过）、`profiles: Vec<ProfileConfig>`（name + `ProfileOverrides`）、`extra`（未知键透传，供上层扩展段消费）。
- `ConfigWarning` 五种：`TrustWorkspacesIgnored` / `ProxyUrlIgnored` / `ProviderBaseUrlIgnored` / `McpTrustedIgnored` / `McpAutoStartIgnored`，均带 tier + source_key + path。
- 错误语义：TOML 语法错、schema 不匹配、IO 错均带文件路径；缺失文件不致命（不加该来源）。
- 写盘入口：`write_default_model_pair(path, provider_id, model_id)`（SET-2）只改目标文件（宿主传 Global 层路径）的 `default_provider`/`default_model` 两键；`write_naming_model_pair(path, provider_id, model_id)`（ADR-054 D4）同形写 `naming_provider`/`naming_model` 两键；`write_proxy_url(path, proxy_url: Option<&str>)`（SET-6a / ADR-047 D2）`Some` 覆盖 `proxy_url`、`None` 移除该键；`write_mcp_server_remove(path, name)`（SET-6c / ADR-049 D2）只移除 Global 层 `mcp.servers.<name>` 键，键不存在返回 `Ok(false)` 不写盘。各入口均保留其余未知字段；缺失文件视为空文档；同目录 tmp + rename 原子写回，序列化失败返回 `ConfigError::Write`（带路径）；同进程内各入口共用 `CONFIG_WRITE_LOCK` 包住 read-modify-write，避免交错写丢更新。`write_provider_use_proxy(path, provider_id, use_proxy)`（ADR-052 SET-6h）按 id 定位 `[[providers]]` 数组条目写入 `use_proxy`（条目缺失时新增仅含 `id` + `use_proxy` 的最小条目；`providers` 非数组返回错误），其余条目与未知字段原样保留。六层合并语义不变，本包此外无任何写盘代码。
- `paths` 函数无 IO 副作用（`locate_workspace_config` 除外，只读存在性检查）；`config_dir_for_app` 用 `directories` 三元组 `dev/pawork/pawork`。

**resources**
- `ResourceLoader::new(WorkspaceService, ResourceLoaderOptions)`；`load(&ResourceRequest) -> Result<ResourceBundle, ResourceLoadError>`（同步、只读）。
- `ResourceLoaderOptions`：`global_resource_dir`（宿主解析，**不得来自模型输入**）、`workspace_resource_dir`（默认 `.pawork`，拒绝绝对路径与 `..`）、`limits`、`memory_available`（默认 false）。
- `ResourceLimits` 默认：单文件 1 MiB、每类 1024 个、模板文件引用 32 个、渲染总量 4 MiB（后两项本包只声明不消费，见 §8）。
- `ResourceRequest.current_path` 是 `WorkspaceRelativePath`——构造与 serde 反序列化两个边界都校验；`selection` 控制 skill 激活/禁用集、profile 选择与 session/run 附加指令。
- `ResourceBundle` 字段：

  | 字段 | 类型 | 内容 |
  | --- | --- | --- |
  | `agents` | `AgentsHierarchy` | root→current_path 的 `AGENTS.md` 文档序列，`nearest()` 取最深 |
  | `skills` | `SkillResolution` | 激活 skill 清单 + 依赖解析诊断 |
  | `profiles` | `Vec<AgentProfile>` | v1 兼容视图（name / instructions / 默认 provider-model） |
  | `profiles_v2` | `Vec<LoadedAgentProfileV2>` | 全维度档案（校验与 memory 标注后） |
  | `resolved_instructions` | `ResolvedInstructions` | 分层指令视图，`ordered_layers()` 输出 |
  | `instructions` | `Vec<ResourceInstruction>` | 按 kind 优先级排序的最终注入序列 |
  | `diagnostics` | `ResourceDiagnostics` | 全部资源诊断，确定性排序 |

- 单个资源损坏隔离为 `ResourceIssue`（warning/error），不崩整批；诊断确定性排序。

**import**
- `CompatLoader::new(CompatLimits)` / `Default`；`scan(workspace_root: Option<&Path>, globals: &[GlobalSource], workspace_id: Option<&WorkspaceId>) -> Result<CompatPlan, CompatError>`：只读扫描 + 解析 + 冲突裁决；workspace_id 决定 hook 的 `HookScope`（缺省 Global）。
- `dry_run(&plan) -> String`：稳定文本预览（条目状态行 + credential 行），不含文件正文 / 命令参数 / Secret 值，不写盘。
- `export_plan(&plan, output_dir) -> Result<ExportReport, CompatError>`：`ExportReport{outcome: Exported|Noop, items, bytes_written, plan_path}`；写 `compat-import.json` 与 `.compat-import-fingerprint`。
- `CompatPlan::select(&BTreeSet<String>)`：按条目 id 人工筛选，同步过滤 credential_references，指纹二次混入选择集（长度前缀编码防拼接歧义）；`counts_by_status()` 统计。
- `scan_local_sessions(&LocalSessionRoots, limits) -> Vec<LocalSessionFile>`：只返回路径与元数据，不解析内容。
- `ImportStatus` 语义：

  | 状态 | 含义 | payload |
  | --- | --- | --- |
  | `Imported` | 成功映射（hook 类仍 `enabled=false`，敏感类带 `requires_review`） | 必有 |
  | `Disabled` | 声明为「默认禁用待审」；当前源码无构造点（见 §8） | 空 |
  | `Unsupported` | 无法安全映射（未知 transport / 事件 / bypass 权限等），带诊断 | 空 |
  | `Conflict` | 同 `(category, id)` 冲突裁决的败者，带 `conflict_loser` | 空（已清除） |

- 条目 id 命名：`instructions:<path>` / `skill:<id>` / `agent:<name>` / `mcp:<server>` / `hook:<event>:<group>:<index>` / `permission:<list>:<tool>` / `permission:global`。
- 无 feature 门控；全部 API 常驻。

## 4. 核心行为与数据流

1. **path 校验语义矩阵**：空串→`Empty`；`Path::is_absolute` 或 `\\` / `//` 前缀或盘符形（`X:`）→`AbsolutePath`；任一组件命中 Windows 保留设备名（大小写不敏感、忽略尾随 `.` 与空格）→`ReservedDeviceName`；其余交 `pawork_policy::resolve_workspace_path` 判定 `Traversal` / `SymlinkEscape` / `GitInternals`（`.git` 段）/ `NonRegular`（设备 / FIFO / socket）/ `NoRoot`，policy 内核含 TOCTOU 复核。返回的 `ResolvedPath.relative` 已规范化、不含 `.` / `..`。
2. **config 六层发现与合并全流程**：
   1. `discover_from` 装配 Builtin（`PaworkConfig::builtin()`，`trust_workspaces=false`）→ Global 文件 → Workspace 文件；
   2. 每个 TOML 文件经 JSON 往返转为 `serde_json::Value`，随即剥单文件根级 `api_key`；
   3. 若已装配层合并后声明 `profile = "<name>"` 且 `[[profiles]]` 有同名项，派生 `profile:<name>` 来源插入 Global 与 Workspace 之间（`ResolvedConfig.active_profile` 记录）；
   4. `with_session` / `with_run` 追加最高两层；
   5. 对**所有非 Builtin/Global 层**执行 `strip_untrusted_layer`：剥顶层 `trust_workspaces`、顶层 `proxy_url`、`providers[].base_url` 与 `use_proxy`（无条件剥字段、不删数组项；ADR-052：非 Global 层不得覆盖代理行为）、`mcp.servers.*.trusted` / `auto_start`（保留 transport/command/url），每项产生对应 `ConfigWarning`；
   6. 按 `(tier, source_key)` 排序后 `merge_json` 依次合并：对象按键递归、标量与数组整体替换；同层多来源按 source_key 升序，结果与文件加入顺序无关；
   7. 终值反序列化为 `PaworkConfig` 前再过 `sanitize_secrets`（剥根级与 `providers[].api_key`）。
3. **resources 加载与注入顺序**：`load` 依次执行——校验 `workspace_resource_dir` 与 `current_path` → `WorkspaceService::get` 取根（`root_index` 越界报错）→ 扫描 `AGENTS.md` 层级（root 至 `current_path` 所在目录逐层收集，深度升序，`nearest()` 为最深）→ 装载 skills（全局目录 + workspace 资源目录发现 → 激活集视图）→ 装载 profiles（v1/v2 解析、迁移、校验、memory 标注）→ `resolve_profile_references` 解析 profile→skill 引用 → 汇总 `instructions` 并按 `ResourceInstructionKind::priority()` 升序注入（编号留空供演进）。所有文件读取经 `io.rs` canonical-within + 大小 + UTF-8 校验。注入优先级：

   | kind | priority | 内容来源 |
   | --- | --- | --- |
   | `AgentProfile` | 2 | 激活 profile 的 system/instructions |
   | `UserGlobalInstructions` | 4 | 用户全局指令文件 |
   | `WorkspaceInstructions` | 5 | workspace 级指令 |
   | `RootAgentsFile` | 6 | workspace 根 `AGENTS.md` |
   | `PathAgentsFile` | 7 | current_path 沿途各层 `AGENTS.md` |
   | `ActiveSkill` | 8 | 激活 skill 的 `SKILL.md` body |
   | `PromptTemplate` | 9 | 选中的 prompt 模板 |
   | `SessionInstructions` | 13 | `selection.session_instructions` |
   | `RunInstructions` | 14 | `selection.run_instructions` |
4. **skills 激活与依赖解析**：发现所有 `manifest.toml`（含 `SKILL.md` body）→ 以 `selection.active_skills` 为种子 BFS 遍历 `dependencies`（semver 兼容检查，不满足记诊断并跳过）→ `disabled_skills` 强制排除 → 检测循环依赖与 manifest 声明的 `conflicts` → 产出 `SkillResolution`（激活清单 + 诊断）。scripts / assets 路径必须是安全相对引用（`is_safe_relative_reference`）。
5. **profiles 装载与 v1→v2 迁移**：全局目录与 workspace 资源目录发现 profile 文件 → 按 schema 判定 v1（instructions + 默认 provider/model）或 v2（`AgentProfileV2` 全维度）→ v1 自动迁移为 v2（v1 视图仍保留在 `profiles` 字段）→ 校验：同名冲突、明文 Secret 检测（含 token 形字面量）、tool 规则一致性（allowed 与 denied 交集）、`memory_available=false` 时把声明 memory 的 profile 标 `Unavailable` 并 warning → `resolve_profile_references` 对 profile 声明的 skill 引用做 id / semver / 重复三类校验并挂诊断。
6. **session_scan 发现规则**：根目录 Claude=`~/.claude/projects`、Codex=`~/.codex/sessions`（`from_home` 可注入测试根）；递归受 `max_scan_depth` / `max_total_files` / `max_dir_entries` 限制；symlink 一律跳过；Claude 匹配 `*.jsonl` 但**排除 `agent-*.jsonl`**（subagent sidecar，避免把子代理会话当独立会话导入）；Codex 匹配 `rollout-*.jsonl`。只返回路径与元数据，不读内容。
7. **import scan → dry-run → export_plan**：
   1. `detect_files` 对 workspace 根与显式 `GlobalSource`（按 source→root 排序保证确定性）做静态候选 + glob 探测，限额截断记 `CompatIssue`；
   2. 每个候选经 `read_utf8_bounded`（no-follow）读取，`tier:relative_path` 头 + 内容一起累入 FNV-64 内容链；
   3. `parse_content` 按 `SourceFileKind` 分派（映射矩阵如下）；未知键只记键名（`unknown_key` / `*_key_unmapped`）绝不复制值；
   4. Secret 处理：`${VAR}` / `$VAR` 插值→`SecretRef` + `CredentialReference`；非空字面量→丢弃值、记 `PendingCredential` + `plaintext_secret_rejected`；
   5. 权限映射：Claude `permissions.allow/ask/deny` 逐条→`PermissionRule`（deny 不需 review，allow/ask 需要）；`defaultMode` 与 Codex `approval_policy` 映射全局规则——`on-request`/`default`/`plan`→Ask 类，`on-failure`/`acceptEdits`→降级 NeverAsk→Ask 决策并告警，`never`/`bypassPermissions`→Unsupported 拒绝导入；
   6. hooks：只支持 `command` 型（`prompt` 型与未知型 Unsupported）；命令按空白切分为 program+args；产出 `HookConfig{enabled: false, lifecycle: Async}` 且条目 `requires_review=true`；matcher 条件不导入（告警）；
   7. `resolve_conflicts` 以 tier priority → source rank → 相对路径裁决同 `(category, id)` 冲突，败者标 `Conflict` 清 payload；
   8. 计划 `sort_deterministically` + 指纹（`FINGERPRINT_FORMAT_VERSION` + manifest_version + 内容链）；
   9. `export_plan`：输出目录 `create_dir_all` 后校验目录与两个目标文件均非 symlink → 指纹命中且磁盘 `compat-import.json` 字节一致才 `Noop`（内容被篡改则重写）→ 否则 tmp + rename 原子写。全程不执行 hook / MCP / script、不改写任何源文件。

   解析映射矩阵（`SourceFileKind` → 产物）：

   | 文件类别 | 典型路径 | 产出 category | 关键规则 |
   | --- | --- | --- | --- |
   | `InstructionsDoc` | `CLAUDE.md`、`AGENTS.md`、rules、`SYSTEM.md` | Instructions | 全文进 payload；Global tier→`UserGlobalInstructions`，否则 `WorkspaceInstructions`；按路径深度记 depth |
   | `ClaudeSettings` | `.claude/settings.json` | PermissionRule + UserHook | permissions 三列表 + defaultMode；hooks 见流程 6.6；`env` 段只记条数绝不复制值 |
   | `ConfigToml` | `.codex/config.toml`、`.grok/config.toml` | McpServer + PermissionRule | `mcp_servers` 逐条解析；`approval_policy` 映射；`model`/`sandbox_mode` 等记 `known_unmapped` |
   | `McpJson` | `.mcp.json`、`.cursor/mcp.json` | McpServer | `mcpServers.*`：stdio（command/args/env）或 http（url/headers）；`sse`/`streamable-http` 传输 Unsupported；恒 `requires_review` |
   | `SkillMarkdown` | `.claude/skills/*/SKILL.md` | Skill | frontmatter `name`（缺省目录名兜底）+ `version`（semver，缺省 0.1.0）+ `allowed-tools`；复杂键记 `skill_key_unmapped` |
   | `AgentMarkdown` | `.claude/agents/*.md` | AgentProfile | frontmatter name/description/model/tools + body 为 system；`!x`/`-x` 前缀工具进 denied 且 deny 优先 |
   | `AgentsJson` | `.codex/agents.json` | AgentProfile | `agents.*` 对象逐条构建 `AgentProfileV2`；`hooks` 键触发 requires_review 且不导入 |
   | `PiSettings` | `.pi/settings.json` | Instructions + McpServer | `instructions` 字符串 + `mcpServers`/`mcp` 两键；hooks 记 `hooks_not_imported` |
8. **file_index 扫描与看护**：全量扫描在 `spawn_blocking` 中用 `ignore::WalkBuilder`（全局 ignore 文件 → workspace ignore 文件 → 内置排除目录）遍历各 root，前 8KB 探测二进制、记录 size / mtime / 语言标签；写回前做 generation CAS——扫描期间代次已变则丢弃本次结果，否则整体替换并 `generation+1`（失败保留旧代）。`notify` 事件经有界 mpsc 通道进入去抖循环，窗口收口后批量 `apply_changes`；watcher 错误与通道丢弃事件计入观测面（`errors` / `errors_truncated` / `dropped_events`）。

## 5. 契约与不变量

- **config schema 无 `api_key`**：`PaworkConfig` / `ProviderConfig` 无该字段；loader 在单文件解析后与终值合并后两次剥除（含 `providers[].api_key`），`Debug` 输出不含任何 api_key 文本（定向回归断言）。凭据定位归 `pawork-auth`，存取归 OS Keychain（`pawork-secrets`），均不在本包。
- **非 Builtin/Global 层不得自我提权 / 劫持出站**（冻结红线）：`trust_workspaces`、顶层 `proxy_url`、`providers[].base_url` / `providers[].use_proxy`、`mcp.servers.*.trusted` / `auto_start` 在 Profile / Workspace / Session / Run 层一律剥离并告警；只剥字段、不删整段配置；Global 层设置照常生效。
- **路径输入契约**：一切工作区文件访问基于 `workspace_id + root_index + relative_path`；绝对路径、盘符、UNC、保留设备名、`.git` 内部、symlink 逃逸、非普通文件一律拒绝。`WorkspaceRelativePath` 在 serde 反序列化边界即拒绝 `..` 与绝对路径（协议入口防线）。
- **resources 确定性与隔离**：同输入必同输出（条目与诊断确定性排序）；单资源损坏降级为诊断不崩批；`global_resource_dir` 只能由宿主注入。
- **memory 不虚假可用**：`memory_available=false` 时 profile 声明的 memory 显式标 `Unavailable` 并 warning，绝不静默假装可用。
- **import 安全边界**（冻结）：只读扫描；绝不执行任何外部内容；明文 Secret 一律丢弃（计划序列化全文不得出现 Secret 字面量，定向回归断言）；导入 hook 恒 `enabled=false` + `requires_review=true`；MCP 条目恒 `trusted=false` + `auto_start=false` + `requires_review`；`bypassPermissions` / approval `never` 拒绝导入（Unsupported）；`on-failure` / `acceptEdits` 降级映射为 Ask 并告警，绝不映射为 Allow。
- **export_plan 幂等**：相同输入（含 `select` 子集）指纹相同→`Noop`；指纹命中但磁盘内容不符仍重写；输出目录 / 目标文件为 symlink 一律 `UnsafeTarget` 拒绝；只用 tmp + rename 原子写。
- **冲突裁决确定性**：tier priority → `ExternalSource.rank()` → 相对路径字典序，三级全定序；胜者保留原 `requires_review` 不因胜出放宽。
- **detect 只读文档化位置**：每种外部来源只探测其文档化的 workspace 内已知路径（或调用方显式启用的全局根）；全局来源默认不读；未知版本 / 形态只返回诊断，绝不猜测执行。
- **不泄漏宿主绝对路径**：`GlobalSource` 与 `ResourceOrigin` 的相对路径只相对启用根记录；计划与诊断中不出现宿主绝对路径。
- **Unix no-follow 读取**：`import/io.rs` 在 Unix 上以 `openat` 逐组件遍历（`O_NOFOLLOW | O_CLOEXEC`，文件叶子再加 `O_NONBLOCK` 防 FIFO 阻塞），`ELOOP` / `ENOTDIR` 归一为 symlink 拒绝，从根上防 TOCTOU 换链。
- 本包对应的产品级安全红线与冻结契约见 [../security.md](../security.md) 与 [../contracts.md](../contracts.md)。

## 6. 依赖关系

- **依赖**：`pawork-domain`（`WorkspaceId`；profile 域类型 `AgentProfileV2` / `ProfileToolRules` / `ReasoningEffort` 等）、`pawork-policy`（`resolve_workspace_path` 路径内核、`canonicalize_platform` / `path_within_root` / `relative_to_root`、`PathSafetyError`、`ApprovalMode`）、`directories`（平台配置目录）、`dunce`、`ignore` + `notify`（文件索引）、`semver`（serde 特性，skill 版本）、`serde` / `serde_json` / `toml`、`thiserror`、`tokio`（macros / rt / sync / time）；Unix 下 `libc`。
- **被依赖**：
  - `pawork-tools`：八个文件系统工具（read_file / write_file / edit_file / apply_patch / list_directory / search_text / find_files / run_command）经 `WorkspaceService` 解析路径；`mcp/config.rs` 消费 `ResolvedConfig`。
  - `pawork-app`：宿主装配 config（`Loader::discover`）、resources（`ResourceLoader`）、import（`CompatLoader` / `scan_local_sessions`，见 `import_host.rs` / `services/import.rs`）并与 tools 共享同一 `WorkspaceService` 实例（约 12 个文件引用）。
- 全仓分层总览见 [../../architecture.md](../../architecture.md)；布局与依赖方向见 [../../design.md](../../design.md) §2；宿主装配与 Agent loop 全流程见 [../flows.md](../flows.md)；产品能力对照见 [../capabilities.md](../capabilities.md)；包级 Spec 总目见 [../README.md](../README.md)。

## 7. 测试与验证资产

- `tests/loader_file.rs`（约 15 用例，真实文件系统）：
  - 六层合并：tier 覆盖顺序、providers 数组整体替换、`discover_from` 三层装配顺序断言。
  - profile 派生：`profile:work` 来源插在 Global 与 Workspace 之间，`active_profile` 记录。
  - Session / Run 一等 API：`with_session` / `with_run` 逐层覆盖 Profile。
  - **安全红线**：`api_key` 剥离且 `Debug` 全文无泄漏；workspace 层 `proxy_url` / `providers[].base_url` / `providers[].use_proxy` / `mcp trusted+auto_start` / `trust_workspaces` 全部剥离并告警、provenance 中该层值已净化。
  - 错误与定位：解析 / schema 错误带路径、`locate_workspace_config` 就近查找、缺失文件不致命、加入顺序无关的确定性、macOS 配置目录快照（`dev/pawork/pawork`）。
- `tests/smoke.rs`（约 14 用例，基于 `fixtures/` 五源夹具）：
  - 五来源六类别全部产出 Imported 条目；导入 hook `enabled=false` + `requires_review` 断言。
  - **明文 Secret 零泄漏**：计划序列化全文断言不含夹具中的假 token；`${VAR}`→`SecretRef`、字面量→`PendingCredential` 占位。
  - 冲突裁决：同 tier 按 source rank（Codex 胜 Claude）、跨 tier 按 priority（workspace 胜 global），败者带 `conflict_loser`；胜者仍 `requires_review`。
  - `export_plan` 显式幂等且不改写源文件；dry-run 无写入；`select` 子集独立指纹、内容篡改后指纹命中仍重写。
  - 防御行为：危险内容（`bypassPermissions` 等）隔离不导入、扫描不追 symlink、per-kind 与 total 限额硬截断、`on-failure` / `acceptEdits` 映射为 Ask 而非 Allow。
- src 内嵌单元测试：`lib.rs`（roots 去重与 canonicalize、Windows 大小写去重）、`path.rs`（逃逸 / 设备名矩阵）、`file_index.rs`（扫描 / 搜索 / 去抖 / watcher / 错误缓冲）、`config/*`（loader 剥离与告警、merge 语义、paths 定位、schema 序列化、writer 原子写回与未知字段保留，含 `write_proxy_url` 设置/清除主路径、`write_provider_use_proxy` 写入并保留其余条目主路径与 `write_mcp_server_remove` 移除/缺失键路径）、`resources/*`（agents 层级、skills 依赖与冲突、profiles 迁移与校验、io 边界）、`import/*`（detect 限额、parse 各分支、io no-follow、frontmatter、map 裁决、session_scan 排除规则）。
- dev-dependencies：`tempfile`（临时目录夹具）、`serde_json`、`tokio`（macros / rt-multi-thread / time，供 async 测试）；无 build.rs、无 feature 矩阵，单一编译形态。
- 默认验证命令：`cargo test -p pawork-workspace --offline --lib --tests`。

## 8. 注意事项与已知限制

- **Prompt 模板渲染不在本包**：`ResourceSelection.prompt_template` / `prompt_arguments` 与 `ResourceLimits.max_template_file_refs` / `max_rendered_prompt_bytes` 已声明并随类型导出，但本包不做模板发现与 `@file` 引用展开；当前整个 workspace 内这些字段无消费者（渲染归上层演进预留）。
- `WorkspaceService` 与 `FileIndex` 均为进程内状态：重启即失，持久化由上层负责；`FileIndex` 无单文件大小上限，超大生成物依赖 ignore 规则排除。
- config 的 Session / Run 层没有文件来源，只能经 API 注入；`discover*` 永不自动加入这两层。
- `providers[].base_url` 与 `providers[].use_proxy` 在非 Builtin/Global 层是**无条件剥离**（无回环例外）；`proxy_url` 的「回环与 `.local` 直连」语义是 `pawork-providers` 的运行时行为，不在本包。
- import 的 `hook.rs` / `mcp.rs` 类型是导入专用的平行定义，不是运行时 hook / MCP 配置的事实源；导入计划落地为运行时配置由上层（`pawork-app`）完成。
- `session_scan` 只发现文件不解析内容；会话内容解析与入库在上层 import 流程。
- 文件索引的二进制判定基于前 8KB 含 NUL 探测，可能误判无 NUL 的二进制格式为文本。
- `import/parse.rs` 对外部格式做宽松兼容（未知键记名不复制值），外部工具 schema 演进会产生新的 `unknown_key` 告警，属预期行为而非缺陷。
- `resources` 与 `import` 仍分两套 IO 错误类型与上限（`resources/io.rs` 委托 policy canonicalize；`import/io.rs` 自管 no-follow 读）。根无法 canonicalize 时 AGENTS.md 加载 fail-closed 为 OutsideRoot，不在无 within-root 检查时读文件。
- `ImportStatus::Disabled` 已声明并有展示映射（preview 输出 `disabled`），但当前源码无构造点：模块文档「无法安全映射标为 Unsupported / Disabled」实际只落在 Unsupported；导入 hook 的「默认禁用」由 `HookConfig.enabled=false` 表达而非条目状态。
- `LanguageServer` / `UserHook` 等 `ResourceKind` 变体已声明，但 resources 加载器当前只装载 Instructions / AgentsFile / Skill / AgentProfile 四类。
