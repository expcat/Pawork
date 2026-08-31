# ADR-044：持久项目注册表与会话级 Workspace 路由（schema v14）

- **状态**：Accepted（用户 2026-08-31 确认）
- **日期**：2026-08-31

## 背景

P1 片 1 已把 Session→Workspace 的弱引用归属持久化到 `sessions.workspace_id`，并通过 Host 重启真窗口复验。片 2 要让“添加 / 切换 / 重开项目”和“新建 / 续聊会话”形成完整正式 UI 路径。当前实现仍不足以承载该目标：

- `AppCore::attach_workspace` 每次新建一个 `WorkspaceService`，把唯一项目写成固定 id `ws-default`，并替换此前项目；`workspace_add` 实际是“替换单例”，不是添加到项目集合。
- snapshot / `workspace_list` 只返回当前单例；Host 重启后没有权威的 `workspace_id → canonical root` 注册表。
- Run、资源注入、文件索引与 session diff 仍读取全局 `extensions.workspace_*`；即使 Session 记住了 workspace id，也不能据此恢复该会话自己的执行根。
- 现有 GUI Connection Protocol 已有 `workspace_add`、`workspace_list`、Workspaces snapshot 数组、`session_create{workspace_id}`，以及 Diff / Terminal 的 `workspace_id` 参数。缺口在 Host 持久态与路由，不需要先新增 wire 变体。

项目注册表属于本地宿主权威状态。当前 session schema 是冻结契约；若把注册表放入 SQLite，须按 ADR 追加 schema 版本。Desktop 侧车或本地偏好会形成第二套项目事实源，不符合单一 Host 权威边界。

## 拟议决策

### D1 — schema v14 追加本地 `workspaces` 注册表

- 在 session SQLite 追加 `workspaces(workspace_id, name, root_path, created_at_ms, updated_at_ms)`；`workspace_id` 为主键，canonical `root_path` 唯一。
- 当前正式 UI 一次只选择一个目录，所以 v14 只持久化单 root；不为未来多 root 预建子表。
- 注册表不进 session import / export；`sessions.workspace_id` 继续是无 FK 的弱引用，避免删除或暂不可达项目阻断历史会话读取。
- 注册表本身不进入 session import / export，也不为此新增 Agent 事件或 Provider 上下文；现有工具事件与诊断中的路径行为不在本 ADR 内扩大或改写。

### D2 — `workspace_add` 变为幂等注册，不再替换单例

- Host 先 canonicalize 目录；同一 canonical root 重复添加返回原 stable opaque `workspace_id`，首次添加才生成并持久化 id。
- Host 启动从注册表加载全部项目到一个 `WorkspaceService`；Workspaces snapshot 与 `workspace_list` 返回完整集合。
- CLI 启动目录也走同一注册入口，不再绕过注册表维护第二个“当前 workspace”。

### D3 — 会话归属决定执行上下文

- RunStart、资源注入、`@file`、file-index、工具 `ToolExecutionContext`、checkpoint / diff 均先由 Session→Workspace 绑定解析 registry root。
- 绑定缺失或 workspace 未登记时 fail-closed：UI 继续诚实显示 Unassigned / unavailable，不得回退到启动目录或最近打开项目。
- Desktop 的 Scope 只负责筛选与选择新 Task 的 workspace；打开已有 Task 后，以 Host 返回的 session 归属为权威。Terminal / Diff 继续复用已有 workspace id 参数。

### D4 — wire 保持不变

- 不新增 command / query / event，不提升 GUI API 版本；只把既有 Workspaces 数组与 `workspace_list` 从单例实现补成真实集合。
- 若实现时证明现有载荷无法表达必要状态，停止编码并另立 wire ADR，不在本 ADR 内顺手扩协议。

### D5 — legacy `ws-default` 的一次性兼容边界

- v14 首次打开且注册表为空时，把本次显式启动目录登记为 `ws-default`，保留 ADR-043 已落盘绑定；不改写任何 session 行。
- 旧实现曾把不同目录都压成 `ws-default`，历史数据无法可靠反推原 root；不做猜测性拆分。该实例后续新项目使用新 opaque id，旧歧义保持可观察并在 UI 标注 legacy，而不是静默重绑。

## 否决支

- **Desktop 本地项目列表 / sidecar JSON**：形成第二事实源，Host 重启与多客户端会漂移。
- **继续复用固定 `ws-default`**：无法区分项目，Session 持久归属失去产品语义。
- **每次重启要求用户手工重加项目**：不能满足 P1 的重开项目与续聊退出条件。
- **以 canonical path 哈希直接作为 id**：把路径派生规则冻结为外部身份，目录移动或平台大小写规则变化时不稳定；持久 opaque id 更小、更清晰。

## 后果与实施切片

- Accepted 后先做 storage v14 迁移、升级 golden 与注册表 API；再改 AppCore / GuiHost 的多项目装配和按 session 路由；最后只补 Desktop 仍缺的正式 UI 接线与真窗口验收。
- 预计写入包为 `pawork-storage`、`pawork-app`、`pawork-workspace`，Desktop 仅在现有交互不能消费真实集合时最小修改；不新增 crate 或生产依赖。
- 本 ADR 已获用户确认；实施仍按 storage → AppCore/GuiHost → Desktop 的切片推进，wire 保持不变。
