# pawork-workspace

workspace roots、相对路径、文件索引、六层配置、资源加载与兼容导入。依赖 `pawork-domain`、`pawork-policy`。

## 职责

维护工作区目录（canonicalize 后的 roots）、把相对路径交给 policy 内核做越界检查、扫描/监视文件索引、按 Builtin&lt;Global&lt;Profile&lt;Workspace&lt;Session&lt;Run 合并 TOML 配置、加载 AGENTS.md / Skills / profiles，只读导入本机 Claude/Codex/Grok/Cursor/Pi 配置，以及只读发现本机 Claude Code / Codex 会话文件（R6 波 C）。R1 波 B 并入原 resources / config / compat。

## 模块树

```
src/
  lib.rs                 # Workspace / WorkspaceService
  path.rs                # 相对路径（Windows 设备名等）
  file_index.rs
  config/{loader,merge,paths,schema,error}.rs
  resources/{loader,agents,skills,profiles,request,...}.rs
  import/{detect,parse,apply,source,mcp,session_scan,...}.rs
tests/
  loader_file.rs  smoke.rs
fixtures/
  AGENTS.md  CLAUDE.md
```

## 对外入口/API 面

`pub mod config` / `import` / `resources`；crate 根 re-export 索引与路径类型，并定义：

- `Workspace`、`WorkspaceService`（`add` / `get`；内存目录，无 list/remove）。
- `resolve_relative_path(roots, relative)` → `ResolvedPath`；symlink / `.git` / TOCTOU 走 `pawork_policy::resolve_workspace_path`。
- `FileIndex`：`scan_workspace` / `search` / `watch_workspace` / `start_debounced_updates`。默认排除 `.git`、`node_modules`、`target` 等。
- **config**：`Loader`（`discover` / `with_session` / `with_run` / `resolve`）、`PaworkConfig`、`ProviderConfig`（**无 `api_key` 字段**；`extra` 会剥掉该键）、`ConfigTier`。路径常量：`APP_QUALIFIER="dev"`、`APP_ORGANIZATION="pawork"`、`APP_APPLICATION="pawork"`、全局 `config.toml`、工作区 `.pawork/`。
- **resources**：`ResourceLoader::load`；`ResourceRequest = workspace_id + root_index + WorkspaceRelativePath`。
- **import**：`CompatLoader::{scan, dry_run, export_plan}`；`ExternalSource::{Claude,Codex,Grok,Cursor,Pi}`。扫描/预览不执行、不联网。**会话发现**（R6 波 C）：`scan_local_sessions(LocalSessionSource, &LocalSessionRoots)` 只读列出 `~/.claude/projects/**/*.jsonl` 与 `~/.codex/sessions/**/rollout-*.jsonl`（`LocalSessionRoots::detect/from_home`；有界、不跟 symlink、根缺失返回空、只取元数据不读内容；Claude 排除 `agent-*.jsonl` subagent sidecar，因其 `sessionId` 复用父会话）。

## 依赖与被依赖

- **依赖**：`pawork-domain`、`pawork-policy`。`directories` / `notify` / `toml` / `ignore`。无 feature。**不**依赖 auth（env 名在 R5 后只走 `pawork-auth::locator`）。
- **被依赖**：`pawork-app`、`pawork-tools`（八工具的 `WorkspaceService`；MCP 读 `ResolvedConfig`）。

## 红线与注意事项

- 文件输入一律 `workspace_id + relative_path`（或 `root_index`）；禁止模型传入任意绝对路径作索引键。
- 配置 schema 无 Secret；导入把 secret 降为引用，检测到明文则失败。
- 导入不执行 hook / 不拉起 MCP；会话发现只列路径与大小，解析与 Secret 扫描在 storage persist 入口。
- 六层合并是冻结契约；S9 已承诺 `with_session` / `with_run` 保留。

## 相关文档

- [docs/design.md](../../docs/design.md) §3.2 配置 schema
- [docs/task-guide.md](../../docs/task-guide.md)（路径红线）
- [代码地图总索引](../../docs/code-map/README.md)
