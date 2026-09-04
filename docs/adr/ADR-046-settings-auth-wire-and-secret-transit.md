# ADR-046：Settings 模型与供应商 wire（auth status / 非重放 Secret 写入 / OAuth 生命周期 / 默认项 mutation，API 1.4）

- **状态**：Accepted（用户 2026-09-01 确认：初始未发布版本，不采取兼容策略，以最佳实现为目标；D1–D4/D6 按拟议执行，D5 按修订版执行）
- **日期**：2026-09-01

## 背景

Settings 活动线（[Feature Spec](../spec/settings.md)、[ADR 索引](../architecture.md)）的硬前置：为 Desktop 的「模型与供应商」页锁定 Host-driven contract。2026-09-01 源码盘点确认的基线事实：

- `AuthStart{provider_id, flow}` / `AuthRemove{provider_id}` 类型已在 `AppCommand` 中存在，但 registry 对 GUI/headless/ACP 三通道全部 `available: false`（[registry.rs:167](../../crates/protocol/src/app/registry.rs)），wire 上无实际消费者；`auth_status`/`oauth_begin` 等 Host 能力只被 CLI 进程内消费。
- `ModelList` 是已对 GUI 开放的 query，但只返回目录条目，不含 descriptor、认证状态与目录来源/错误。
- command ledger 只持久化**响应**信封（[idempotency.rs:204](../../crates/app/src/idempotency.rs)），请求 payload 不落盘；错误响应不缓存（`should_cache`）。请求侧无按命令类型的 non-replayable 标记先例。
- 配置已有 `default_provider`/`default_model` 六层只读合并，但**不存在任何写盘入口**（仅进程内 `from_cli` 覆盖与运行期 switch）。
- `GuiCapability` 现有 5 变体为粗粒度能力族；本 ADR 不新增（理由见 D5）。
- Secret 红线：明文 token 只允许停留在 SecretBackend 内部、adapter 瞬时 `expose_secret()`、受保护 AEAD 信封（[flows.md §4](../spec/flows.md)）；wire 瞬时传递的生命周期尚未定义。

本 ADR 只拍板 wire 词汇、Secret 瞬时传递语义与版本策略；Host 门面（verify-then-replace、config writer、descriptor 装配）属 SET-2，四家认证与目录属 SET-4/5。不新增 crate、不新增生产依赖、不改 schema。

## 拟议决策

### D1 — 新查询 `ProviderAuthStatus`：descriptor + 认证状态 + 目录状态的最小只读面

- `AppQuery::ProviderAuthStatus { provider_id: Option<ProviderId> }`，registry 标 `since = V1_4`、GUI available；headless/ACP 不开放（首期为认证本机 GUI 专用，见 Feature Spec §6.3）。
- 响应走既有 `AppResponse::Data(Value)` 模式（ModelList/WorkspaceAdd 先例），JSON 形状由专用 golden fixture 钉死，逐 provider 包含：
  - descriptor：`provider_id`、`display_name`、`endpoint_label`（端点语义说明，非 Secret）、`auth_methods`（Host 声明的可用认证方法数组，Desktop 禁止按品牌硬编码分支）；
  - 认证状态：`none` / `connecting` / `connected{method, masked_credential}` / `error{sanitized}`；脱敏复用 app `auth_status` 的 File/Env/None + masked 结构；
  - 目录状态：`remote` / `fixed_fallback{snapshot_label}` / `unavailable{sanitized_error}`，附 `fetched_at`；认证成功与目录成功是两个独立字段（SET-009）。
- 模型条目本身仍由既有 `ModelList` 提供，不复制第二套目录查询。

### D2 — 新命令 `AuthSetApiKey`：非重放 Secret 瞬时传递 + verify-then-replace

- `AppCommand::AuthSetApiKey { provider_id: ProviderId, api_key: ApiKeySecret }`，`since = V1_4`、仅 GUI available。
- `ApiKeySecret` 为 protocol 侧 newtype：wire 必须 Serialize/Deserialize，但 `Debug` 恒输出 `[REDACTED]`（`ClientAuthentication` 先例），不实现 `Display`，不进入任何 `ProviderError`/diagnostic 文案。
- **非重放语义**：Secret 只存在于单次请求帧的内存传输（本机 UDS/named pipe + token proof，无 TLS 需求）。持久化安全由三道既有事实加一条新断言保证：command ledger 只存响应不存请求（已核实）；tracing 不打印 command payload（已核实，本 ADR 要求保持）；响应只含脱敏元数据（本 D 要求）；新增 Secret 负断言回归（D6）。
- 命令语义（SET-2 实现，本 ADR 锁定 wire 承诺）：Host 先在内存中验证新 key（调用该通道最廉价已认证端点），成功后原子写入 auth backend 替换旧值；**验证失败返回 Error，旧凭证保留**。未知 provider、该 provider 未声明 API-key 方法、断线均 fail-closed。
- 响应 `Data` 只含 `{provider_id, method: "api_key", masked_credential, verified_at}`；非 Error 响应进 ledger 缓存是安全的（不含 Secret），重试同 `command_id` 直接命中缓存响应、不重复执行，幂等不依赖持久化 Secret payload。
- `flow` 语义不混入本命令：API key 与 OAuth 是 descriptor 里两个独立 auth method。

### D3 — 开放 `AuthStart`/`AuthRemove` 的 GUI 语义 + 新命令 `AuthCancel` + 新事件 `AuthChanged`

- `AuthStart{provider_id, flow}`：registry 改 GUI available、`since = V1_4`。它目前在全部远端通道关闭、wire 零消费者，因此本 ADR 直接定义其 GUI 语义而非兼容演进：启动指定 OAuth flow，响应 `Data` 携带 `{verification_url, user_code: Option, expires_at: Option}`；随后 Host 后台轮询，进度经 D3 事件下发。同 provider 已有进行中认证时返回 busy Error，不并发覆盖。
- `AuthRemove{provider_id}`：GUI available、`since = V1_4`。语义不变：移除该 provider 当前连接的凭证（幂等，无连接时 fail-closed 报 Error）；Desktop 负责确认交互，协议层不加确认字段。
- 新命令 `AuthCancel { provider_id }`：取消进行中的 OAuth 等待，不写半成品凭证；无进行中操作时幂等 Accepted。否决复用 `AuthRemove` 表达取消（移除已存凭证 ≠ 取消进行中流程）。
- 新事件 `AppEvent::AuthChanged { provider_id, state }`，`state` 为 serde 枚举 `Pending / Succeeded{method, masked_credential} / Failed{sanitized_error} / Cancelled / Expired / Removed`；归属既有事件流机制，不新增 stream 变体；Desktop 收到后刷新 `ProviderAuthStatus`。沿用 ADR-045 D2 结论：只加快照不加 live 事件会让断线前操作不可见，予以否决。事件无条件推送，不做版本门控（见 D5）。

### D4 — 新命令 `SetDefaultModel`：默认 provider/model 的 Host mutation

- `AppCommand::SetDefaultModel { provider_id: ProviderId, model_id: String }`，`since = V1_4`、仅 GUI available；provider 与 model 原子成对，不允许分两次提交造成跨供应商半成品。
- Host 校验 `model_id` 属于该连接当前可运行目录（远端或固定回退均可，但必须是 adapter 可运行集合），校验失败 Error 且不落盘。
- 持久化写入 Global 层配置的既有 `default_provider`/`default_model` 字段（六层优先级不变：Workspace/Session/Run 覆盖仍生效；Settings 改的是用户级默认）。SET-2 在 workspace 补最小 writer；本 ADR 不改变配置 schema 与层级语义，故 CON-CONFIG-01 无形状变化。
- 响应 `Data` 回执 `{provider_id, model_id}`；Desktop 读到回执后才更新 Composer，失效降级（目录变化导致默认模型不可用）由读取侧按既有规则显式提示，不静默跨供应商切换。

### D5 — 版本策略（修订）：minor 1.3 → 1.4 仅作记账，无兼容门控

- 用户拍板：当前为初始未发布版本，不采取兼容策略，一切以最佳实现为目标。因此本 ADR 不引入任何面向旧客户端的行为门控：`AuthChanged` 事件不按协商 minor 门控、直接推送；registry `since` 仅作来源元数据，不产生行为分支。
- `API_VERSION` 升 `V1_4`，`SUPPORTED_API_VERSIONS` 追加保留现状清单（1.0–1.3 的保留仅是握手清单不动，不代表承诺旧客户端兼容）。
- **不新增 `GuiCapability` 变体**：理由是设计最小性——registry availability 已是「宣告 = 授权 = 实现」单源，Settings 认证没有独立于连接认证之外的授予场景；而非旧客户端兼容顾虑。
- headless/ACP 三通道对全部新词汇与 `AuthStart`/`AuthRemove` 保持关闭；未来开放需另立 ADR。

### D6 — golden/typegen 先行与 Secret 负断言

- golden 先于 handler 实现检入，新增 fixture：client 侧 `auth_set_api_key`、`auth_start`、`auth_remove`、`auth_cancel`、`set_default_model`、`provider_auth_status`（查询帧）各一；server 侧 `auth_changed` 事件帧、`provider_auth_status` 响应帧、`auth_set_api_key` 脱敏响应帧各一。typegen 重新生成三产物并过 `--check`。
- Secret 负断言定向回归（随 SET-1/SET-2 落地，不推迟）：真实形态假 key 经完整 GUI 链路后，扫描 command_ledger.sqlite3、会话事件、日志输出、协议 fixture，断言零明文；`AuthSetApiKey` 错误路径断言旧凭证保留。

## 否决支

- **复用 `AuthStart` 携带 api_key 字段**：混淆 OAuth 与 API key 两种 auth method 语义，`flow` 字段无法自证；descriptor 明确要求方法分离。
- **新增 `GuiCapability` 变体宣告 Settings 能力**：registry availability 已足够表达授权，无独立授予场景，属多余词汇。
- **Secret 经 snapshot/event/query 下行**：任何下行路径都可能进投影缓存与诊断；Secret 只许上行一程，状态一律脱敏引用。
- **阻塞式 `AuthStart`（响应等到 OAuth 完成）**：无法表达取消/过期/断线，且占用 request 通道；进度走事件是既有架构。
- **为 Settings 新建模型目录查询**：`ModelList` 已覆盖条目；`ProviderAuthStatus` 只补来源/状态摘要，避免第二套目录。
- **headless/ACP 同步开放 Secret 写入**：Feature Spec §6.3 明确首期不开放，避免未经产品验收的通道获得凭证能力。
- **`SetDefaultModel` 拆成 set-default-provider / set-default-model 两命令**：中间态会产生跨供应商的 provider/model 错配。

## 后果与实施切片

- Accepted 后按 SET-1 写入集推进：① `crates/protocol` 新变体 + `ApiKeySecret` + registry 登记（`since = V1_4`）+ API 1.4 + golden/typegen 先行；② 仅为 contract harness 必需时触及 `crates/client`、`crates/app`；不写生产 handler（SET-2）。
- 同批回写 `docs/architecture.md` §3.2/§5、`docs/spec/contracts.md`（CON-GUI-01 版本与 CON-REGISTRY-01 计数）、`docs/spec/crates/{protocol,client,app}.md`。盘点已发现 protocol.md §1/§2「19 AppCommand」与 §7「32 fixture」两处与源码（20 / 34）不符，同批修正。
- SET-2 起 Host 行为按 D2/D4 承诺实现：verify-then-replace、config Global 层 writer、descriptor 装配。
- 不改动 schema、事件信封、capability 基线、headless/ACP 通道与其余冻结契约；不新增 crate 与生产依赖。
