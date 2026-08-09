# P2-12：Phase 2 评审修复（REVIEW remediation）

> Phase 2 · 首个真实 Provider · 状态：🟢已完成 · 交付成熟度：Validated · 依赖：P2-1 ~ P2-11

**最终目的**：消除 [REVIEW.md](../REVIEW.md) §2（Phase 2）发现的「mock 过得去、真实端点翻车」型正确性高危与基线/契约/文档漂移——让流式 usage 真实可得（Phase 14 额度的数据源）、长流不被 60s 超时掐断、取消语义与认证头正确，并收敛退避三方并存的死代码与 plan 文档未同步的流程偏差。

**涉及范围**：`provider-runtime`、`provider-openai-compatible`、`model-registry`、`test-support`、根 `Cargo.toml`、ROADMAP「依赖选型基线」、`docs/features/providers.md`、`plan/P2-*.md`

## 细分步骤（分组）

### A. 正确性高危（V1 / V2 / V3 / V4）

1. **V1 流式超时**：`provider-runtime/src/http.rs` 流式路径改用 reqwest `read_timeout`（按读操作重置），取消/大幅放宽覆盖全程的总 `timeout`。目的：长生成与慢本地模型不被中途掐断。
2. **V2 取消守卫**：删除 `http.rs` 两处 `if !cancel.is_cancelled()` 守卫，恢复预取消语义；将 `contract_cancel_mid_stream` 改为读到 delta 后真取消，并保留预取消用例断言「请求不应发出」（用 wiremock 命中计数）。目的：预取消不再误发请求。
3. **V3 include_usage**：`provider-openai-compatible` 请求体固定附加 `stream_options.include_usage = true`，正确处理尾部 usage-only chunk（`choices` 为空）。目的：真实 API 下 usage 不再恒为 0。
4. **V4 list_models 认证**：`list_models` 复用 `auth_header()` 携带 Authorization。目的：受保护 `/models` 不再 401。

### B. 正确性/质量中低（V5 ~ V10）

5. **V5 stop reason 归一**：`[DONE]` 而未见 finish_reason 时归一为 `Completed`（或 `Other("done")`），同步修 `map_stop_reason(None)` 语义。目的：本地服务收尾不被误记 Error，不误导重试判定。
6. **V6 provider_options 保留键**：定义保留键集合（model/messages/stream/tools 及认证字段），透传命中时忽略并告警。目的：防止覆盖 canonical 关键字段。
7. **V7 解析器有界缓冲**：SSE/JSONL `buf` 设上限（1 MiB，超限发解析错误并重置），非法字节改游标 `drain` 批量移除。目的：消除无限内存与 O(n²) 退化。
8. **V8 退避死代码**：删除 `provider-runtime/src/retry.rs` 的 `ExponentialBackoff`（带「cap 钳进 Retry-After」与「jitter 固定种子采样减半」两个 bug 的死代码）。目的：退避收敛为 agent-engine 单一来源。
9. **V9 resolve 大小写**：`model-registry` `resolve()` 入口 `to_ascii_lowercase` 归一（别名表构建同步归一）。目的：消除「不区分大小写」注释与精确匹配实现的矛盾。
10. **V10 调试输出**：删除 `tests/contract.rs` 遗留的 `println!("XXXURI...")`。目的：测试输出干净。

### C. 基线与包清理

11. **补登/移除**：在 ROADMAP「依赖选型基线」补登 `futures`、`bytes`（根 `Cargo.toml` 已声明）；移除 `backon`、`arbitrary`（声明未引用，`fuzz/` 缺位）；随 V8 删除 `provider-runtime` 的 `backon` 依赖行。目的：基线一致、无死声明。

### D. 契约与文档漂移

12. **契约套件补齐**：新增 timeout、reconnect 用例（P2-11 验收原文要求，[ADR-015](../docs/adr/ADR-015-provider-contract-tests.md)）；修复 `assert_error_kind` 空事件流 vacuous 通过（改为同时接收并强制断言其一）；cancel 用例按 V2 改造。目的：兑现 ADR-015 与 P2-11 自身验收。
13. **plan/docs 同步**：11 篇前置 `plan/P2-*.md` 状态回填 🟢、现有 22 个验收框全部勾选；`providers.md` 补 `include_usage` 与 stop reason 语义说明。目的：纠正违反 AGENTS.md §4 的流程偏差。

## 主要产出物

- http.rs read_timeout + 取消守卫修复；include_usage + list_models 认证；stop reason 归一、provider_options 保留键、解析器有界缓冲、ExponentialBackoff 删除
- ROADMAP 基线补登/依赖移除（futures/bytes/backon/arbitrary）；timeout/reconnect 契约用例 + assert_error_kind 修复
- 11 篇 plan 回填 + providers.md 语义说明

## 验收标准（保留 REVIEW 追踪编号）

- [x] **V1**：慢速长流（>60s）不被超时掐断（契约用例）
- [x] **V2**：预取消不发出请求（wiremock 命中计数 0）；mid-stream 取消用例读到 delta 后取消
- [x] **V3**：请求体含 `stream_options.include_usage`，尾部 usage-only chunk 正确归一（断言请求体字段）
- [x] **V4**：`list_models` 请求头含 Authorization（契约断言）
- [x] **V5**：`[DONE]` 无 finish_reason 时 stop_reason 非 Error（用例）
- [x] **V6**：provider_options 命中保留键被忽略并告警（测试）
- [x] **V7**：解析器 buf 超 1 MiB 发错误并重置；非法字节批量移除（不退化 O(n²)）
- [x] **V8**：`ExponentialBackoff` 已删除，生产退避仅 agent-engine 一处
- [x] **V9**：`resolve("GPT-4O")` 与 `resolve("gpt-4o")` 等价（测试）
- [x] **V10**：`tests/contract.rs` 无调试 println
- [x] **基线**：ROADMAP「依赖选型基线」补登 `futures`/`bytes`（根 `Cargo.toml` 已声明），移除 `backon`/`arbitrary`，ROADMAP 基线表同步
- [x] **契约**：timeout、reconnect 用例存在且通过；`assert_error_kind` 不再 vacuous 通过
- [x] **文档**：11 篇 `plan/P2-*.md` 状态 🟢、全部 22 个验收框勾选；providers.md 补 include_usage/stop reason 语义
- [x] **快速验证**：只运行 Provider/HTTP/parser/auth 受影响 crate 的定向测试；仅在 schema 实际变化时定向检查生成物，Phase 1～7 remediation 收尾后统一执行 Core 主干 L2

## 验证记录

- 2026-08-09：受影响的 Provider / runtime / registry / contract helper / auth crate 定向测试通过。
- 2026-08-09：上述 crate 的 `cargo clippy --all-targets -- -D warnings` 与定向 `cargo fmt -- --check` 通过。
- 本任务未修改 schema，未触发生成物检查；Core 主干 L2 仍按 Phase 1～7 remediation 的统一收尾节奏执行。

**相关文档**：[REVIEW.md](../REVIEW.md) §2 · [ADR-015 Provider 契约测试](../docs/adr/ADR-015-provider-contract-tests.md) · [providers](../docs/features/providers.md) · [ROADMAP 依赖选型基线](../ROADMAP.md#依赖选型基线)
