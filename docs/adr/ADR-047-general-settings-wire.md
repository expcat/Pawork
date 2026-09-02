# ADR-047：Settings 通用页 wire（`GeneralSettings` 查询 / `SetProxyUrl` 命令，API 1.5）

- **状态**：Accepted（用户 2026-09-02 确认，D1–D4 按拟议执行）
- **日期**：2026-09-02

## 背景

SET-6 逐页立项的第一页是「通用」。2026-09-02 经主代理源码盘点与独立只读调研双确认的基线事实：

- `PaworkConfig` 顶层键仅 `profile` / `default_provider` / `default_model` / `providers` / `models` / `profiles` / `trust_workspaces` / `proxy_url`；`extra` 实际被消费段为 `mcp` / `oauth` / `model_transports` / `provider_protocols`。
- 归属排除：`default_*` 已由「模型与供应商」页管理（SET-5）；`trust_workspaces` 属「权限与审批」页；`mcp` 段属「工具与 MCP」页；`profile` 与默认 provider/model 强耦合，另开写路径会产生双轨；`oauth` / `model_transports` / `provider_protocols` 是内部装配段，不是用户设置面。
- 剩余唯一真实通用键：`proxy_url`——Global 层专属（非 Builtin/Global 层一律剥离，冻结红线），运行时语义已存在。
- 现有 GUI wire 无任何读写通用配置的帧；GUI Settings 唯一写盘入口是 `set_default_model`（Global 层 `write_default_model_pair`）。CLI 无 proxy 子命令。
- 出站面拆分（审查钉死）：`core.http`（OAuth 刷新 / token 交换 / 目录探测）在装配与重载时由 `http_from_config` 构造并缓存；**模型流量**的代理在 `assemble_provider` 装配时从 `config.proxy_url` 拷入各 adapter 的 `http.proxy`，之后不再读 `core.http`；已装配 adapter 只能靠重新装配（`switch_provider` / 重启）换新代理。
- `PaworkConfig::merge_with` 只在 `other.proxy_url.is_some()` 时覆盖，无法把 `Some` 清成 `None`；内存同步必须直接赋值。

本 ADR 只拍板通用页 wire 词汇与语义；不新增 crate、不新增生产依赖、不改 schema、不动其余 Settings 页。

## 拟议决策

### D1 — 新查询 `GeneralSettings`

- `AppQuery::GeneralSettings`，registry 标 `since = V1_5`、仅 GUI available；headless/ACP 不开放（沿用 ADR-046 D5 通道保守策略）。
- 响应走既有 `AppResponse::Data(Value)` 模式，JSON 形状由 golden 钉死：`{ "proxy_url": string | null }`。
- `proxy_url` 是 Global 层专属键，生效值即 Global 持久值（或 null），不另设 source 字段；null 的展示语义（未设置、跟随系统环境变量）由 Desktop 文案承载。

### D2 — 新命令 `SetProxyUrl`：校验先行、原子写盘、有界同会话生效

- `AppCommand::SetProxyUrl { proxy_url: Option<String> }`，`since = V1_5`、仅 GUI available。wire 上 `proxy_url` 字段**必填**：显式 `null` 表示清除，缺字段是帧解码错误（本变体不使用 `#[serde(default)]` / `skip_serializing_if`，区别于既有 Option 字段惯例）；`""` 为非法值，走校验失败保旧。
- Host 语义三步，顺序固定：
  1. **校验 = 完整构造目标客户端**：用 `http_from_config` 同路径（含 `loopback_aware_proxy`）把候选值（含清除后的 None 形态）构造成最终 `reqwest::Client`。非法即 Error 不落盘、旧值保留。校验期就完成客户端构造，写盘后的内存交换不再有可能失败的新建步骤。
  2. **原子写盘**：经 workspace 新增的最小 writer 写 Global 层（保留全部未知字段，同 `write_default_model_pair` 模式；`None` 时移除该键）。写失败即 Error，内存不动。
  3. **内存同步**：直接赋值 `core.config.proxy_url`（含 `None`，禁止 `merge_with`）并换入第 1 步构造好的 `core.http`。
- **生效边界（诚实文案的事实依据）**：同会话立即生效的是新 OAuth 刷新/交换（换入的 `core.http`）、新 API-key 验证与按当前 `config` 现装现探的目录探测（`catalog_state` / `models_overview` 每次按 `config.proxy_url` 重新装配临时 adapter）；**当前活跃供应商连接的模型流量**（`core.provider`）不重建，新代理于该连接下次装配（切换供应商 / 重启 Host）后生效。Desktop 必须把这一边界写进页面文案，不得宣称全局即时生效。
- 响应 `Data` 回执 `{ "proxy_url": string | null }`（写后状态）。该回执非 Secret，进 command ledger 响应缓存可接受（与 `set_default_model` 回执同级）。
- 错误脱敏：`loopback_aware_proxy` 的错误串回显完整 URL，**不得**原样进入 GUI Error / tracing / Diagnostic；handler 统一映射为 `invalid_proxy_url` 错误码 + 不含原文的解析原因文案。proxy URL 可能内嵌 `user:pass@`，但它是用户自有配置而非供应商 Secret，不适用 `ApiKeySecret` 非重放通道；持久记录（日志/诊断）只留类别。

### D3 — 版本策略

沿用 ADR-046 D5 用户已拍板口径：初始未发布版本不采取兼容策略。API minor 升 `V1_5` 仅作记账；registry `since` 只作来源元数据，不产生行为分支；`SUPPORTED_API_VERSIONS` 追加保留现状清单；不新增 `GuiCapability` 变体。

### D4 — golden 先行与定向回归

- golden 先于 handler 检入：client 侧 `general_settings` 查询帧、`set_proxy_url` 设置帧与清除帧（`"proxy_url": null`）各一；server 侧 `general_settings` 响应帧与 `set_proxy_url` 回执帧各一，其中回执帧至少一帧为清除回执（`{ "proxy_url": null }`）。typegen 重新生成三产物并过 `--check`。
- 定向回归上限：主路径两条（设置 → 重查一致；清除 → 重查 null，同属一个 mutation 的两半）；关键失败路径一条（非法 URL fail-closed 保旧）。现有测试可覆盖时不新增。

## 否决支

- **通用 `set_config_value(key, value)`**：无类型逃逸口会绕过冻结的层级剥离与逐键校验语义；typed 命令保持显式。
- **复用 `ProviderAuthStatus` 或 `ModelList` 帧夹带通用配置**：跨域污染既有契约。
- **Desktop 直写 Global 配置文件**：违反 GUI 不直连配置的架构红线。
- **写后重建已装配 adapter**：`assemble_provider` 含凭证解析与远端目录探测，把设置命令变成重网络操作并引入「写盘成功 / 重建失败」分叉；D2 的下次装配生效 + 诚实文案已是充分语义。
- **本页暴露 `trust_workspaces` / `profile`**：前者属「权限与审批」页；后者与默认 provider/model 存在双轨写风险。
- **`proxy_url` 走 `ApiKeySecret` 非重放通道**：用户自有配置不是供应商 Secret，现有 Global 配置文件本就明文承载该键。
- **缺字段默认同 `null` 清除**：静默把残缺帧解释成破坏性清除，违反 fail-closed。

## 后果与实施切片

- Accepted 后按 SET-6a 写入集推进：① `crates/protocol` 新变体 + registry 登记（`since = V1_5`）+ API 1.5 + golden/typegen 先行；② `crates/workspace` 增 `write_proxy_url`；③ `crates/app` handlers + 校验先行构造 + 内存同步；④ `apps/desktop` 通用页（含生效边界文案）。
- 同批回写 `docs/architecture.md` §3.2/§5、`docs/spec/contracts.md`（CON-GUI-01 版本与 CON-REGISTRY-01 计数）、实际写入集的包级 Spec。
- 不改动 schema、事件信封、capability 基线、headless/ACP 通道与其余冻结契约；不新增 crate 与生产依赖。
