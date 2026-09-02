# ADR-048：Settings 权限与审批页 wire（`PermissionsSettings` 查询 / `SetApprovalMode` 命令 / 开放 `WorkspaceTrust`，API 1.6）

- **状态**：Accepted（用户 2026-09-02 确认，D1–D5 按拟议执行）
- **日期**：2026-09-02

## 背景

SET-6 逐页立项的第二页是「权限与审批」。任务书锁定的最小真实能力是「当前 approval mode/trust 的读取与受控修改」，明确不做「绕过 Policy、静默 Allow」。2026-09-02 经主代理源码实读与两路独立只读调研确认的基线事实：

- `ApprovalMode` 五档（`always_ask` / `ask_for_writes` / `ask_for_dangerous` / `never_ask` / `read_only`，默认 `ReadOnly`，`crates/policy/src/mode.rs`）；只经 CLI `--approval-mode` 启动参数注入（`AppLoadOptions` → `configure_approval`），**`PaworkConfig` 无对应键、无持久化、无运行时查询/修改 API**。GUI 宿主启动时保留装配值（`cli/src/gui.rs`），GUI 用户不传参即永远 `ReadOnly`。
- `workspace_trusted` 是 `ApprovalService` 内存态，来源为 CLI `--trust-workspaces` 或 Global 层 `trust_workspaces`（`Option<bool>`，builtin 默认 false）。冻结红线：仅 Builtin/Global 层可设，profile/workspace/session/run 层一律剥离并告警（loader.rs）。writer 层现有 `write_default_model_pair` / `write_proxy_url`，无 trust 写函数。
- **生效快照语义（既有架构）**：run 启动时把 `approval_mode` / `workspace_trusted` 拷入 `SessionLoopCtx` / `ToolSchedulerConfig`，进行中的 run 不随宿主后续变更；Policy 决策链每调用组 `PolicyInput` 现场裁决，灾难命令地板（`rm -rf /` 等）在任何 mode 下保持 Deny/升档。
- wire 现状：审批决策帧齐全（`ToolApprove` / `ApprovalDecision` / `ToolApprovalRequired` 事件 / snapshot 段，均为运行时逐次审批）；`WorkspaceTrust { workspace_id, trusted }` 自 R3 registry 落地即登记但**从未有 handler**，GUI/headless 均不可用（死词汇）；全协议无 approval mode 的 query/command。
- Desktop 侧无任何 trust 概念与权限设置页；审批展示为 Timeline 内联卡 + 状态点（纯瞬态 pending）。Settings 壳已有 SET-3/SET-6a 的查询→渲染→命令→刷新与 capability gate 模式可复用。

本 ADR 只拍板权限与审批页 wire 词汇与语义；不新增 crate、不新增生产依赖、不改 config schema、不动 Policy 决策链与其余 Settings 页。

## 拟议决策

### D1 — 新查询 `PermissionsSettings`

- `AppQuery::PermissionsSettings`，registry 标 `since = V1_6`、仅 GUI available；headless/ACP 不开放（沿用 ADR-046 D5 通道保守策略）。
- 响应走既有 `AppResponse::Data(Value)` 模式，JSON 形状由 golden 钉死：
  `{ "approval_mode": string, "workspace_trusted": bool, "trust_workspaces_global": bool | null, "workspace_id": string }`。
  （实现期修订：增补 `workspace_id` 为 Host 权威 attached id。原拟 D3 由 Desktop 从 snapshot 取 id，
  审查发现 snapshot 的注册表首项在多 workspace 状态下与 Host attached 不一致；透出权威 id 使
  校验方与发送方同源，且不引入新查询。）
- 三字段语义分列、互不合并：
  - `approval_mode`：当前会话生效值（内存态，snake_case 串，与 `ApprovalMode` serde 表示一致）；
  - `workspace_trusted`：当前会话内存态（对之后启动的 run 生效的值）；
  - `trust_workspaces_global`：Global 层持久值（`None` → `null`，展示语义「未设置（默认不信任）」由 Desktop 文案承载）。

### D2 — 新命令 `SetApprovalMode { mode }`：会话内生效、不持久化

- `AppCommand::SetApprovalMode { mode: String }`，`since = V1_6`、仅 GUI available。`mode` 为必填 snake_case 串；未知值解码/校验失败即 Error，旧值保留（fail-closed）。
- Host 语义：写入 `ApprovalService`（新增运行时 setter，`configure` 保持启动装配专用），**只影响之后启动的 run**（快照语义）；进行中的 run 不中断、不升格、不降格。
- **不持久化**：重启 Host 后回到 CLI 参数/builtin 默认（`ReadOnly`）。这是有意的 fail-closed 安全默认——放宽权限不跨会话静默延续。持久化到 Global config 属 schema 演进与安全敏感决定，登记为后续候选，本片不做。
- 响应 `Data` 回执 `{ "approval_mode": string }`（写后状态）。回执无 Secret，进 command ledger 响应缓存可接受（与 `set_default_model` 回执同级）。
- 安全语义：所有变更由用户在 Settings 显式选择，不构成静默 Allow；Policy 灾难命令地板在任何 mode（含 `NeverAsk`）下保持 Deny/升档，本片不动 `pawork-policy` 任何裁决逻辑。

### D3 — 开放并实装既有 `WorkspaceTrust`：会话内信任切换、不写盘

- 复用冻结词汇 `AppCommand::WorkspaceTrust { workspace_id, trusted }`，新增 Host handler 并把 registry 的 GUI 可用性开放（`since` 维持原登记值，可用性变化记 `V1_6` 元数据）；不新增词汇。
- 语义：切换当前会话的 `workspace_trusted` 内存态，对之后启动的 run 生效；**不写盘**，重启后跟随 Global 配置。这与「信任单个 workspace 持久化」刻意区分——后者与既有红线（仅 Global 层可持久化 trust）语义错位，本片不做。
- `workspace_id` 必须匹配当前 attached workspace，不匹配即 Error（fail-closed）；GUI 场景 Host 只 attach 一个 workspace，Desktop 从 `PermissionsSettings` 响应透出的 Host 权威 attached id 原样回填（见 D1 实现期修订）。
- 响应 `Data` 回执 `{ "workspace_trusted": bool }`（写后状态）。
- `trust_workspaces_global` 本片只读展示；Global 层写（`write_trust_workspaces` + 内存同步）登记为后续候选，需要时另立小切片。

### D4 — 版本策略

沿用 ADR-046 D5 用户已拍板口径：初始未发布版本不采取兼容策略。API minor 升 `V1_6`（`SUPPORTED_API_VERSIONS` 追加 1.6）仅作记账；registry `since` 只作来源元数据，不产生行为分支；不新增 `GuiCapability` 变体（`WorkspaceTrust` 复用既有 `Approvals` capability）。

### D5 — golden 先行与定向回归

- golden 先于 handler 检入：client 侧 `permissions_settings` 查询帧、`set_approval_mode` 命令帧、`workspace_trust` 命令帧各一；server 侧 `permissions_settings` 响应帧（`trust_workspaces_global` 为 `null` 与布尔各覆盖其一）、`set_approval_mode` 回执帧、`workspace_trust` 回执帧各一。typegen 重新生成三产物并过 `--check`。
- 定向回归上限：主路径两条（`set_approval_mode` → 重查一致；`workspace_trust` 匹配 id → 内存切换 → 之后启动的 run 生效）；关键失败路径一条（`workspace_trust` id 不匹配 fail-closed 保旧）。现有测试可覆盖时不新增。

## 否决支

- **持久化 `approval_mode` 到 Global config**：把放宽权限跨会话静默延续，接近「静默 Allow」边界且属 schema 演进；首片不做，登记候选。
- **新增 `SetTrustWorkspacesGlobal` / 本片写 Global trust**：「信任当前 workspace」与 Global「信任所有 workspace」语义错位；会话内切换已是真实受控修改，Global 写留候选。
- **运行时改造进行中 run 的 mode/trusted**：快照语义是既有架构（`SessionLoopCtx` 值拷贝），中断/升格进行中的 run 引入新的竞态面，无当前需求支撑。
- **新增 `PermissionsChanged` 事件广播**：当前唯一消费者是 Settings 页自身，回执 + 页级重查充分；未来多处消费时再演进。
- **通用 `set_config_value(key, value)`**：同 ADR-047 否决，无类型逃逸口绕过逐键校验。
- **Desktop 直写配置或直接调 `pawork-policy`**：违反 GUI 不直连配置/业务包的架构红线。
- **页面提供「全部允许」一键放宽组合**：等同诱导静默 Allow；五档 mode 逐项显式选择即为受控边界。

## 后果与实施切片

- Accepted 后按 SET-6b 写入集推进：① `crates/protocol` 新变体 + registry 登记（`since = V1_6`）+ API 1.6 + golden/typegen 先行；② `crates/app` `ApprovalService` 运行时 setter + 三 handler（查询 / `set_approval_mode` / `workspace_trust`）；③ `apps/desktop` 权限与审批页（五档选择、会话信任开关、Global 默认只读行、生效边界文案）。
- 同批回写 `docs/architecture.md` §3.2/§5、`docs/spec/contracts.md`（CON-GUI-01 版本与 CON-REGISTRY-01 计数）、实际写入集的包级 Spec。
- 不改动 config schema、Policy 决策链、事件信封、capability 基线、headless/ACP 通道与其余冻结契约；不新增 crate 与生产依赖。
