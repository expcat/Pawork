# 代码地图（三层）

给 Agent 与协作者按需加载的包级导览。公开细节以 rustdoc / 源码为准；本目录不替代 [design.md](../design.md) 的冻结契约。

## 怎么用

1. **先看本页**：确认包在依赖图中的位置，再打开对应 `MODULE.md`。
2. **按 crate 加载**：进入某包工作时读该包根目录的 `MODULE.md`，不要一次读完全仓。
3. **热点最后**：跨包热路径见 [hotspots/](hotspots/)。

`MODULE.md` 固定六节：职责 · 模块树 · 对外入口/API 面 · 依赖与被依赖 · 红线与注意事项 · 相关文档。函数级不铺开。

布局与依赖方向的权威表是 [design.md](../design.md) §2（21 成员，ADR-039）。本索引按**依赖自底向上**排列，与任务书 [plan/out-of-band/code-map.md](../../plan/out-of-band/code-map.md) 提交队列一致。

## 第 1 层：包清单

无内部 `pawork-*` 依赖的叶子在上；宿主与两个二进制在下。反向依赖见各 `MODULE.md`「依赖与被依赖」。

| # | 包 | 目录 | `MODULE.md` | 一句话 |
| --- | --- | --- | --- | --- |
| 1 | `pawork-domain` | `crates/domain` | [MODULE.md](../../crates/domain/MODULE.md) | canonical 类型、事件信封、`provider_api` / `tool_api` |
| 2 | `pawork-exec` | `crates/exec` | [MODULE.md](../../crates/exec/MODULE.md) | 进程 / 沙箱 / PTY（无内部 pawork 依赖） |
| 3 | `pawork-transport` | `crates/transport` | [MODULE.md](../../crates/transport/MODULE.md) | 有界字节帧：local UDS/pipe + memory |
| 4 | `pawork-protocol` | `crates/protocol` | [MODULE.md](../../crates/protocol/MODULE.md) | GUI 帧 / headless-json / core-api / 投影 reducer |
| 5 | `pawork-testkit` | `crates/testkit` | [MODULE.md](../../crates/testkit/MODULE.md) | dev-only MockProvider / MockTool / 契约断言 |
| 6 | `pawork-policy` | `crates/policy` | [MODULE.md](../../crates/policy/MODULE.md) | 安全内核：`PolicyDecision` / `ApprovalMode` |
| 7 | `pawork-auth` | `crates/auth` | [MODULE.md](../../crates/auth/MODULE.md) | Secret 后端 / OAuth / 脱敏 / locator |
| 8 | `pawork-storage` | `crates/storage` | [MODULE.md](../../crates/storage/MODULE.md) | sqlite actor、session 事件、PWB1 blob |
| 9 | `pawork-providers` | `crates/providers` | [MODULE.md](../../crates/providers/MODULE.md) | net + registry + 六通道（feature 门控） |
| 10 | `pawork-workflow` | `crates/workflow` | [MODULE.md](../../crates/workflow/MODULE.md) | plan / task 纯 reducer |
| 11 | `pawork-control-plane` | `crates/control-plane` | [MODULE.md](../../crates/control-plane/MODULE.md) | 控制面 core + quota + credential lease/pool |
| 12 | `pawork-workspace` | `crates/workspace` | [MODULE.md](../../crates/workspace/MODULE.md) | 路径 / 索引 / resources / 六层 config / 导入 |
| 13 | `pawork-git` | `crates/git` | [MODULE.md](../../crates/git/MODULE.md) | Diff / Status / GitService / HunkStage / worktree |
| 14 | `pawork-tools` | `crates/tools` | [MODULE.md](../../crates/tools/MODULE.md) | 八工具 + scheduler + MCP client |
| 15 | `pawork-engine` | `crates/engine` | [MODULE.md](../../crates/engine/MODULE.md) | tool loop / session turn；生产依赖仅 domain |
| 16 | `pawork-orchestration` | `crates/orchestration` | [MODULE.md](../../crates/orchestration/MODULE.md) | supervisor / budget / lifecycle / task_graph |
| 17 | `pawork-client` | `crates/client` | [MODULE.md](../../crates/client/MODULE.md) | framed 连接面 + headless SDK |
| 18 | `pawork-app` | `crates/app` | [MODULE.md](../../crates/app/MODULE.md) | 装配宿主 + gui_server / gui_host |
| 19 | `pawork-cli` | `crates/cli` | [MODULE.md](../../crates/cli/MODULE.md) | 子命令 + ACP 通道 |
| 20 | `pawork`（bin） | `apps/pawork` | [MODULE.md](../../apps/pawork/MODULE.md) | composition root + 日志脱敏 |
| 21 | `pawork-desktop`（bin） | `apps/desktop` | [MODULE.md](../../apps/desktop/MODULE.md) | GPUI 四层；业务依赖仅 `pawork-client` |

## 第 2 层：模块图

上表链接即各包根目录已落地的 `MODULE.md`。

## 第 3 层：热点

跨包路径见 [`hotspots/`](hotspots/)，不要塞进单一 crate 的模块图。

## 相关文档

- 包布局与冻结契约：[design.md](../design.md) §2 / §3
- 工作红线：[../../AGENTS.md](../../AGENTS.md) §2
- 任务书：[../../plan/out-of-band/code-map.md](../../plan/out-of-band/code-map.md)
- 任务总索引：[../../ROADMAP.md](../../ROADMAP.md) §3.1
