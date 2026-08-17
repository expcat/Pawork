# 多账户额度控制、切换、子 Agent 调用与输入缓存 — 实施方案与推荐

> 状态：**已确认**（2026-08-14 用户按推荐通过全部方案 F1–F6，决策原则：**减少实现复杂度、优先缓存命中**；并入 plan 由后续独立任务执行，在此之前不改动 `plan/S*.md` 与代码）。
>
> 决策记录与并入 plan 任务书：[multi-account-quota-plan-merge.md](multi-account-quota-plan-merge.md)（决策唯一入口，含执行期凭证 / 少测试无门禁 / 缓存命中 95-97-99 目标等工作约定与疑问解答归档）。
>
> 依据：[multi-account-quota-reference.md](multi-account-quota-reference.md)（外部开源项目实现逻辑调研，下文引用记作「参考 §N」）+ Pawork V1 既有资产（`provider-control` 13.5k 行、`quota-service` 核心约 6k 行、`usage-ledger`、`orchestration` budget-gate 等，见 [../v1-migration-reference.md](../v1-migration-reference.md) §4.1 与参考 §7 对照表）。
>
> 全文适用的架构红线：Agent Engine 不按 Provider 名走特例（能力差异一律经 registry/capability 数据表达）；Secret 不落数据库、不入日志/事件流/仓库（账户凭证走 `pawork-auth` 的仓库外 `auth.json`，0600、原子写、损坏 fail-closed）；canonical domain 纯净（厂商字段不进 `pawork-domain`/`pawork-api` 核心类型）；所有路由/切换决策事件化、可持久化、可重放。

---

## 0. 功能域划分与结论一览

| 功能域 | 推荐方案 | 主要落点 | 性质 |
| --- | --- | --- | --- |
| F1 多账户模型与凭证 | F1-B：激活 V1 账户层 + 订阅 plan 凭证类型 + auth 文件多凭证 | S6 铺垫 / S11 主体 | 沿用 + 扩展 |
| F2 额度感知与预算控制 | F2-A+B：LocalLedger 派生 + 被动信号捕获；远端适配器保持冻结 | S11 | 沿用 + 小新增 |
| F3 切换与路由策略 | F3-B：会话-账户亲和默认开 + 新会话再平衡 + 分类错误 rebind | S11 | 沿用 + 策略新增 |
| F4 子 Agent 跨供应商调用 | F4-A+B：声明式绑定（默认继承、显式覆盖）+ budget-gate 预算分配 | S9 铺垫 / S11 主体 | 沿用 + 新增契约 |
| F5 输入缓存策略控制 | F5-B：canonical cache 注解 + registry 能力表 + adapter 映射 + 用量入账 | S2 占位 / S5 分段 / S6 全量 | 新增（附加式契约扩展） |
| F6 对外账户池网关模式 | F6-A：近期不做；以 openai-compatible 上游接外部网关；长期按需评估 channels 扩展 | 暂不排期 | 决策登记 |

每个功能域按「目标 → 观察到的模式 → 方案选项 → 推荐与理由 → 契约影响与开放问题」展开。

---

## F1 多账户模型与凭证

**目标**：同一 Provider 下管理多个账户（API key 或订阅 plan OAuth），账户可携带优先级/权重/并发上限/生命周期状态，凭证安全存储，供路由层取用。

**观察到的模式**（参考 §2.3、§3、§6.5）：OpenCode/Pi 内核层均为「一 providerID 一凭证」，多账户靠拆别名 provider 或换配置目录绕行；opencodex/CLIProxyAPI 在代理层做账户池（本地明文 JSON 存 OAuth token，自动刷新）；cc-switch 把账户组织为 SQLite 记录 + 写回各工具配置。生态分层是「内核单凭证 + 外部池化」，内核原生多账户是差异化机会。

**方案选项**：

- **F1-A（最小）**：沿用「一 Provider 一凭证」，多账户靠配置别名 providerID（`glm-coding-a`/`glm-coding-b`）。改动为零，但账户不是一等实体：无优先级/健康度/并发语义，F2/F3 无从谈起。
- **F1-B（推荐）**：激活 V1 `provider-control` 账户层并补两块：
  1. **账户实体沿用 V1**：`ProviderAccount`（priority、weight、max concurrency、lifecycle：Active→CoolingDown→Active / BillingBlocked / Disabled）+ `CredentialMetadata`（仅 `secret_ref`、kind、expiry、refresh state）——契约已冻结（account-control schema v2），直接迁移。
  2. **新增凭证 kind：订阅 plan OAuth**（ChatGPT plan / Claude plan / Copilot 等，对应 [../design.md](../design.md) §6.5 候选 D8）：refresh token 入 Pawork auth 文件（`pawork-auth` 已有 OAuth PKCE/Device/refresh 全流程），凭证解析链沿用 auth 文件 → env fallback。多账户的 secret_ref 命名规约：`<provider>/<account_id>`。
  3. **CLI 用户面**：`pawork accounts list/add/remove/enable/disable`（S11 与 `pawork usage` 同批），`auth set-key` 扩展 `--account` 维度。
- **F1-C（完整 UX）**：对齐 opencodex dashboard 体验（账户池页面、一键配额刷新、selection order 拖拽）——属 GUI 范畴，推迟到 S7 最小壳之后的 Settings 增量，不进本批。

**推荐 F1-B**。理由：词表与状态机已是 V1 冻结资产（参考 §7 对照表），激活成本远低于新造；auth 文件已有 0600、原子写、损坏 fail-closed、掩码展示与日志脱敏基线；plan OAuth 是两条真实测试通道（GLM Coding Plan、OpenCode Go）之后最现实的账户形态。

**契约影响与开放问题**：plan-credential kind 为 account-control schema 的**附加**变体（unknown-field fail-closed 契约下需登记 schema 迁移）；ToS/封号风险需在文档显著声明（参考 §6.7——Anthropic 已封锁第三方 OAuth 的先例）；**不做**身份伪装类手段（Claude Code UA 伪装、`identity-confuse`），宁可少接一家。附属候选 G6：`pawork-compat`（S9）增加账户/端点只读导入源（`~/.codex/auth.json`、cc-switch SQLite、CLIProxyAPI auth-dir、opencodex config），导入的 secret 直接转存 Pawork auth 文件、不落仓库或中间文件。

---

## F2 额度感知与预算控制

**目标**：回答「这个账户还剩多少额度、下一个任务该派给谁、什么时候必须停」。

**观察到的模式**（参考 §3、§4.2、§6.2）：三形态叠加——本地记账（litellm 层级预算、new-api quota 折算）、被动信号（429/Retry-After、`usage_limit_reached`、成功响应配额头；CLIProxyAPI 冷却退避、opencodex 捕获配额头）、主动探测（opencodex 三窗口刷新、CLIProxyAPI-Plus 周额度阈值停用）。可信度分级是共同做法。

**方案选项**：

- **F2-A（S11 已规划基线）**：LocalLedger 派生——V1 `usage-ledger`（dedup_key 幂等）+ `LedgerQuotaAdapter` 按 Rolling5h/Weekly/Monthly 滚动派生 `Derived` 快照；orchestration budget-gate 消费投影。零网络请求、零 ToS 面，但只见自己消耗、看不到账户在其他客户端的用量。
- **F2-B（推荐叠加）**：**被动配额信号捕获**——Provider adapter 在正常请求的响应头/错误体中捕获配额信息（Anthropic `anthropic-ratelimit-*` 头、OpenAI `x-ratelimit-*` 与 plan 窗口字段、`Retry-After`、`usage_limit_reached` 类错误体），归一为 `QuotaSnapshot`（confidence 按来源定 `Exact`/`Derived`）写入 quota 缓存与账户健康状态。opencodex「成功响应捕获配额头」同款；不新增任何请求。归一化放 provider adapter（厂商差异不进 core，符合红线）。
- **F2-C（保持冻结）**：激活六厂商远端适配器 + `RefreshScheduler`（约 8k 行，主动轮询官方 usage/billing API）——维持 [../v1-migration-reference.md](../v1-migration-reference.md) §4.4 冻结候审不变，激活条件（真实需求 + 账号归属落地）不因本批候选自动满足。
- **F2-D（保持冻结）**：WebScrape 兜底（GLM Coding Plan 5h/周窗口目前无 API，只能 Scraped）——维持 opt-in 冻结。

**预算执行规则沿用 V1**（参考 §7）：仅 fresh `Exact` 且明确耗尽的信号可触发硬停止；`Derived`/`Scraped`/stale 只产软告警。budget-gate 按窗口余量为子 Agent 分配预算（联动 F4）。

**推荐 F2-A+B**。理由：A 是 S11 既定项；B 增量小（adapter 内解析 + 一条快照写入路径）、把「额度感知」从纯本地估算升级为「用真实信号校准」，且完全被动。C/D 不动，避免把冻结资产隐式解冻。

**契约影响与开放问题**：`QuotaSnapshot`/`QuotaProvenance` 契约已有，B 只新增来源枚举值（附加式）；开放问题——plan 窗口 reset 时间的时区/不确定性表达（V1 `QuotaReset::uncertain` 已可表达）；各厂商配额头的覆盖面需在 S6 迁移各 adapter 时逐家登记。

---

## F3 多账户切换与路由策略

**目标**：新会话选对账户、会话中不乱跳、账户出问题时正确切换，且一切可解释、可重放。

**观察到的模式**（参考 §3.4、§4.4、§6.1、§6.3、§6.6）：sticky session 是订阅池代理标配（CRS 内容 hash、CLIProxyAPI session-affinity、opencodex thread affinity、claude-code-hub `SET NX` 首成锁、antigravity Sticky 策略）；新会话才再平衡；错误分类驱动 failover（分类错了会误惩罚账户）。

**方案选项**：

- **F3-A（S6 已规划基线）**：手动切换——REPL `/provider` `/model`、配置 `default_provider`。必要但不满足池化。
- **F3-B（推荐）**：**缓存感知亲和 + 新会话再平衡**，全部落在 V1 `provider-control` 既有机制上：
  1. **会话-账户亲和默认开**：`SessionBinding`（Unbound→Bound→Rebinding→Bound）作为默认行为；绑定键 = session_id（Pawork 自有会话体系，无需像代理们那样对请求内容做 hash）。
  2. **新会话再平衡**：新 session 首次 `AcquireRequest` 走 `RoutingPolicy` 完整过滤链（capability → tenant → health → priority → affinity → weighted/fill-first → concurrency），**新增一个「配额余量优先」策略**（对齐 opencodex `quota` 策略与 CLIProxyAPI-Plus 排序：比较各账户最紧窗口的剩余比例，消费 F2 快照），与既有 SWRR/Fill-First 并列可选。
  3. **Rebind 仅由 `ErrorClassifier` 触发**：沿用 V1 错误表（RateLimited → scope-aware cooldown、QuotaExceeded hard → failover、AuthRejected → refresh-once、BillingBlocked → 显式恢复、ClientCancelled/ContextTooLarge/ProtocolIncompatible 不轮换）——该表已覆盖 CRS issue #1000 的「非限流 429」教训。
  4. **决策可观测**：每次选择/淘汰/rebind 进 `RouteDecision`（不含 Secret）并事件化；缓存命中率（F5 的 usage 数据）纳入账户健康视图，作为「亲和值不值得保」的量化依据。
- **F3-C（不作默认）**：请求级轮换（RoundRobin per request）——破坏 prompt cache（参考 §5.4），仅作为 RoutingPolicy 既有可选策略保留，文档标注适用场景（无缓存诉求的批量吞吐）。

**推荐 F3-B**。理由：与外部最佳实践收敛一致，且 V1 的 binding/routing/health 三件套已具备全部骨架，本批实际新增只有「配额余量优先策略 + 亲和默认开 + 命中率指标」三点。

**契约影响与开放问题**：`RouteDecision`/binding 事件已在 V1 词表；开放问题——绑定粒度默认 session 还是 run（建议 session，run 级可配）；亲和过期时长默认值（对齐上游 cache TTL：Anthropic 5m/1h、OpenAI ~30m，建议默认 1h 可配）；**不做** CLIProxyAPI 的 `identity-confuse` 类身份重写（合规红线，见 F1）。

---

## F4 子 Agent 跨供应商调用

**目标**：编排（supervisor）派发的每个子 Agent 可声明自己的 provider/model/账户约束与预算，路由层据此供给，事件流可区分归属。

**观察到的模式**（参考 §2.1、§2.2、§4.1、§6.4）：三模式——声明式绑定（opencode `agent.model` + 权限派生 + 深度限制；Claude Code agents frontmatter）、in-band 标签（CCR `<CCR-SUBAGENT-MODEL>`，是「改不了客户端」的补丁）、模型即子代理槽位（opencodex）。Pawork 同时控制引擎与编排两侧，**in-band 标签无存在理由**。缓存取舍见参考 §5.5：独立上下文 + 便宜模型适合短任务；共享大前缀高频往返适合同模型同账户。

**方案选项**：

- **F4-A（推荐主体）**：**声明式绑定**——Agent Profile（S9 `pawork-resources` 的 profiles 契约）与编排 spawn 参数中声明 `provider` / `model` /（可选）`account_hint` / `budget`；supervisor spawn 时写入 `RouteContext`，由 `provider-control` 完成账户选择（子 Agent 不直接接触凭证，符合「Agent 只提交 AcquireRequest」红线）；budget-gate 按声明为子 Agent 划预算（V1 budget trait 注入已在 [../v1-migration-reference.md](../v1-migration-reference.md) §4.1 映射总表第 29 行规划）。S11 多 Agent demo（GLM + OpenCode Go 双子 Agent）即最小验收场景，无需另立验收。
- **F4-B（推荐叠加，默认行为）**：**默认继承、显式覆盖**——未声明绑定的子 Agent 继承父的 provider/model/账户绑定（同账户共享缓存前缀、行为可预期），声明了则覆盖。对应 opencode 的继承语义。
- **F4-C（不采纳）**：CCR 式 prompt 内标签路由——引擎两侧皆可控时属多余间接层，且污染 prompt、不可类型化审计。

**推荐 F4-A+B 组合**。

**契约影响与开放问题**：Agent Profile schema 增加绑定字段（S9 契约激活时一并定型，避免后补破坏冻结契约）；`TeamEvent`/子 Agent 事件已含归属，需确认 `ProviderRequestStarted` 携带 account 维度（脱敏 hint，不含 secret）；开放问题——子 Agent 并发对单账户 max concurrency 的挤占策略（建议沿用 lease 并发上限 + fill-first 下沉，不为子 Agent 特设通道）。

---

## F5 输入缓存策略控制

**目标**：把 prompt caching 从「各 adapter 自行其是」升级为 canonical 可配置、可观测的一等能力：断点/TTL/亲和键统一策略化，缓存用量入账，命中率可查。

**观察到的模式**（参考 §5）：厂商机制分显式断点（Anthropic/Bedrock/Qwen 显式）与隐式前缀（OpenAI/Gemini/DeepSeek/GLM/Kimi）两族 + 亲和键（OpenAI `prompt_cache_key`）；客户端实践收敛为「静态前缀 + 少量滑动断点 + 会话稳定亲和键」；compaction 与缓存天然冲突需折中；用量字段各家不同但都可归一为 cache_read/cache_write 二元。

**方案选项**：

- **F5-A（现状延伸）**：各 adapter 内部硬编码断点（S6 迁移 V1 `provider-anthropic` 的 prompt cache 即此形态）。能用，但策略不可配、不可跨厂商观测、compaction 联动无从挂接。
- **F5-B（推荐）**：**三层设计**：
  1. **canonical 注解层**（`pawork-provider-core` / canonical request）：`CanonicalModelRequest` 增加缓存策略字段（枚举：`Off` / `Auto` / `Explicit { retention: Default | Long }`）与「前缀稳定性分段」标注——context-engine 产出上下文时按（static system｜tools｜history｜dynamic tail）分段标记可缓存边界。**不含任何厂商字段**（cache_control 不进 canonical 类型）。
  2. **adapter 映射层**（`pawork-providers` + model registry）：缓存能力进 registry 数据表——`cache_kind`（explicit/implicit/none）、`min_cacheable_tokens`、`supports_ttl`、`affinity_key_kind` 等；显式族映射为断点（缺省策略对齐 pi/opencode 收敛实践：system 尾 + 末 tool 定义 + 滑动末 user；TTL 按 retention 映射 5m/1h 或 24h retention 参数）；隐式族映射为亲和键（`prompt_cache_key` / session 头 = Pawork session_id）。Engine 全程零厂商分支（红线），一切查表。
  3. **用量与观测层**：cache_read/cache_write token 归一进 usage（`ModelResponseSummary` 与 usage-ledger 记录增列），计价按 registry 单价（写入溢价/命中折扣）；`pawork usage` 与事件流展示命中率；命中率喂给 F3 的账户健康视图。
  4. **配套纪律（context-engine/compaction，不新增包）**：static-first 排序、工具列表确定性排序、历史 append-only（V1 context 契约已含确定性要求）；**compaction 视为缓存重置事件**——触发时机偏向任务自然边界、压缩后首请求即预热新前缀、`CompactionCompleted` 事件附缓存影响标注。
- **F5-C（不属于本域）**：网关式响应缓存/语义缓存（litellm cache）——那是输出缓存，与输入 prompt caching 无关，不做。

**推荐 F5-B**。理由：这是四个功能域里唯一「V1 没有对应资产」的净新增，但外部实践已高度收敛（参考 §5.2 四家客户端做法一致到细节），可以低风险抄收敛解；分层设计保住两条红线（canonical 纯净、engine 零厂商分支）。

**契约影响与开放问题**：`CanonicalModelRequest`/`ModelResponseSummary` 字段新增为**附加式**，serde 向后兼容 + golden 先行（[../design.md](../design.md) §3.2 原则）；分阶段：S2 留注解占位（契约激活时字段就位、宁可闲置）、S5 context 分段产出、S6 adapter 映射与用量入账全量。开放问题——GLM Coding Plan 套餐不参与缓存计费（参考 §5.1），计价表需按「计费模式」区分套餐/按量；OpenAI `prompt_cache_key` 每键 ~15 rpm 限制在高并发编排下的分片策略（子 Agent 家族共享 key 时，参考 §5.5 Codex 命中率 62%→9.6% 的反例）。

---

## F6 对外账户池网关模式（决策项）

**问题**：是否让 `pawork` 像 opencodex/CLIProxyAPI 那样对外暴露 OpenAI/Anthropic 兼容端点，把自己的账户池服务给其他客户端（其他 CLI/IDE）？

**方案选项**：

- **F6-A（推荐）**：**不内建**。近期需求两条腿走：① Pawork 作为消费者——经 openai-compatible adapter 把外部网关（opencodex、CLIProxyAPI 等）当上游（S0 起仅需 base_url，已支持，[../task-guide.md](../task-guide.md) §5 已注明 opencodex 网关场景）；② Pawork 自身多账户能力对内服务（F1–F4）。
- **F6-B（长期候选，P3）**：以 `pawork-channels` 扩展 feature 评估——V1 `client-claude-gateway` / `client-codex-app-server`（14.4k 行 channels 资产）已有「外部客户端协议 → Pawork」的翻译层，反向暴露「模型代理端点」在技术上是其邻接能力；若未来有真实需求（如团队共享 Pawork 账户池），在 S12 审查与高优先级整改后按 [../../ROADMAP.md](../../ROADMAP.md) §3.3 候选流程评估。
- **F6-C（不做）**：独立网关 app——偏离产品定位（Coding Agent 而非 API 网关），且订阅账户转售式代理的 ToS 风险最重（参考 §6.7）。

**推荐 F6-A，登记 F6-B 为 P3 候选**。

---

## 7. 分阶段落地图（确认后生效；本次不改动阶段计划文件）

| 阶段 | 并入内容 | 涉及包 |
| --- | --- | --- |
| S2 | F5-B-1 canonical 缓存注解占位（契约激活即完整形状，字段暂闲置） | api/provider-core（契约） |
| S5 | F5-B-1 context 前缀分段产出；缓存用量并入 token 统计路径 | engine、provider-core、session |
| S6 | F1-B-2 plan 凭证 kind 铺垫 + auth 文件多凭证命名；F5-B-2/3 adapter 缓存映射、registry 能力表、用量入账 | providers、auth、provider-core、config |
| S9 | G6 账户/端点导入源（Claude/Codex/opencodex/cc-switch/CLIProxyAPI 布局）；F4 Agent Profile 绑定字段随 profiles 契约定型 | compat、resources |
| S11 | F1-B 账户层激活与 CLI；F2-A+B 额度感知；F3-B 亲和 + 再平衡 + 配额余量策略；F4-A+B 子 Agent 绑定与预算 | provider-control、quota、control-plane、orchestration、cli |
| 冻结不变 | quota 六厂商远端适配器 + WebScrape（F2-C/D）；激活条件沿用 [../v1-migration-reference.md](../v1-migration-reference.md) §4.4 | — |
| 明确不做 | 请求级默认轮换（F3-C）、in-band 子代理标签（F4-C）、身份伪装/identity-confuse、响应缓存（F5-C）、独立网关 app（F6-C） | — |

与 [../design.md](../design.md) §5 候选表的对应：G1↔F1、G2↔F2、G3↔F3、G4↔F4、G5↔F5、G6↔F1 附属、G7↔F6。

## 8. 需要拍板的决策清单

1. **F1-B**：订阅 plan OAuth 凭证是否纳入范围（含 ToS 风险声明的接受度；不做身份伪装的立场确认）。
2. **F3-B**：会话-账户亲和默认开 + 「配额余量优先」新路由策略，是否作为多账户场景默认行为（绑定粒度 session、亲和 TTL 默认 1h 可配）。
3. **F5-B**：canonical 缓存注解的契约扩展（CanonicalModelRequest/ModelResponseSummary 附加字段，S2 起占位）是否批准——这是唯一动冻结契约形状的项。
4. **F6-A**：确认近期不内建网关模式（外部网关作上游 + 对内账户池）。
5. **落地方式**：确认后是否由本批任务把 §7 落地图并入对应 `plan/S*.md` 阶段计划文件（本次调研任务未触碰阶段计划，遵守「未确认不排期」）。
