# Phase 2 Review：Provider 运行时、OpenAI-compatible 适配、认证与模型目录

- **日期**：2026-08-08
- **评审基线**：`main` @ `67d6c4d`（工作树干净）；Phase 2 由单一提交 `a8cd17d` 交付（31 文件，+6273 行）
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置；后续再研究是否采纳）
- **范围**：ROADMAP.md Phase 2 的 11 个任务（P2-1 ~ P2-11）的完成情况、所引入包是否合适、基线偏差；漏洞与优化点一并列出。Phase 3/6 构建在 Phase 2 地基之上（如 provider-openai 直接委托 openai-compatible 引擎），受影响处标注「传播面」。

### 1. 结论摘要

1. **完成度基本可信，但「测试绿」与「真实可用」的差距比 Phase 1 大**：P2-1 ~ P2-11 全部 🟢，5 个交付面（provider-runtime / provider-openai-compatible / auth-service / model-registry / test-support）复跑共 **120 passed / 0 failed**；`clippy -D warnings`、`fmt --check`、`schema-typegen --check` 均干净。
2. **四个高风险问题全部是「mock 过得去、真实端点会翻车」型**：reqwest 总超时 60s 覆盖流式全程，长生成必被掐断（V1）；select! 守卫使预取消失效、请求照发（V2）；OpenAI 流式从不请求 `include_usage`，真实 API 下 usage 恒为 0（V3）；`list_models` 不带认证头，任何受保护的 `/models` 端点 401（V4）。
3. **包选型总体合理**：reqwest / keyring / wiremock / proptest / futures / bytes 使用面都足够大；SSE、JSONL、Partial JSON 按基线「参考 + 自实现」落地且各带 no-panic 属性测试，方向正确。**没有「引用面小、应自实现替换」的包**。
4. **主要问题在基线管理**：`futures`、`bytes` 引入未登记；`backon`、`arbitrary` 声明未引用。生产重试实际是 agent-engine 自实现的 `RetryPolicy`，provider-runtime 的 `ExponentialBackoff` 是带两个 bug 的死代码，与基线声明的 backon 三方并存、无一生效（V8）。
5. **流程偏差**：11 篇 `plan/P2-*.md` 全部停留 🟡未开始、19 个验收勾选全部未勾，提交 `a8cd17d` 未触碰任何 plan/ 与 docs/ 文件，违反 AGENTS.md §4「任何任务完成后，对应模块文档与 ROADMAP 状态须同步更新」（ROADMAP 本身已更新）。
6. **契约套件对 ADR-015 与 P2-11 自身验收均有缺口**：timeout、reconnect 用例缺失（P2-11 验收原文含 timeout，[plan/P2-11-contract-tests.md:22](../../plan/P2-11-contract-tests.md)）；`assert_error_kind` 对空事件流 vacuous 通过；名为 cancel-mid-stream 的测试实际是预取消。

### 2. P2 任务完成情况核对表

| 任务 | 交付 crate | 状态 | 关键证据 |
| --- | --- | --- | --- |
| P2-1 HTTP 运行时 | `provider-runtime` | 🟢（有 V1/V2） | [http.rs](../../crates/provider-runtime/src/http.rs)：超时/代理/自定义 header/x-trace-id/取消竞争；默认 60s 见 [http.rs:34](../../crates/provider-runtime/src/http.rs) |
| P2-2 SSE 解析器 | `provider-runtime` | 🟢（fuzz 口径见 §3.2） | [sse.rs](../../crates/provider-runtime/src/sse.rs)：data/event/id/retry、BOM、跨 chunk UTF-8；proptest 随机字节不 panic（[sse.rs:304-306](../../crates/provider-runtime/src/sse.rs)） |
| P2-3 JSON Lines 解析器 | `provider-runtime` | 🟢 | [jsonl.rs](../../crates/provider-runtime/src/jsonl.rs)：提前断开、错误事件、proptest |
| P2-4 Partial JSON 拼接 | `provider-runtime` | 🟢（见 §6-15） | [partial_json.rs](../../crates/provider-runtime/src/partial_json.rs)：修复语义 + proptest |
| P2-5 OpenAI-compatible 适配 | `provider-openai-compatible` | 🟢（有 V3/V4/V5） | [provider.rs](../../crates/provider-openai-compatible/src/provider.rs)、[request.rs](../../crates/provider-openai-compatible/src/request.rs)、[stream.rs](../../crates/provider-openai-compatible/src/stream.rs) |
| P2-6 API Key 认证 | `auth-service` | 🟢 | Keychain/Memory 双后端（[backend.rs:17](../../crates/auth-service/src/backend.rs)、[backend.rs:81](../../crates/auth-service/src/backend.rs)）；明文不入 StoredCredential 有测试（[credential.rs:269-273](../../crates/auth-service/src/credential.rs)） |
| P2-7 Model Registry | `model-registry` | 🟢（见 V9、§6-12） | [registry.rs](../../crates/model-registry/src/registry.rs)：目录/别名/能力/定价 |
| P2-8 流式组装 | `provider-runtime` | 🟢 | [stream_assembly.rs](../../crates/provider-runtime/src/stream_assembly.rs)：事件→领域消息 |
| P2-9 Usage 与 stop reason | `provider-runtime` | 🟢（有 V3/V5） | [usage.rs](../../crates/provider-runtime/src/usage.rs)：多 Provider 字段归一、整数 micro 计价 |
| P2-10 重试与错误归一化 | `provider-runtime`（+ Phase 3 `agent-engine`） | 🟢（有 V8） | [retry.rs:14](../../crates/provider-runtime/src/retry.rs) classify_status / parse_retry_after；生产退避在 [agent-engine/src/retry.rs:109-124](../../crates/agent-engine/src/retry.rs)，正确尊重 retry_after（[agent-engine/src/retry.rs:114-116](../../crates/agent-engine/src/retry.rs)） |
| P2-11 Provider Contract Tests | `test-support` + `provider-openai-compatible` | 🟡部分（见 §7.1） | [test-support/src/contract.rs](../../crates/test-support/src/contract.rs) 断言库 + [tests/contract.rs](../../crates/provider-openai-compatible/tests/contract.rs) 10 用例 |

**门禁证据（2026-08-08 复核）**：

- `cargo test -p provider-runtime -p provider-openai-compatible -p auth-service -p model-registry -p test-support`：**120 passed / 0 failed**（provider-runtime 54；provider-openai-compatible 12 单元 + 10 契约；auth-service 27，含 Phase 6 追加的 oauth 模块；model-registry 10；test-support 7）。
- `cargo clippy --workspace --all-targets -- -D warnings`：干净。
- `cargo fmt --all -- --check`：干净。
- `cargo run -p schema-typegen -- --check`：TypeScript declarations up to date。
- 各任务 plan 文档（`plan/P2-*.md`）状态与验收勾选**均未同步**（§4、§7.2）。

### 3. 包选型评估

#### 3.1 建议保留（自实现不值得）

| 包 | 版本（Cargo.lock） | 使用点 | 使用面评估 | 结论 |
| --- | --- | --- | --- | --- |
| `reqwest`（rustls+stream+json） | 0.12.28 | P2-1；P9-2 将复用 | 唯一 HTTP 客户端，全部 Provider 流量经此；基线已论证（[ROADMAP.md:81](../../ROADMAP.md)） | **保留**；用法需修（V1：改 `read_timeout`） |
| `keyring` | 3.6.3 | P2-6 | OS Keychain 唯一入口，Secret 不落库红线的承载者（[backend.rs:26-40](../../crates/auth-service/src/backend.rs)） | **保留** |
| `futures` | 0.3 | P2-1/P2-5 | `Stream`/`StreamExt` 是字节流消费的核心抽象（[http.rs:10](../../crates/provider-runtime/src/http.rs)、[provider.rs:145](../../crates/provider-openai-compatible/src/provider.rs)） | **保留**；需回填基线（§4） |
| `bytes` | 1 | P2-1/P2-5 | 流式字节载体 `Bytes`（[http.rs:9](../../crates/provider-runtime/src/http.rs)） | **保留**；需回填基线（§4） |
| `wiremock` | 0.6.5 | P2-11；Phase 6 全部 provider | 契约套件 HTTP mock 基座，6 个 crate dev 依赖 | **保留**；注意 mock 遮蔽真实行为的风险（V3/V4） |
| `proptest` | 1 | P2-2/P2-3/P2-4 | 三个解析器的 no-panic 属性测试（[sse.rs:304](../../crates/provider-runtime/src/sse.rs)、[jsonl.rs:126](../../crates/provider-runtime/src/jsonl.rs)、[partial_json.rs:526](../../crates/provider-runtime/src/partial_json.rs)） | **保留** |

#### 3.2 需要重新评估的项

| 项 | 现状 | 选项 | 建议 |
| --- | --- | --- | --- |
| `backon` | 基线声明用于 P2-10（[ROADMAP.md:98](../../ROADMAP.md)）；workspace 与 provider-runtime 均声明（[Cargo.toml:129](../../Cargo.toml)、[provider-runtime/Cargo.toml:21](../../crates/provider-runtime/Cargo.toml)）但**全仓库零引用**（唯一命中是注释 [agent-engine/src/retry.rs:13](../../crates/agent-engine/src/retry.rs)）。生产重试 = agent-engine 自实现 `RetryPolicy`；provider-runtime 的 `ExponentialBackoff` 是死代码（V8） | a) 移出基线并删依赖，承认「退避自实现」；b) 用 backon 替换自实现退避 | **倾向 a**：agent-engine 的退避已满足需求且尊重 Retry-After，继续引入 backon 收益有限；同时删除 `ExponentialBackoff` 死代码。若未来需要更复杂策略（按错误类别差异化退避）再评估 b |
| `cargo-fuzz` + `arbitrary` | 基线测试工具行（[ROADMAP.md:100](../../ROADMAP.md)）；`arbitrary` 已声明（[Cargo.toml:135](../../Cargo.toml)）但无 crate 引用，仓库无 `fuzz/` 目录。P2-2/P2-3 验收「fuzz 不 panic」（[plan/P2-2-sse-parser.md:23](../../plan/P2-2-sse-parser.md)）实际由 proptest 承担 | a) 建 `fuzz/` 目标（SSE/JSONL/partial-json 三个解析器是理想靶子）；b) 修订基线与 plan，明确「属性测试代替 cargo-fuzz」 | **建议 a**：解析器是外部输入第一入口，libFuzzer 级覆盖值得；至少应先把 `arbitrary` 的声明处置掉 |
| `reqwest` 超时语义 | 0.12.28 已提供 `read_timeout`（按读操作重置），当前只用了总 `timeout`（V1） | 无需换包，改用 API | 见 V1 |

#### 3.3 「自实现替换包」总体判断

针对「引用面小 → 自实现换取可控性」的命题：**P2 范围内没有命中的包**。真正需要收敛的是反向问题——**自实现与已声明的包并存且自实现是死代码**：退避策略一处有 backon（声明未用）、一处有 agent-engine `RetryPolicy`（生产中用）、一处有 provider-runtime `ExponentialBackoff`（死代码且带 bug）。三方并存是最差状态，按 §3.2 收敛为一处。基线「参考 + 自实现」的三个解析器（SSE/JSONL/Partial JSON）实现质量良好，不需要回退为引包。

### 4. 基线偏差清单

规则来源：ROADMAP「依赖选型基线」要求新增依赖同步回填基线表（[ROADMAP.md:14](../../ROADMAP.md)、[ROADMAP.md:58](../../ROADMAP.md)）。

| 类型 | 项 | 位置 | 说明 |
| --- | --- | --- | --- |
| 引入未登记 | `futures = "0.3"` | [Cargo.toml:68](../../Cargo.toml) | `a8cd17d` 引入；ROADMAP 基线表无此行。Cargo.toml 注释自称「依赖选型基线（ROADMAP『依赖选型基线·直接采用』）」，镜像关系已失真 |
| 引入未登记 | `bytes = "1"` | [Cargo.toml:69](../../Cargo.toml) | 同上 |
| 声明未引用 | `backon = "1"` | [Cargo.toml:129](../../Cargo.toml)、[provider-runtime/Cargo.toml:21](../../crates/provider-runtime/Cargo.toml) | 见 §3.2；零代码引用 |
| 声明未引用 | `arbitrary = "1"` | [Cargo.toml:135](../../Cargo.toml) | 无 `fuzz/` 目录、无 crate 引用；见 §3.2 |
| 流程偏差 | `plan/P2-*.md` 全部未同步 | 11 篇均 `🟡未开始`，19 个验收框未勾 | 提交 `a8cd17d` 只改 Cargo/ROADMAP/源码，未触碰 plan/ 与 docs/，违反 AGENTS.md §4。ROADMAP 状态列本身已更新为 🟢，属「半同步」 |

**建议**：一次小型清理任务统一处理——回填 futures/bytes 两行、删除 backon/arbitrary 两处声明（或说明豁免理由）、同步 11 篇 plan 文档。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V10）。

#### V1 [正确性·高] reqwest 总超时 60s 覆盖流式全程，长生成必被掐断

[http.rs:34](../../crates/provider-runtime/src/http.rs) 默认 `timeout = 60s`，经 [http.rs:101-102](../../crates/provider-runtime/src/http.rs) 设为 reqwest 的 `timeout()`。reqwest 文档明确该超时是「from when the request starts connecting until the response body has finished…a total deadline」——对流式响应，**整个 body 读取都计入**。任何总时长超过 60s 的 LLM 生成（长 reasoning、慢本地模型）都会在中途被超时打断并归一为错误。reqwest 0.12.28 已提供 `read_timeout`（按每次读操作重置，专为「未知大小的长流」设计）。**传播面**：provider-openai-compatible 与 Phase 6 的 provider-openai/anthropic/google 全部经此客户端（provider-openai 的 `request_timeout` 覆盖的也是同一总超时语义，[provider-openai/src/provider.rs:72-76](../../crates/provider-openai/src/provider.rs)）。**建议**：流式路径改用 `read_timeout`（如 60s 无新字节才判定停滞）并取消/大幅放宽总超时；补「慢速长流不被掐断」的契约用例（与 §7.1 的 timeout 用例合并）。

#### V2 [正确性·高] select! 取消分支守卫使预取消失效，请求照发

[http.rs:173-175](../../crates/provider-runtime/src/http.rs) 与 [http.rs:216-218](../../crates/provider-runtime/src/http.rs) 的取消分支写作 `_ = cancel.cancelled(), if !cancel.is_cancelled() => …`。agent-domain 的 `CancellationFuture` 在 token 已取消时首次 poll 即 Ready（[cancel.rs:65-68](../../crates/agent-domain/src/cancel.rs)），**不加守卫时预取消本可被正确处理**；加了守卫后，预取消的 token 反而使取消分支丧失候选资格，select 只等 `send_fut`——请求照样发出、照样等待响应。契约测试 `contract_cancel_mid_stream`（[tests/contract.rs:185-199](../../crates/provider-openai-compatible/tests/contract.rs)）恰好是预取消：`cancel.cancel()` 在 `stream()` 之前调用，请求实际仍打到 mock，Cancelled 错误来自 [provider.rs:145-148](../../crates/provider-openai-compatible/src/provider.rs) 循环内的 `is_cancelled` 检查——测试通过但验证的完全不是 select 路径。**建议**：删除两处守卫；把该测试改为真正的 mid-stream 取消（读到一个 delta 后取消），并保留一个预取消用例断言「请求不应发出」（可用 wiremock 的命中计数验证）。

#### V3 [正确性·高] OpenAI 流式 usage 永远拿不到：未发送 `stream_options.include_usage`

[request.rs](../../crates/provider-openai-compatible/src/request.rs) 构造请求体时没有任何 `stream_options` / `include_usage` 字段（全仓库亦无命中）。OpenAI 及兼容 API 的流式模式**默认不返回 usage**，必须显式请求 `stream_options: {"include_usage": true}`。契约测试 `contract_usage_and_stop_reason`（[tests/contract.rs:164-181](../../crates/provider-openai-compatible/tests/contract.rs)）由 mock 主动推送 usage chunk，把该缺口完全遮蔽。后果：OpenAI 系（含 Phase 6 provider-openai 委托路径）真实流量下 usage 恒为 0，P2-9 的归一化没有输入源，下游费用估算与 Phase 14 额度监控全部失真。**建议**：请求体固定附加 `stream_options.include_usage = true`，正确处理尾部 usage-only chunk（`choices` 为空）；契约测试改为断言请求体包含该字段。

#### V4 [正确性·中] `list_models` 不携带认证头

[provider.rs:210-216](../../crates/provider-openai-compatible/src/provider.rs)：参数名为 `_credential`（未使用），`get_json` 不带任何 Authorization。OpenAI 官方与绝大多数云端兼容端点的 `/v1/models` 要求认证，此路径必然 401。契约测试 `contract_list_models`（[tests/contract.rs:320-337](../../crates/provider-openai-compatible/tests/contract.rs)）用无认证 mock + `None` 凭据，无法暴露。旁证：Phase 6 的 provider-openai 选择完全不调远端 `/models`、直接返回内置目录（[provider-openai/src/provider.rs:97-102](../../crates/provider-openai/src/provider.rs)）来绕开它。**建议**：复用 `auth_header()`（[provider.rs:87-93](../../crates/provider-openai-compatible/src/provider.rs)）给 `list_models`；契约测试增加「请求头含 Authorization」断言。

#### V5 [正确性·中] `[DONE]` 无 finish_reason 时 stop_reason 被记为 Error

[provider.rs:137](../../crates/provider-openai-compatible/src/provider.rs) 将 summary 初始 `stop_reason` 置为 `StopReason::Error`；`[DONE]` 分支（[provider.rs:152-155](../../crates/provider-openai-compatible/src/provider.rs)）只置 `saw_completion`、不更新 stop_reason。部分本地服务（Ollama/vLLM 的某些版本）最后一个 chunk `finish_reason` 为 null 或缺失、直接以 `[DONE]` 收尾——此时流成功完成，但 summary 记为 Error，误导 P3-7 重试判定与 GUI 展示。同源问题：`map_stop_reason(None, false) → StopReason::Error`（[usage.rs:51](../../crates/provider-runtime/src/usage.rs)）。**建议**：流正常走到 `[DONE]` 而从未见到 finish_reason 时归一为 `Completed`（或 `Other("done")`）；`map_stop_reason(None)` 的语义在 docs/features/providers.md 中写明。

#### V6 [安全·中] `provider_options` 透传无键保护，可覆盖 canonical 关键字段

[request.rs:89-93](../../crates/provider-openai-compatible/src/request.rs)：provider_options 以「覆盖」语义合并进请求体顶层，无任何保留键限制——调用方可覆盖 `model`、`messages`、`stream`、`tools`。P2 阶段入口只有测试，但 Phase 6（P6-9）已把该透传作为正式能力，后续 GUI/配置一旦直通，即可绕过 canonical 层约束（例如把 `stream` 改为 false 破坏整个流式管线）。**建议**：定义保留键集合（model/messages/stream/tools 及认证相关字段），透传命中保留键时忽略并告警；或在 provider_options 入口做 schema 白名单，并在 [docs/features/providers.md](../features/providers.md) 记录边界。

#### V7 [健壮性·中] 解析器缓冲无上限，且非法字节逐个移除是 O(n²)

SSE 与 JSONL 解析器的内部 `buf` 只增不减、无容量上限（[sse.rs:61](../../crates/provider-runtime/src/sse.rs)、[jsonl.rs:20](../../crates/provider-runtime/src/jsonl.rs)）：一条永不出现行终止符的流（恶意或故障端点）会让内存无限增长。非法 UTF-8 的处理用 `Vec::remove` 逐字节移除（[sse.rs:137](../../crates/provider-runtime/src/sse.rs)、[jsonl.rs:84](../../crates/provider-runtime/src/jsonl.rs)），每次 remove 是 O(n) memmove，持续坏字节流下退化为 O(n²)。P2-2/P2-3 验收只要求「不 panic」，性能健壮性缺了一半。**建议**：buf 设上限（如 1 MiB，超限发解析错误事件并重置）；非法字节改用游标分段 `drain` 批量移除。

#### V8 [质量·中] `ExponentialBackoff` 死代码且带两个 bug，与 P2-10 验收字面矛盾

[retry.rs:159-208](../../crates/provider-runtime/src/retry.rs) 的 `ExponentialBackoff` 仅被自身测试引用（[retry.rs:254](../../crates/provider-runtime/src/retry.rs)、[retry.rs:266](../../crates/provider-runtime/src/retry.rs)），生产重试走 agent-engine 的 `RetryPolicy`（正确尊重 Retry-After，[agent-engine/src/retry.rs:114-116](../../crates/agent-engine/src/retry.rs)）。死代码本身有两个 bug：① [retry.rs:206](../../crates/provider-runtime/src/retry.rs) 结尾 `Some(delay.min(self.cap))` 把 Retry-After 也钳进 cap，直接违反其文档注释（[retry.rs:157](../../crates/provider-runtime/src/retry.rs)「遵守 Retry-After」）与 P2-10 验收项「退避遵守 Retry-After」（[plan/P2-10-retry-error.md:23](../../plan/P2-10-retry-error.md)）；② jitter 用固定种子 LCG（[retry.rs:173](../../crates/provider-runtime/src/retry.rs)），所有实例共享同一序列（削弱雷群缓解），且 `(rng_state >> 33) / u32::MAX`（[retry.rs:190](../../crates/provider-runtime/src/retry.rs)）的采样值域只有 [0, ≈0.5]，抖动区间减半。**建议**：随 §3.2 一并处置——删除该结构（首选）或修复后接线生产。

#### V9 [正确性·低] `resolve()` 精确匹配与「不区分大小写」注释矛盾

[registry.rs:101-109](../../crates/model-registry/src/registry.rs)：两次 HashMap 精确查找，无任何大小写归一；注释（[registry.rs:107](../../crates/model-registry/src/registry.rs)）却声称「不区分大小写」。`resolve("GPT-4o")` 返回 None，用户以不同大小写书写模型 id 时静默落入「目录外模型」路径（能力/定价/上下文校验全部失效）。**建议**：入口 `to_ascii_lowercase` 归一（别名表构建时同样归一），或修正注释并在校验层给出明确错误。

#### V10 [质量·低] 契约测试遗留调试输出

[tests/contract.rs:205](../../crates/provider-openai-compatible/tests/contract.rs)：`println!("XXXURI_START{}XXXURI_END", uri);` 调试残留，随 `cargo test` 输出。**建议**：删除（顺手项）。

### 6. 优化建议（按优先级）

#### P0（建议在 Provider 面向真实用户接线前处理）

1. **V3**：请求体附加 `stream_options.include_usage` 并处理 usage-only 尾块——usage 是费用与 Phase 14 额度的数据源，当前恒为 0。
2. **V1**：流式路径改 `read_timeout`；与 timeout 契约用例（§7.1）一起落地。
3. **V2 + V4**：删 select 守卫、`list_models` 加认证头；两处都是小改动，可一提交完成，并各自补针对性断言。

#### P1（近期排期）

4. **V5**：`[DONE]` 无 finish_reason 归一为 Completed；同步修订 `map_stop_reason(None)` 语义说明。
5. **退避收敛**：删 `ExponentialBackoff`（V8）+ 移除 backon 声明（§3.2）+ 回填 futures/bytes（§4），一次基线清理提交。
6. **V7**：解析器有界缓冲 + 批量移除非法字节。
7. **契约套件补齐**：新增 timeout、reconnect 用例（P2-11 验收原文要求，[plan/P2-11-contract-tests.md:22](../../plan/P2-11-contract-tests.md)）；修复 `assert_error_kind` 的 vacuous 通过——当前 `found || events.is_empty()`（[test-support/src/contract.rs:93-96](../../crates/test-support/src/contract.rs)）让空事件流永远通过，应改为同时接收 `stream()` 的返回错误并强制断言其一。
8. **文档同步**：11 篇 `plan/P2-*.md` 状态与验收勾选回填（AGENTS.md §4）；删 [tests/contract.rs:205](../../crates/provider-openai-compatible/tests/contract.rs) 调试输出（V10）。
9. **V6**：provider_options 保留键保护（赶在更多入口接入前）。

#### P2（顺手/评估项）

10. [request.rs:49-50](../../crates/provider-openai-compatible/src/request.rs) 发送 `max_tokens`：OpenAI o 系列只接受 `max_completion_tokens`，建议按模型族切换或双发兼容。
11. [provider.rs:236-237](../../crates/provider-openai-compatible/src/provider.rs) 对发现的模型硬编码 128k/16k 能力：改为「未知 → 留空/可配置覆盖」，避免 `validate_context` 与真实窗口错位。
12. model-registry 内置目录陈旧（gpt-4o / gpt-4o-mini / claude-3-5-sonnet / gemini-1.5-pro / gpt-3.5-turbo，定价为硬编码近似值，[registry.rs:183-261](../../crates/model-registry/src/registry.rs)）：标注数据日期、建立目录更新任务；另注意 Phase 6 provider-openai 自带一份内置目录（[provider-openai/src/provider.rs:114](../../crates/provider-openai/src/provider.rs) 起），双目录并存有漂移风险。
13. 计价逻辑双份：[pricing.rs:83-105](../../crates/model-registry/src/pricing.rs) 与 [usage.rs:76-97](../../crates/provider-runtime/src/usage.rs)（`ModelPricingRef` 注释自称「与 model-registry 的 ModelPricing 字段对齐，避免循环依赖」）。字段对齐靠注释维持，建议把计价纯函数收敛到单一 crate 导出，另一侧复用。
14. `response_id` 用 trace_id 顶替（[provider.rs:130-131](../../crates/provider-openai-compatible/src/provider.rs)、[provider.rs:199](../../crates/provider-openai-compatible/src/provider.rs)）：应取响应体 `id`，trace 关联与 provider 侧 id 目前混为一个值。
15. partial_json 两处毛刺：`parse_repaired` 对已完整 JSON 双重解析（[partial_json.rs:46-51](../../crates/provider-runtime/src/partial_json.rs) 先 `from_str`，`repair_json` 内 [partial_json.rs:372](../../crates/provider-runtime/src/partial_json.rs) 再查一遍）；`scan_number`（[partial_json.rs:382-404](../../crates/provider-runtime/src/partial_json.rs)）把 `1.2.3` 这类畸形数字按「EOF 截断」原样保留，最终 `parse_repaired` 返回 None，组装侧回退 `Value::Null` 丢失整个 arguments（[stream_assembly.rs:183-185](../../crates/provider-runtime/src/stream_assembly.rs)）。实际流中罕见，记录备查。
16. auth-service 的 `MemoryBackend`（[backend.rs:81](../../crates/auth-service/src/backend.rs)）中 secret 不做 zeroize：当前是测试/回退后端，若升级为生产可用，需评估 `Zeroizing` 包装；在 [docs/features/auth.md](../features/auth.md) 记录该残余风险。
17. SSE `finish()` 在流尾残留非法 UTF-8 时静默丢弃（[sse.rs:92-104](../../crates/provider-runtime/src/sse.rs) 注释「尽力而为」）：可接受，但建议在诊断日志中计数，便于排查「最后一个事件消失」类问题。

### 7. 附录

#### 7.1 ADR-015 契约用例覆盖对照

[ADR-015:12](../adr/ADR-015-provider-contract-tests.md) 要求 14 类用例：text、tool call、multiple tool calls、image、thinking、usage、stop reason、cancel、timeout、rate limit、malformed stream、partial JSON、reconnect、context overflow。

| 用例 | provider-openai-compatible（P2-11） | 说明 |
| --- | --- | --- |
| text / tool call / multiple tool calls | ✅ 3 用例 | [tests/contract.rs:95-163](../../crates/provider-openai-compatible/tests/contract.rs) |
| usage / stop reason | ✅（但被 mock 遮蔽，见 V3） | [tests/contract.rs:164](../../crates/provider-openai-compatible/tests/contract.rs) |
| cancel | ⚠️ 名为 mid-stream 实为预取消（V2） | [tests/contract.rs:185](../../crates/provider-openai-compatible/tests/contract.rs) |
| rate limit / context overflow / malformed / partial JSON | ✅ 4 用例 | [tests/contract.rs:202-318](../../crates/provider-openai-compatible/tests/contract.rs) |
| **timeout** | ❌ 缺失 | P2-11 验收原文包含（[plan/P2-11-contract-tests.md:22](../../plan/P2-11-contract-tests.md)）；Phase 6 三个 provider 套件同样没有 |
| **reconnect** | ❌ 缺失 | ADR-015 与 P2-11 步骤 1 均列出；全仓库无对应用例 |
| image / thinking | ➖ 不在 P2 范围 | 已由 Phase 6 各自套件覆盖（provider-openai/anthropic/google 的 tests/contract.rs） |
| list_models（超出 ADR 清单） | ✅ | [tests/contract.rs:320](../../crates/provider-openai-compatible/tests/contract.rs) |

另：`tests/contract.rs` 自 `a8cd17d` 后无任何改动（git log 确认），Phase 6 未回补 P2 套件。

#### 7.2 plan 文档漂移清单

| 文件 | 状态字段 | 未勾验收框 |
| --- | --- | --- |
| plan/P2-1-http-runtime.md ~ plan/P2-11-contract-tests.md（共 11 篇） | 全部 `🟡未开始`（应为 🟢） | 合计 19 个 `- [ ]`，如 [plan/P2-10-retry-error.md:22-23](../../plan/P2-10-retry-error.md)、[plan/P2-11-contract-tests.md:22](../../plan/P2-11-contract-tests.md) |

对照 REVIEW.md §2 的做法（Phase 1 的 plan 文档均已勾选），Phase 2 是唯一「ROADMAP 已 🟢、plan 全未动」的阶段；提交 `a8cd17d` 的文件清单（31 个文件）确认未触碰 plan/ 与 docs/。

### 8. 建议的后续动作（本次未执行，供研究）

1. 对 V1~V4 立项（真实端点可用性红线；V3 是 Phase 14 额度监控的前置）。
2. 基线清理小任务（§4 + §3.2）：回填 futures/bytes、移除 backon/arbitrary、删 ExponentialBackoff，一次提交。
3. 契约套件补齐：timeout/reconnect 用例 + `assert_error_kind` 语义修复 + cancel 用例改造（§6-7）。
4. plan/docs 同步任务：11 篇 plan 文档回填状态与勾选，providers.md 补 `include_usage` / stop reason 语义说明。
5. 目录与计价治理（§6-12/13）：模型目录更新机制、计价单一来源，可与 Phase 14 一并评估。

---

*评审方法：以 `67d6c4d` 为基线，逐项核对 ROADMAP/plan 状态、源码与依赖清单，并复跑 Phase 2 相关 5 个 crate 的测试与静态门禁（test/clippy/fmt/schema-typegen）；对 reqwest 超时语义、CancellationFuture 行为等关键断言直接核对了依赖源码与 agent-domain 实现；文中所有结论均给出文件与行号级证据。本文档仅为评审记录，不代表已批准的变更。*

---

## 修复记录（review-remediation）

> Phase 2 · 首个真实 Provider · 状态：🟢已完成 · 交付成熟度：Validated · 依赖：P2-1 ~ P2-11

**最终目的**：消除 [REVIEW.md](../../REVIEW.md) §2（Phase 2）发现的「mock 过得去、真实端点翻车」型正确性高危与基线/契约/文档漂移——让流式 usage 真实可得（Phase 14 额度的数据源）、长流不被 60s 超时掐断、取消语义与认证头正确，并收敛退避三方并存的死代码与 plan 文档未同步的流程偏差。

**涉及范围**：`provider-runtime`、`provider-openai-compatible`、`model-registry`、`test-support`、根 `Cargo.toml`、ROADMAP「依赖选型基线」、`docs/features/providers.md`、`plan/P2-*.md`

### 细分步骤（分组）

#### A. 正确性高危（V1 / V2 / V3 / V4）

1. **V1 流式超时**：`provider-runtime/src/http.rs` 流式路径改用 reqwest `read_timeout`（按读操作重置），取消/大幅放宽覆盖全程的总 `timeout`。目的：长生成与慢本地模型不被中途掐断。
2. **V2 取消守卫**：删除 `http.rs` 两处 `if !cancel.is_cancelled()` 守卫，恢复预取消语义；将 `contract_cancel_mid_stream` 改为读到 delta 后真取消，并保留预取消用例断言「请求不应发出」（用 wiremock 命中计数）。目的：预取消不再误发请求。
3. **V3 include_usage**：`provider-openai-compatible` 请求体固定附加 `stream_options.include_usage = true`，正确处理尾部 usage-only chunk（`choices` 为空）。目的：真实 API 下 usage 不再恒为 0。
4. **V4 list_models 认证**：`list_models` 复用 `auth_header()` 携带 Authorization。目的：受保护 `/models` 不再 401。

#### B. 正确性/质量中低（V5 ~ V10）

5. **V5 stop reason 归一**：`[DONE]` 而未见 finish_reason 时归一为 `Completed`（或 `Other("done")`），同步修 `map_stop_reason(None)` 语义。目的：本地服务收尾不被误记 Error，不误导重试判定。
6. **V6 provider_options 保留键**：定义保留键集合（model/messages/stream/tools 及认证字段），透传命中时忽略并告警。目的：防止覆盖 canonical 关键字段。
7. **V7 解析器有界缓冲**：SSE/JSONL `buf` 设上限（1 MiB，超限发解析错误并重置），非法字节改游标 `drain` 批量移除。目的：消除无限内存与 O(n²) 退化。
8. **V8 退避死代码**：删除 `provider-runtime/src/retry.rs` 的 `ExponentialBackoff`（带「cap 钳进 Retry-After」与「jitter 固定种子采样减半」两个 bug 的死代码）。目的：退避收敛为 agent-engine 单一来源。
9. **V9 resolve 大小写**：`model-registry` `resolve()` 入口 `to_ascii_lowercase` 归一（别名表构建同步归一）。目的：消除「不区分大小写」注释与精确匹配实现的矛盾。
10. **V10 调试输出**：删除 `tests/contract.rs` 遗留的 `println!("XXXURI...")`。目的：测试输出干净。

#### C. 基线与包清理

11. **补登/移除**：在 ROADMAP「依赖选型基线」补登 `futures`、`bytes`（根 `Cargo.toml` 已声明）；移除 `backon`、`arbitrary`（声明未引用，`fuzz/` 缺位）；随 V8 删除 `provider-runtime` 的 `backon` 依赖行。目的：基线一致、无死声明。

#### D. 契约与文档漂移

12. **契约套件补齐**：新增 timeout、reconnect 用例（P2-11 验收原文要求，[ADR-015](../../docs/adr/ADR-015-provider-contract-tests.md)）；修复 `assert_error_kind` 空事件流 vacuous 通过（改为同时接收并强制断言其一）；cancel 用例按 V2 改造。目的：兑现 ADR-015 与 P2-11 自身验收。
13. **plan/docs 同步**：11 篇前置 `plan/P2-*.md` 状态回填 🟢、现有 22 个验收框全部勾选；`providers.md` 补 `include_usage` 与 stop reason 语义说明。目的：纠正违反 AGENTS.md §4 的流程偏差。

### 主要产出物

- http.rs read_timeout + 取消守卫修复；include_usage + list_models 认证；stop reason 归一、provider_options 保留键、解析器有界缓冲、ExponentialBackoff 删除
- ROADMAP 基线补登/依赖移除（futures/bytes/backon/arbitrary）；timeout/reconnect 契约用例 + assert_error_kind 修复
- 11 篇 plan 回填 + providers.md 语义说明

### 验收标准（保留 REVIEW 追踪编号）

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

### 验证记录

- 2026-08-09：受影响的 Provider / runtime / registry / contract helper / auth crate 定向测试通过。
- 2026-08-09：上述 crate 的 `cargo clippy --all-targets -- -D warnings` 与定向 `cargo fmt -- --check` 通过。
- 本任务未修改 schema，未触发生成物检查；Core 主干 L2 仍按 Phase 1～7 remediation 的统一收尾节奏执行。

**相关文档**：[REVIEW.md](../../REVIEW.md) §2 · [ADR-015 Provider 契约测试](../../docs/adr/ADR-015-provider-contract-tests.md) · [providers](../../docs/features/providers.md) · [ROADMAP 依赖选型基线](../../ROADMAP.md#依赖选型基线)
