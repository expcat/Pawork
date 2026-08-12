# Agent 控制流

## 1. 命令入口与事件分发

CLI 与 GUI 发起的所有命令统一进入同一个 Command Router，再交由 `app-service` 执行；执行产生的 Core Event 统一进入 Event Hub，以相同顺序扇出到 CLI 渲染器与所有 GUI。

```text
CLI Command ─┐
Local GUI ───┼─→ Command Router → app-service → core-runtime
Remote GUI ──┘                                   │
                                                 ▼
                                             Event Hub
                          ┌── CLI Renderer ───┐
                          ├── Local GUI A/B ──┼── 相同事件顺序
                          └── Remote GUI C ───┘
```

每条命令携带 `CommandSource` 与身份（见 [GUI Connection Protocol §3](api-surface.md)），所有状态变化可追溯来源；每条事件携带 `global_sequence`，CLI 与 GUI 看到完全一致的状态演进。Phase 19 Desktop 先应用 Snapshot，再按 sequence 更新可丢弃 projection；缺口无法补齐时重新 Snapshot，不在视图层猜测权威状态。命令路径、统一 Command Source、Event Hub 与多客户端同步见 [GUI Connection Protocol](api-surface.md)、[GUI 连接与多客户端](../features/gui-connection.md) 与 [Desktop GUI](../features/desktop-gui.md)。

## 2. Agent Loop

每轮执行：

1. 解析 Session 状态
2. 加载工作区上下文
3. 计算 Token Budget
4. 判断是否需要 Compaction
5. 构建 `RouteContext` 与 Provider Request
6. 经 TenantPolicy、RoutingPolicy 与 CredentialPool 获取 `CredentialLease`，再调用 Provider
7. 流式提交 Assistant 内容
8. 解析 Tool Call，并按 `ToolKind` 分类执行位点
9. 执行 Policy；需要时请求用户审批
10. 仅对 `ClientFunction` 调用 Tool Scheduler 本地执行
11. `ClientFunction` 提交 `ToolResult(CoreSuppliedResult)`；`ProviderHosted` / `ProviderExtension` 只记录 `ServerToolEvent(ProviderTranscript)`
12. 按 `ContinuationMode` 构建下一轮 Provider continuation，不跨位点伪造结果
13. 判断是否继续模型循环
14. 用 `LeaseOutcome` 释放租约并提交最终状态；cancel 不降低 account health

状态机与事件见 [领域模型](domain-model.md)。

### 2.1 Provider 资源控制（Phase 18）

```text
RouteContext
   ↓ capability filter
TenantPolicy filter
   ↓
Health + priority + affinity + weight/fill-first
   ↓ concurrency admission
CredentialLease
   ↓
ModelProvider
   ↓
ErrorClassifier → retry same credential / failover credential /
                  fallback model / fallback provider / fallback protocol
```

`ModelProvider` contract 不变；Provider Runtime 的 transport retry（P2-10）与账号健康/轮换（P18-5）分层。`ClientCancelled`、`InvalidRequest`、`ContextTooLarge`、`ProtocolIncompatible` 不默认触发 credential rotation。

## 3. 运行控制

必须支持：

- Cancel
- Pause
- Resume
- Retry Last Provider Call
- Retry Run
- Fork From Message
- Replace Queued Messages
- 修改模型
- 修改 Thinking Level
- 修改预算
- 手动 Compaction
- 恢复 Interrupted Run

## 4. 预算控制

预算类型：

```text
最大 Agent 迭代次数
最大 Tool Call 次数
最大运行时间
最大输入 Token
最大输出 Token
最大费用
最大 Shell 输出
最大 Artifact 大小
最大并发 Tool Call
```

达到预算后必须生成明确事件，而不是静默停止。

## 5. Tool Call 调度

默认策略：

- 写操作串行
- Shell 操作默认串行
- 只读文件操作可并发
- 搜索工具可并发
- 相同文件上的操作必须串行
- Git Index 操作必须串行
- 用户审批期间暂停相关调用
- 所有调用都可取消

Tool Scheduler 按 capability 分类判断调度：

```text
ReadOnly
WorkspaceWrite
GitWrite
Process
Network
UserInteraction
ExternalPlugin
```

### 5.1 ToolKind 三执行位点路由（Phase 15 起）

自 Phase 15（P15-1）起，Tool Scheduler 按 `ToolKind` 三执行位点决定「谁执行、结果如何回填」，三类位点互不串味（早期规划的 `ExecutionOwner` 冗余枚举已按 P15-10 删除，位点语义由 `ToolKind` 直接承载）：

```text
ClientFunction   → Core 本地执行 → ToolResult(CoreSuppliedResult) → 回灌 Provider
ProviderHosted   → Provider 自执行 → Core 只记录/归一/重放 → ServerToolEvent(ProviderTranscript)
ProviderExtension→ Provider 中介外部通道执行 → Core 审批/审计 → 回填(ProviderTranscript)
```

- `ProviderHosted` / `ProviderExtension` 的 tool_call **不触发本地 `AgentTool::execute()`**；结果属 `ContinuationMode::ProviderTranscript`，归一为 [P15-5](../../plan/P15-5-server-tool-events.md) `ServerToolEvent`，不伪装成本地 `ToolResult`。
- `ClientFunction` 才走 §5 的本地调度策略（写串行 / 只读并发 / 审批暂停等）。
- Browser/Computer（P17-10）等能力 facade 也必须服从该位点模型：Local/Playwright→ClientFunction、ProviderHosted Computer Use→ServerToolEvent、MCP→ClientFunction 或 ProviderExtension。

详见 [tools](../features/tools.md) 与 [policy](../features/policy.md)。

## 6. 相关文档

- [领域模型](domain-model.md)
- [GUI Connection Protocol](api-surface.md)
- [GUI 连接与多客户端](../features/gui-connection.md)
- [Desktop GUI](../features/desktop-gui.md)
- [agent-engine](../features/agent-engine.md)
- [tools](../features/tools.md)
- [context](../features/context.md)
- [providers](../features/providers.md)
- [Provider Control Plane](../features/provider-control-plane.md) · [Tenant、Usage 与 Audit](../features/tenant-audit.md)
- [ADR-033 控制面分离](../adr/ADR-033-control-plane-separation.md) · [ROADMAP Phase 15 / Phase 18](../../ROADMAP.md)
