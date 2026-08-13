# P17-7：ACP Host（Agent Client Protocol 适配宿主）

> Phase 17 · Ecosystem & Host Compatibility · 状态：✅已实现（HostWired） · 交付成熟度：HostWired（历史代码交付≠产品验收） · 依赖：P0-8、P13-1、P18-10

**最终目的**：在 `core-api` 之上新增一个可替换的 Host Adapter——ACP（Agent Client Protocol）Host，使外部 ACP 客户端能通过标准 Agent 客户端协议接入 Pawork Core。ACP Host 只做协议翻译：把 ACP 的 session/task/message/tool/event 映射到 `core-api` 的 `AppCommand`/`AppQuery`/`AppEvent`，不承载业务逻辑，也不取代 GUI Connection Protocol——GUI 仍只经 GUI Connection Protocol 接入，ACP 是另一条面向生态互操作的可选接入通道（[ADR-030](../docs/adr/ADR-030-core-sole-source-of-truth.md)）。

**涉及范围**：新增 `acp-host` crate；实现 `client-adapter-api::ClientAdapter`；复用 `core-api`、`app-service`、`agent-events`、`subscription-hub`（订阅事件流）；不动 `gui-protocol` / `gui-server` / `agent-engine`。

## 细分步骤

1. **ACP 协议模型与映射表** —— 目的：实现统一 ClientAdapter 契约，覆盖 initialize/capabilities、session create/resume/prompt/update、permission/tool event、cancellation；与 `core-api` 建立显式映射表，未知 method 返回 `ProtocolUnsupported`。
2. **ACP Host 适配层** —— 目的：实现一个监听 ACP 客户端的 server（stdio / 本地 socket），把入站 ACP 请求转成 `AppCommandEnvelope`（`CommandSource` 标注为互操作来源）交给 `app-service`，把 Core 事件回译成 ACP 事件流。
3. **与 GUI Connection Protocol 边界隔离** —— 目的：ACP Host 独立于 `gui-protocol`/`gui-server`，复用同一 `app-service` 与 Event Hub，但走自己的协议层；明确「GUI 不得经 ACP 接入、ACP 客户端不被当作 GUI」，避免两套接入语义耦合。
4. **能力协商与降级** —— 目的：在握手期协商 ACP 客户端支持的能力（streaming/tool result/approval），Core 不具备的能力显式降级或拒绝，协商结果写入事件来源记录，便于审计。
5. **`pawork acp serve` 接线** —— 目的：在 `cli-host` 暴露一个可选子命令启动 ACP Host（与 `serve`/`shell` 并列），不改 `cli-host` 既有装配；ACP Host 失败或关闭不影响 GUI 与 CLI 既有模式。
6. **定向 / Golden 测试** —— 目的：用 versioned fixture 覆盖「请求翻译 → Core 执行 → 事件回译」全链路、握手协商、resume/cancel、permission/tool event、unsupported method 与 GUI 边界隔离。完整矩阵由 P18-15 集中门禁。

## 主要产出物

- `acp-host` crate：ACP 类型 + 与 core-api 映射表 + Host 适配层
- `pawork acp serve` 可选子命令（不改 cli-host 既有装配）
- 定向测试（翻译全链路 / 协商降级 / 与 GUI 协议边界隔离）

## 验收标准

- [x] ACP Host 是 `core-api` 之上的纯协议翻译，不含业务决策，不修改 `agent-engine` / `gui-protocol` / `gui-server`
- [x] GUI Connection Protocol 仍是 GUI 的唯一接入通道，ACP Host 不取代它
- [x] ACP 请求经映射产生合法 `AppCommandEnvelope`，Core 事件可回译为 ACP 事件流
- [x] 能力协商结果显式记录来源，不支持的能力降级而非静默失败
- [x] ACP adapter 复用 P18-10 Session Registry/capability snapshot，不私建 ownership 或 credential 状态
- [x] 定向 / Mock smoke 覆盖翻译全链路与边界隔离断言

## 收尾记录（2026-08）

- 并发 RunStart 因果绑定回归 `concurrent_run_starts_carry_distinct_run_ids_bound_to_their_own_runs`
  （`crates/app-service/tests/router_integration.rs`）：并发来源各自从 Accepted 响应取
  run id 绑定，不依赖全局 `last_started_run`（该字段已随 run_id 迁移移除）。
- `pawork acp serve` 真实进程 e2e（`apps/pawork/tests/acp_e2e.rs`）通过；修复 cwd 解析
  symlink 别名误判：`AcpHost::CwdResolver` 双侧 canonicalize 后再做前缀匹配（macOS
  `/var` → `/private/var`），新增回归 `session_new_matches_cwd_across_normalization_aliases`。
- core-api schema typegen 已同步（`AppResponse` 的 `run_id` 字段落盘），
  `cargo run -p schema-typegen -- --check` 通过。
- acp-host / cli-host 定向 test（acp-host fixtures 11 + floor 14、cli-host 31 + acp_stdio 1、
  pawork acp e2e 2）与 `cargo clippy -p acp-host -p cli-host --all-targets -- -D warnings` 全绿。
- 评审修复：`session/resume` 的 `mcpServers` / `additionalDirectories` 按官方 builder
  改为 optional/default（省略即空数组），`session/new` 的 `mcpServers` 必填保持；
  补缺省 golden fixture `fixtures/v1/session-resume-minimal.json` 与全链路回归
  `session_resume_omitted_builder_defaults_are_accepted`。
- 评审修复：出站消息统一走单一有序 outbox（`OutboxItem::Frame` + `FlushBarrier`），
  prompt 终态屏障排在 run 的全部 `session/update` 帧之后，由传输层冲刷时先写出
  帧再释放完成信号——`session/prompt` 响应保证在本 prompt 全部更新写出后才返回；
  传输失败时 `resolve_queued_prompts` 就地释放剩余屏障，等待中的 prompt 不悬挂。
- 评审新增：ACP host 两 session 并发 prompt 因果 run_id 回归
  `concurrent_prompts_across_two_sessions_carry_distinct_causal_run_ids`（各自绑定
  自己的 run id、update 不串流、各自 Completed 收敛）；真实 SQLite 跨 host/进程
  resume e2e `acp_serve_resume_across_processes_uses_sqlite_session_db`
  （进程 A 建会话并 close 落盘，进程 B 同 `session.db` resume 后继续 prompt）。
- 评审回归修复（并发因果测试首跑暴露）：host 生成的取消信封
  （`session-cancel` / `cancel-request` / `cancel-permission`）改为携带
  run_id / 权限请求 id 的唯一 command id——此前固定标签会被 app-service
  幂等缓存按 command_id 判重 replay，第二 session 的 `session/cancel`
  被静默吞掉、run 永不终态。
- 跨 host/进程 resume 实现补充：持久化记录对应的 core session 不在本 Core
  aggregate（内存态）时，用客户端 `cwd` 重建 core session，并把记录经
  registry 单次 ownership/revision CAS 重绑到新 core_session_id
  （`SessionRegistry::rebind_core_session`：epoch 不变、revision 单调递增）
  后继续 Reattach claim，reattach 使用重绑后的新 handle；不私建 ownership
  状态，也不复制 registry/Core 状态机。
- 评审修复（跨进程 resume 原子性）：把上条的 registry `remove` → `register`
  两步 remap 改为单次 CAS 重绑。旧实现存在崩溃窗口——进程在 remove 之后、
  register 之前崩溃会永久丢失 client_session_id → core_session_id 映射，
  并发 resume 还会互相删除对方刚注册的行；单次 CAS（SQLite
  `UPDATE ... WHERE epoch/revision` / 内存互斥锁内原子替换）保证映射在
  任何时刻都不缺失，冲突时权威记录同步回缓存并随 `StaleOwner` 暴露。
  新增定向测试：`rebind_*`（client-adapter-api 内存 registry 4 项：
  语义/并发恰一胜/陈旧 handle 暴露权威记录/缺失会话）、
  `sqlite_rebind_*`（session-store 3 项：崩溃重开映射不丢、并发恰一胜、
  行缺失 resync 为 UnknownSession）、floor
  `resume_across_restart_rebinds_core_session_atomically`（host A 落盘 →
  host B 全新 Core aggregate + 同一 `session.db` resume，重绑 + claim 后
  revision 2→4、新 handle 在 aggregate 中存在、继续 prompt 往返），真实
  进程 e2e `acp_serve_resume_across_processes_uses_sqlite_session_db` 继续
  覆盖跨进程恢复路径。
- occupancy 泄漏修复（2026-08-13）：`handle_request` 在 decode 失败、未匹配
  canonical 命令、以及 `session_prompt` 取 `session_context` 失败时，都会
  `release_reservation` / `release_occupancy`，避免未知 session 的
  `session/prompt` 永久占住 occupancy。新增 floor 回归
  `unknown_session_prompt_releases_occupancy`（未知 session 返回 `-32002`，
  `!has_active_runs()`，随后合法 prompt 仍能 `end_turn`）。
- `cargo test -p acp-host --test floor -- --test-threads=1`：23 项通过。
- Validation Level：L1；独立 occupancy 审查卡住未给出裁决，故保持 Built + L1，
  不标已验收。

**相关文档**：[CLI Host](../docs/features/cli-host.md) · [GUI 连接与多客户端](../docs/features/gui-connection.md) · [ADR-017 GUI 不直连 Core](../docs/adr/ADR-017-gui-no-direct-access.md) · [ADR-030 Core 单一事实源](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [workspace-layout](../docs/architecture/workspace-layout.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：不新增第三方依赖；复用 `core-api` / `app-service` / `agent-events` / `subscription-hub`。新 crate `acp-host` 依赖方向：`core-api → acp-host → cli-host`（可选接线），与 `gui-server` 平级、互不依赖。
