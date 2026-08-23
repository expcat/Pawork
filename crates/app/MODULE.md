# pawork-app

Core 装配宿主：配置、session store、`run_session`、GUI server。领域 crate 的唯一生产汇合点。

## 职责

把 engine / providers / tools / policy / storage / git / workflow / orchestration / control-plane 焊成 `AppCore`，并实现 `GuiHost` 供本机多客户端 GUI 连接。R4 把巨 match 拆成 `services/` + `gui_host/` 静态分发表。外部 GUI **不得**依赖本包，只经 protocol + transport 连 CLI。

## 模块树

```
src/
  lib.rs                     # AppCore
  approval.rs  auth.rs  channels.rs  checkpoint.rs  control.rs
  data_dir.rs  diff.rs  extensions.rs  hub.rs  idempotency.rs
  import_host.rs  loop_ctx.rs  orchestration_host.rs  persist.rs
  plan_host.rs  protected.rs  protocol.rs  provider_assembly.rs
  tasks_host.rs
  gui_server/{mod,connection,session}.rs     # pub mod
  gui_host/{mod,bus,events,handlers/*}.rs    # 私有；类型再导出
  services/{mod,session,run,approval,usage,tasks,import,extension}.rs  # pub(crate)
tests/
  smoke.rs  timeline_projection_host.rs  gui_server/
```

## 对外入口/API 面

- **AppCore**：`load*` / `from_config` / `from_parts`；会话 `create_session` / `resume_messages*` / `chat_turn*`；模型与 auth；MCP；diff/checkpoint；usage；plan/tasks；compat import；`run_multi_agent_demo`。
- **gui_server**：trait `GuiHost`；`GuiServer` / `ConnectionManager`。断线不取消 Run。
- **gui_host**：`GuiHostAdapter`；query/command 走 protocol registry 派生的分发表（与 `gui.available` 双射）。Timeline 映射 = `pawork_protocol::projection::project_event`。
- 其它根 re-export：审批宿主、EventHub、IdempotencyStore（底层 CommandLedger）、首发通道 facade（`FIRST_PARTY_CHANNELS` ← `CHANNEL_REGISTRY`）、diff 渲染等。

`services/` 与 `CatalogOnlyProvider` 不是公开 API。

## 依赖与被依赖

- **依赖**：domain、engine、providers（六通道 feature 全开）、auth、tools、policy、workspace、exec、storage（compaction/checkpoint/protected）、git、workflow、orchestration（`default-features=false`）、control-plane、protocol、transport。
- **被依赖**：生产仅 `pawork-cli`；`pawork-client` 为 dev-dep。desktop **禁止**依赖本包。

## 红线与注意事项

- 明文 key 不得进入 `AppError` 任何变体；`auth_status` 脱敏。
- 不按 Provider 名称分支；通道表是 providers registry 的 facade。
- GUI resume 保留待审批（`resume_messages_keep_pending`）；CLI resume 仍 seal Denied。
- resume/fork/compact 直接消费 storage lineage；fork resume 只能看到祖先前缀，compaction 存储错误显式上抛而非降级为“未发生”。
- 幂等 `record` 失败要计数并释放 inflight，不可吞错挂死。
- `home` 回退经 `DataDirOutcome` 结构化告警，禁止静默落到 temp。
- Reasoning 保护：`protected.rs` 注入 `ProtectedBlobStore`（instance-level `BlobScope` `instance-reasoning` 为已接受偏差）。

## 相关文档

- [docs/design.md](../../docs/design.md) §2
- [plan/R3-protocol-unification.md](../../plan/R3-protocol-unification.md) / [plan/R4-host-decomposition.md](../../plan/R4-host-decomposition.md)
- [代码地图总索引](../../docs/code-map/README.md)
