# Phase 9 Review：MCP（mcp-client）

> 审查范围：`crates/mcp-client`（P9-1～P9-7）及其与 `agent-engine` / `tool-runtime` / `policy-engine` / `config-service` / `auth-service` / `resource-loader` 的接线。
> 方法：Commander 统筹 + 3 个 `deepseek_explorer` 并行调查（内部结构与冗余 / 端到端接线 / canonical 抽象一致性），结论由 Commander 复核合并并独立验证关键事实。
> 性质：**只 Review，不改实现。**

---

## 0. 一句话结论

`mcp-client` 的 **crate 内部实现是扎实、确定、有测试的**（rmcp 隔离、stdio/HTTP 双 transport、健康/重连/取消/脱敏、OAuth 复用 auth-service），架构方向正确——它与 P8 `resource-loader` 同属「叶子 crate，等待接线」家族。但它和 P8 一样**没有任何端到端消费者**：整个 workspace 没有第二个 crate 依赖它，`McpConfig::from_resolved` / `ManagedMcpClient::new` / `register_discovered_tools` / `pawork mcp doctor` 的运行时调用全部为零。这不是 P9 的缺陷——工具链路（builtin-tools、agent-engine、tool-runtime 本身）整体尚未被 `apps/pawork` 装配（属 P13/P15+），P9 只是这条装配链上尚未通电的一节。

真正值得在本阶段记录的是 **crate 内部的冗余与提前抽象**：在零消费者的前提下，它已经长出了两份 `RestartPolicy`、一份单变体 `SecretValue` enum、一套与 config-service 语义冲突且只服务测试的 `merge`、一份与 tool-runtime `ApprovalResolver` 并存的 `McpApproval` 审批通道，以及在 adapter 内重写了一遍调度器已做等价裁决的完整门禁管线。这些在接入时（P15 Canonical Tool v2 / P19 GUI Resources·MCP）会成为真实的不一致来源。

核心建议方向：**减少**——合并重复类型、删死抽象、把 per-server 策略上提到 canonical 调度器，而不是在接入前继续往 adapter 里堆门禁。

---

## 1. 设计符合度

| 子任务 | plan 目标 | 实现位置 | 符合度 | 备注 |
|---|---|---|---|---|
| P9-1 stdio Transport | 启动/通信/握手/错误 | `transport.rs::connect_stdio` 驱动 rmcp `TokioChildProcess` | ✅ 符合 | 合理薄封装 |
| P9-2 Streamable HTTP | 远程接入 + timeout/restart | `transport.rs::connect_http` + `manager.rs` 重连/退避 | ✅ 符合 | 拒绝旧 HTTP+SSE，HTTPS/Secret 边界有测试 |
| P9-3 Tools/Resources/Prompts | 发现 + 注册 | `capabilities.rs::discover` + `register_server_tools` | ⚠️ 符合但有死 API | 见 §3.1：Resources/Prompts 半套，从未被任何适配器调用 |
| P9-4 Health/restart/cancel/logging | 故障隔离 | `manager.rs` 全套生命周期 | ✅ 符合（最大亮点） | 真实 rmcp in-process server 跑在 tokio duplex 上，覆盖超时/取消/握手取消/有界重连/健康快照响应性；不是过度 mock |
| P9-5 Approval/输出限制/Secret | 每 server 独立权限 | `capabilities.rs::McpInvocationPolicy` + `McpApproval` + `config.rs::McpPermissions` + `security.rs` | ⚠️ 符合但重复门禁 | 见 §4.1：与 `tool-runtime::ApprovalResolver` + `PolicyEngine` 双轨 |
| P9-6 MCP Config | workspace/global | `config.rs::McpConfig::from_resolved` 读 `ResolvedConfig.extra["mcp"]` | ⚠️ 符合但 plan/实现不符 | plan 声称涉及 resource-loader，实际 resource-loader 零 mcp 代码（`profiles.rs:16` 注释把 MCP 划给 P17-5 v2） |
| P9-7 OAuth（P1） | 复用 OAuth、保护型 server | `oauth.rs` 复用 auth-service PKCE/刷新/轮换 | ✅ 符合 | 边界清晰，未复制 Token 生命周期 |

**判定**：plan 目标在 crate 内全部达成，🟢 与实现一致，rmcp 隔离 + canonical tool 复用的红线**结构上合规**（mcp-client 是叶子 crate，Agent Engine 只经 `AgentTool`/`ToolDescriptor` 接触它，rmcp 类型不出 crate，`{server}.{tool}` 是配置层命名空间而非按 Server/Provider 名称分支）。但 P9-3/5/6 三项有「实现完整但含死 API 或重复门禁」的问题，P9-6 还存在 plan 与实现范围不符。

---

## 2. 零端到端消费者（背景，非缺陷）

与 P8 同型的结论，由 3 个独立调查路径一致确认，并经 Commander 复核：

- **crate 依赖链**：全 workspace 没有任何 crate 依赖 `mcp-client`（仅其自身 `Cargo.toml` 提及）。真实运行链是 `apps/pawork` → `cli-host` + `app-service` + `cli-command`，其中 `app-service` 的依赖仅 `core-api/directories/serde/serde_json`，不引入 `agent-engine`/`tool-runtime`/`builtin-tools`/`mcp-client`。
- **未接线的不止 MCP**：`agent-engine` 同样无下游依赖；`tool-runtime` 仅被 `agent-engine`/`mcp-client`/`wasm-plugin-host` 引用，而这三者自身都是孤立的；`builtin-tools` 也无人依赖。**整个工具链路（builtin 与 MCP）尚未被任何主流程调用点装配**——这是跨 Phase 的接线工作（P13 Host Run 编排 / P15 Canonical Tool v2 / P19 GUI Resources·MCP），不是 P9 的范围越界。
- **运行时调用为零**：`McpConfig::from_resolved` / `from_value` / `McpServerConfig::build_client` / `ManagedMcpClient::new`（生产路径）/ `register_discovered_tools` 的全部调用点都在 `mcp-client` 自身测试里。`pawork mcp doctor` 经 `cli-host` 映射为 `ServiceOperation::Placeholder`，是 stub。
- `CommandSource::Mcp` 在 `core-api`/`app-service` 中仅作为遥测标签（`source_name` → `"mcp"`），不承载任何 MCP 逻辑。

**含义**（与 P8 一致）：

1. P9 的全部行为正确性目前**只能由单元测试背书**（48 个测试，capabilities 14 / manager 10 / config 8 / transport 8 / oauth 4 / security 4，无 `tests/` 集成目录），无法由集成路径背书。
2. 在接线前引入的抽象承担的是「为 P15/P19 预留契约」的角色；Review 重点应放在「这些预留是否最小、是否会在接入时制造不一致」，而非「是否已生效」。

---

## 3. 冗余与过度设计（按可削减量排序）

> 行数以源文件为准（不含测试）。本节为 REVIEW，不执行修改。

### 3.1 Resources / Prompts 半套死 API（最该在接入前决策）

`McpCapabilities::discover` 列出 4 类能力（tools/resources/resource_templates/prompts），`McpPeer` trait 对应 7 个方法，但适配器只消费 tools——`register_server_tools` 明确「only tools are adapted」（`capabilities.rs:349`），`read_resource` / `get_prompt` / `list_resources` / `list_resource_templates` / `list_prompts` 没有任何适配器调用。这导致两难：要么接入时这些方法被真正使用（OK，保留），要么始终不接（则它们是带完整签名却无消费方的死 API）。当前应至少标注为 deferred-consumer，避免被误读为「Resources/Prompts 已可读取」。`McpServerCapabilities::default()` 全 true（`session.rs:26`）也只为测试 peer 便利而设。

### 3.2 双 `RestartPolicy` + 魔法换算（纯冗余层）

两份语义重叠的类型：

- 持久化版 `config.rs::RestartPolicy`（`enabled` / `max_restarts` / `delay_ms`）。
- 运行时版 `manager.rs::RestartPolicy`（`max_attempts` / `base_delay` / `max_delay`）。
- 靠 `McpServerConfig::runtime_options`（`config.rs:167`）桥接，内含魔法映射 `max_attempts = max_restarts + 1`、`max_delay = delay_ms * 16`。

两份类型 + 一个转换器应合并为一份可序列化 struct，去掉 `×16` 魔法数（约 -50 行机器 + 配套测试）。

### 3.3 单变体 `SecretValue` enum（为未来预留的空壳）

`config.rs:292` 的 `SecretValue` 只有一个变体 `SecretRef(SecretRef)`，注释自述 inline 明文「intentionally not representable」。既然唯一表示就是 `SecretRef`，env/headers 直接用 `BTreeMap<String, SecretRef>` 即可，省掉单变体 enum + 手写 `Debug` + `resolve` 转发（约 -30 行）。脱敏语义由 `SecretRef` 自身保证。

### 3.4 `McpInvocationPolicy` ≈ `McpPermissions`（字段级 1:1 镜像）

`capabilities.rs:85` 的 `McpInvocationPolicy` 与 `config.rs:394` 的 `McpPermissions` 字段一一对应，且都带 1 MiB 默认值（两处字面量：`capabilities.rs:99` vs `config.rs:26`），另配 `from_permissions` / `from_server_config` 两个转换器。直接消费 `McpPermissions` 即可，删掉一个类型 + 两个转换器（约 -45 行）。

### 3.5 URL / 传输校验双份 + `is_loopback_url` 重复

`TransportSpec::validate`（`config.rs:213`）与 `build_http_transport_config`（`transport.rs:277`）各自完整校验 scheme/userinfo/fragment/loopback+HTTPS；`is_loopback_url` 在两个文件各抄一份（`config.rs:451` 与 `transport.rs:403`）。校验收敛到 config 解析一处，`build_http_transport_config` 只保留非空/冲突检查，删掉重复副本（约 -60 行）。这是安全关键逻辑，收口到单点尤其重要。

### 3.6 `McpConfig::merge`（第二套合并语义，只服务测试）

`config.rs:97` 自实现 whole-server 覆盖合并，与 config-service 的递归 object 合并语义不同（config-service 在 JSON 层已完成 tier 合并，`from_resolved` 注释自述），且全仓库唯一调用点是 `config.rs:674` 的测试。应删除或标记 `#[cfg(test)]`。

### 3.7 输出截断逻辑寄居在 MCP adapter 里

`apply_output_cap`（`capabilities.rs:517`）与 `convert_call_tool_result`（`capabilities.rs:433`，约 150 行）是通用的「硬字节上限 + UTF-8 边界截断 + 二进制丢弃 + `truncated` 标记」逻辑，不应长在 MCP 适配器里。上移为 `tool-runtime`/`tool-api` 的共享输出截断助手后，capabilities.rs 减负约 150 行，且服务未来其他外部工具适配器。这是职责再分配而非纯删减。

### 3.8 文件碎片化：error.rs(30) + session.rs(51) + lib.rs(14)

`McpError` 被全部模块横切引用，`McpPeer` 同时被 manager（实现）与 capabilities（消费）使用——三者是同一处 API 面，却拆成 3 个文件 + re-export。并入 `lib.rs` 可去 2 个文件、不改逻辑。（命名上 `session.rs` 也容易误读为「会话持久化」，而它只是 peer trait 与能力标志。）

---

## 4. 架构问题

### 4.1 双重门禁：adapter 内重写调度器裁决（P0，接入前必须决策）

这是本次 Review 最重要的架构发现，也是与「canonical 唯一路径」红线张力最大的一处：

通用侧 `tool-runtime::ToolScheduler` 已对每个注册工具做完整 `PolicyEngine.decide` + `ApprovalResolver` 裁决（`scheduler.rs:112`、`:241`），agent-engine 所有工具调用都走 `ToolScheduler.execute_named`（`provider_loop.rs:827`/`:893`）。但 `McpToolAdapter::execute`（`capabilities.rs:247`）又在 adapter 内重写了一遍完整门禁管线——workspace allowlist（`:254`）、tool allowlist（`:267`）、`PolicyEngine.decide`（`:284`）、`McpApproval`（`:303`）。

后果：

1. MCP 工具一旦注册进 `ToolRegistry`，会被**两套策略**（全局模式 + per-server 模式）和**两个独立审批通道**（`ApprovalResolver` + `McpApproval`）裁决，结果取决于哪一套先否决、语义如何叠加，这是接入时真实的职责模糊来源。
2. `McpApproval` 是与 `ApprovalResolver` 并存的第二套审批抽象；宿主必须为 MCP 单独接线审批回调，无法复用统一的审批 UI/审计。
3. adapter 内嵌 `PolicyEngine::new(mode)`（`capabilities.rs:197`）但 `decide` 实际以 `PolicyInput.approval_mode` 为准（`engine.rs:20`），构造时的 `mode` 是死状态。

**建议**：接入时不要让 adapter 自裁，而是把 per-server 差异（approval_mode/trust/allowlist/workspace allowlist）提升为调度器的 per-tool 输入（扩展 `PolicyInput` 或 `ToolDescriptor`），由 canonical 调度器统一裁决。当前阶段（零消费者）是做这种对齐的最佳时机——没有外部调用方会因此受损。注意 `PolicyInput` 目前没有 allowlist 字段，mcp-client 之所以手写 allowlist 门禁，部分是被迫的；对齐时一并补上。

### 4.2 OAuth 双 bearer 解析（P3，可优化）

`should_reconnect_before_request`（`oauth.rs:169`）与 `authorized_config`/`connect`（`oauth.rs:141`）在同一轮请求前各解析一次 bearer。属可优化，非重复实现；OAuth 整体（PKCE/刷新/轮换/持久化）正确委托 auth-service，边界清晰。

### 4.3 stdio 进程绕过 Sandbox Runtime（P2，已记为后续 remediation）

`P9-1` plan 的「执行所有权约束」自述：当前经 rmcp `TokioChildProcess` 直接 spawn，未声明经 Sandbox Runtime。按「Core-owned 进程统一 Sandbox/Process Runtime 所有权」（Phase 11 网络策略边界），MCP stdio 子进程须经 `Sandbox Runtime → Process Runtime`，禁止以 `tokio::process::Command` 绕过。plan 已将其记为后续 remediation 且不修改本任务 🟢 状态——与 §2 的接线时机一致（接入时一并收口）。本 Review 仅确认该记录存在、范围合理。

### 4.4 `McpPeer` 公开签名泄露 rmcp 类型（P2）

`session.rs::McpPeer` trait 的方法签名直接使用 `rmcp::model::{Tool, Resource, Prompt, CallToolResult, ...}`。lib.rs 文档自述「rmcp 类型不出 crate」，但 `McpPeer` 是 `pub` trait，其签名上的 rmcp 类型对任何实现者可见。当前无外部实现者（只有 `ManagedMcpClient`），影响有限，但接入时若要让宿主自定义 peer，这会破坏「rmcp 隔离在 mcp-client 内」的承诺。可在接入前评估是否引入 thin canonical DTO。

---

## 5. 合并 / 拆分 / 删除建议

按优先级与风险给出（**本 Review 不执行任何修改**）：

### 建议删除（零风险，纯减负）

- 删 `McpConfig::merge` 整段（`config.rs:97`）——只服务测试，且与 config-service 合并语义冲突。
- 删 `transport.rs::is_loopback_url` 副本与 `build_http_transport_config` 里的重复校验——安全关键逻辑收口到 config 解析一处。
- 删单变体 `SecretValue` enum——env/headers 直接用 `BTreeMap<String, SecretRef>`。
- 删 `McpInvocationPolicy`——直接消费 `McpPermissions`。

### 建议简化（低风险，接入前做）

- 合并双 `RestartPolicy` 为一份可序列化 struct，去掉 `×16`/`+1` 魔法换算。
- URL/传输校验收敛到一处。
- `McpApproval` 退役，per-server 策略提升为调度器 per-tool 输入（与 §4.1 配套）。
- error.rs + session.rs 并入 lib.rs。
- `begin_pkce_login` / `complete_pkce_login` 是对 auth-service 的薄透传，可由宿主直调 auth-service；`McpBearerProvider` 与 `OAuthHttpConnector` 可评估合并为一个 connector。

### 建议职责再分配（与 P15 Canonical Tool v2 协同）

- `apply_output_cap` / `convert_call_tool_result` 上移为 tool-runtime 共享输出截断助手。

### 建议补强（防御性，仅测试/文档）

- 给 Resources/Prompts 半套 API（§3.1）加 deferred-consumer 标记，接入前明确是否实现 Resources/Prompts 读取，否则下线其 trait 方法。
- 在接线前为「adapter 门禁 vs 调度器门禁」做一次明确取舍决策（§4.1），写入对应 ADR/feature 文档。
- plan/实现范围不符项（P9-6 的 resource-loader 实际未参与）更新 plan 备注，避免被误读。

### 不建议改动（做对的部分）

- **rmcp 隔离边界**：`connect_stdio`/`connect_http` 是合理薄封装，`McpConnector` trait（含 `should_reconnect_before_request` 钩子）服务于可测试性与 OAuth 重连，价值真实，保留。
- **config 复用 config-service**：`from_resolved` 只做 `extra["mcp"]` 强类型投影 + 校验，递归合并复用 config-service，方向正确。
- **OAuth 复用 auth-service**：未复制 PKCE/刷新/轮换/持久化，只加传输层 bearer 注入与轮换触发，符合 ADR-011 / mcp.md。
- **manager.rs 故障隔离质量**：真实 rmcp in-process server + tokio duplex 的测试设计是 crate 最大亮点，覆盖超时/取消/握手取消/有界重连/健康快照响应性，不过度 mock。
- **脱敏实现**：transport 的 stderr 整体脱敏、URL userinfo/fragment 拒绝、含 Secret 非 loopback 须 HTTPS、Debug 脱敏——均有定向测试，安全不变量到位。
- **stderr 转发 + 密钥打码**是 MCP 特有的合理增量。

---

## 6. 改进优先级矩阵

| 优先级 | 项 | 收益 | 风险 | 时机 |
|---|---|---|---|---|
| P0 | adapter 门禁 vs 调度器门禁取舍决策（§4.1） | 消除双重裁决/双审批通道，接入即走 canonical 路径 | 设计决策，需与 P15 协同 | 接入前（P15 Canonical Tool v2） |
| P1 | 删 `McpConfig::merge` + `is_loopback_url` 副本 + `SecretValue` 单变体（§3.3/3.5/3.6） | 纯减负，安全逻辑收口 | 零 | 现在可做 |
| P1 | 合并双 `RestartPolicy` + `McpInvocationPolicy` 并入 `McpPermissions`（§3.2/3.4） | 去魔法换算与字段镜像 | 低 | 现在可做 |
| P2 | stdio 进程纳入 Sandbox/Process Runtime（§4.3） | 符合执行所有权红线 | 中（需跑 spawn/resize 门禁） | 接入时（已记为 remediation） |
| P2 | 评估 `McpPeer` 是否引入 canonical DTO（§4.4） | 真正兑现「rmcp 不出 crate」 | 低 | 接入前 |
| P2 | Resources/Prompts 半套 API 明确 deferred-consumer 或下线（§3.1） | 避免死 API 误导 | 零 | 接入前决策 |
| P3 | 输出截断逻辑上移 tool-runtime（§3.7） | 职责归位，服务未来适配器 | 低（与 P15 协同） | P15 接入时 |
| P3 | OAuth 双 bearer 解析合并（§4.2） | 小优化 | 零 | 现在可做 |
| P3 | error.rs + session.rs 并入 lib.rs（§3.8） | 去文件碎片化 | 零 | 现在可做 |
| 跟踪 | 端到端接线（§2） | 首次被真实路径验证所有 MCP 能力 | 无 | P13/P15/P19 |

---

## 7. 整体评价

Phase 9 的**架构方向是对的**：单一 `mcp-client` 叶子 crate 把 rmcp 隔离在边界内，对外只暴露 canonical `AgentTool`/`ToolDescriptor` 与传输无关的能力快照，复用 config-service 的 `extra` 扩展点与递归合并、复用 policy-engine 的 `PolicyEngine`、复用 auth-service 的 OAuth/SecretBackend——依赖方向干净，符合 ADR-011 与 AGENTS.md §2「Agent Engine 不感知 Provider/Server 名称」的红线。故障隔离（manager.rs）的测试质量是 crate 的最大亮点。

与 P8 同构的问题在**时序与抽象错配**：在零消费者的阶段，它已经为未来长出了单变体 enum、双 RestartPolicy、第二套合并语义、与调度器并存的 `McpApproval` 审批通道，以及在 adapter 内重写完整门禁的管线。前几项是纯冗余，删改零风险；真正需要决策的是 §4.1——让 per-server 策略融入 canonical 调度器，而不是在接入时让 MCP 工具走一条旁路。

按本次 Review 的导向（「优先寻找可以减少代码、模块、接口和概念数量的方案」），最值得做的是 §5 的「删除/简化」两组——它们能在不损失任何当前可观测语义的前提下净减约 250–350 行机器代码（不含测试），让 P15/P19 接线时面对的是一个更小、门禁单一、没有魔法换算的 mcp-client。

---

## 附：调查覆盖与证据

本次 Review 由 3 个 `deepseek_explorer` 并行调查以下不重叠切片，证据均为 `file:line`，关键事实由 Commander 独立复核：

- **内部结构与冗余**：`mcp-client/src/` 全 8 文件 + 与 policy-engine / auth-service / tool-runtime / config-service / tool-api 关键接口交叉核对；逐模块 pub API 盘点、YAGNI 候选、层重叠。
- **端到端接线**：全 workspace `Cargo.toml` 依赖核查（确认 `apps/pawork` → `cli-host`+`app-service`+`cli-command`，四者均不引入 `agent-engine`/`tool-runtime`/`mcp-client`）；`provider_loop.rs` 工具装配路径、`scheduler`/`registry` 调用点、`from_resolved`/`build_client`/`register_discovered_tools` 调用点。
- **canonical 抽象一致性**：config 合并 vs config-service、security vs auth-service 脱敏、oauth vs auth-service OAuth、transport vs rmcp、capabilities 映射 vs tool-api、canonical domain 红线核查。

Commander 独立复核的关键事实：双 `is_loopback_url` 重复（`config.rs:451` / `transport.rs:403`）；`SecretValue` 单变体（`config.rs:292`）；`build_client` 全 workspace 零调用；`profiles.rs:16` 注释把 MCP 划给 P17-5；`cli-host` 将 `Command::Mcp` 映射为 `Placeholder`；`CommandSource::Mcp` 仅为遥测标签。
