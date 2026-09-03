# Settings 活动线任务书

> 状态：SET-0～SET-5 已审查提交（SET-5：模型发现/固定回退/默认项，审查修复四处）；真实账号验收待凭证。2026-09-01 重置时仓库内不存在旧 `plan/` 文档；本文件是新建后的唯一活动任务书。范围依据 [Settings Feature Spec](../docs/spec/settings.md)，顺序与状态以 [ROADMAP](../ROADMAP.md) 为准。

## 1. 开工合同

### 目标

在现有纯 Rust / Host-owned 架构内完成 Desktop Settings。第一条闭环覆盖 Z.AI/GLM、Kimi、DeepSeek、xAI/Grok 的连接认证、模型发现/固定回退和默认 provider/model；之后再逐页增加其它设置。

### 非目标

- 发布、安装器、自更新、License、签名、公证、SBOM、供应链或三平台发布矩阵；
- 同 Provider 多账户、额度/缓存路由、团队凭证、任意自定义 OpenAI-compatible provider；
- 新 crate、新 Secret 后端、Desktop 直连 Provider/config/auth/DB；
- 一次性铺满所有 Settings 页、假按钮、假模型、假 quota；
- 为未来场景补大套抽象或测试体系。

### 验收标准

1. gear → Settings → 返回工作台不改变当前会话、草稿、Inspector 或 Run。
2. Host descriptor 驱动四家认证方式；API key/OAuth 各自闭环，错误与取消可恢复。
3. 模型目录显示远端/固定回退/不可用来源，只展示 adapter 可运行模型。
4. 默认 provider/model 由 Host 持久化并跨重启恢复，失效不静默跨供应商切换。
5. Secret 只落 auth backend；协议、ledger、DB、事件、日志、诊断、fixture 均无明文。
6. Desktop 可见、键盘与 AX 路径同 gate，1440×1024 和 1080×720 主操作可达。
7. 自动验证、真实账号验证、人工视觉验收和发布状态分开记录。

### 不改动范围

- Agent Engine、Tools、Git、PTY、会话事件与 SQLite schema；
- 当前 Changes/Terminal/Resources 行为；
- 包布局、依赖红线和非 Settings 候选池；
- 未在当前切片写入集中点名的文件。

## 2. 已核实基线

| 事实 | 影响 |
| --- | --- |
| `ModelList { provider_id }` 已对 GUI 开放；AppCore 已做运行期 probe + 静态回退 | 复用目录主干，不另建 catalog service |
| `AuthStart` / `AuthRemove` 存在但 GUI unavailable；无 auth status/API-key-set GUI contract | 先做 ADR/golden，不直接在 Desktop 绕行 |
| registry 当前六通道，Kimi 缺失；xAI adapter OAuth-only 且固定模型 | 只补首批真实缺口，不重写全部 provider 体系 |
| config 已有 default provider/model，`ProviderConfig` 不含 Secret | 默认项复用现有层级，凭证继续只进 auth backend |
| Desktop 底部只有 `Local`，无 Settings route | Settings 壳是独立切片；不与 provider adapter 混改 |
| GUI wire/config/Secret 语义为冻结契约 | ADR-046 Accepted 是生产改动硬前置 |

## 3. 切片顺序

### SET-0 — 文档立项 🟢

**写入集**：`ROADMAP.md`、`plan/settings.md`、`docs/spec/settings.md`、相关产品/GUI 文档。

**完成条件**：

- 活动路线图只保留 Settings 与后续设置顺序；
- Feature Spec 固化供应商矩阵、目录规则、安全边界、IA 与验证；
- 旧阶段从活动计划移除，历史仍由 git / `docs/history.md` 可追溯；
- 发布明确排除。

**验证**：Markdown 链接/状态/外部依据、`git diff --check`；纯文档不跑 Cargo。

### SET-1 — ADR-046 与协议 golden 🟢

> 2026-09-01 完成：ADR-046 Accepted（用户确认初始未发布版本不采取兼容策略）；protocol 落地 `ApiKeySecret`、`AuthSetApiKey`/`AuthCancel`/`SetDefaultModel` 命令、`ProviderAuthStatus` 查询、`AuthChanged` 六态状态机，registry 登记 since=V1_4，`auth_start`/`auth_remove` 开放 GUI，API 1.4，golden 34→43 帧，typegen 同步。生产 handler 属 SET-2，本切片未触碰。

**目标**：锁定 Host-driven Settings contract，尤其是 Secret 不可重放路径。

**写入集**：

- `docs/adr/ADR-046-*.md`、`docs/architecture.md`、`docs/spec/contracts.md`；
- `crates/protocol/` 与 `schemas/` 的类型、registry、fixture/golden/typegen；
- 仅为 contract harness 必需时触及 `crates/client/`、`crates/app/`；
- 同批更新 `docs/spec/crates/{protocol,client,app}.md`。

**必须拍板**：

1. provider/auth descriptor 与 status 的最小 query/response；
2. API-key secret 如何瞬时过 wire，且不进入 command ledger/replay/event/diagnostic；
3. OAuth start/progress/finish/cancel 的请求与状态；
4. 默认 provider/model 的 Host mutation 与确认；
5. API minor、旧客户端行为和 capability gate。

**停止条件**：ADR 未获用户 Accepted 时，停在 ADR + 预期 golden，不写生产 handler。

**验证**：`cargo test -p pawork-protocol --offline --lib --tests`；需要 typegen 时使用该包现有 typegen 定向命令。单次只运行一个 Cargo 进程。

### SET-2 — Host Settings 门面 🟢

> 2026-09-02 完成：`gui_host/handlers/settings.rs` 六入口（provider_auth_status / auth_set_api_key / auth_start / auth_cancel / auth_remove / set_default_model）；providers 增 `ChannelPreset.display_name`/`auth_methods()` 与 `verify_api_key`（GET /models 写前验证）；workspace 增 Global 层 `write_default_model_pair` 原子写回；app 增按 provider 单飞守卫与 `AuthChanged` 广播。新增测试 2 条（验证成功替换主路径 + 401 失败保旧且无明文）。审查修正两处：providers lib.rs 的 `verify_api_key` re-export 补 feature 门（默认 features 下单包可编译）、`publish_provider_auth` 的 Global 流 `stream_sequence` 归一到既有置 0 约定。Kimi 通道与 xAI API-key adapter 按任务书属 SET-4，未动。

**目标**：用现有服务形成唯一的 Settings 读写门面。

**写入集**：

- `crates/app/`：descriptor/status/query/command handler 与装配；
- `crates/auth/`：仅补非重放写入、验证、移除/替换所需最小 API；
- `crates/providers/`：descriptor/auth-method 元数据与验证入口；
- `crates/workspace/`：仅当现有 default provider/model 无安全写入入口时补最小 writer；
- 对应 `docs/spec/crates/{app,auth,providers,workspace}.md`。

**完成条件**：

- CLI 已有凭证在 GUI status 中脱敏可见；
- 同一 provider 同时最多一个 auth/refresh 操作；
- 新 key 验证成功后原子替换，失败保留旧值；
- default provider/model 按现有配置优先级落盘并可重启读取；
- 未知 provider/method、断线和权限缺失 fail-closed。

**验证**：一个 Cargo 进程执行
`cargo test -p pawork-auth -p pawork-providers -p pawork-workspace -p pawork-app --offline --lib --tests`。

**定向回归上限**：主路径一条；另加 Secret 不落 ledger/日志或替换失败保旧中的一个关键失败路径。现有测试可覆盖时不新增。

### SET-3 — Settings 壳与只读供应商页 🟢

> 2026-09-02 完成：TaskRail `Local` 行 gear（可见/键盘/AX 同 gate）+ AppRoute 顶层路由（Settings 与工作台互斥渲染，工作台状态全保留）+ Settings Rail（返回 + 唯一真实导航项）+ 只读供应商页（provider_auth_status 全量查询，auth 四态/catalog 三态，断线 stale 标注）。审查修复六处：Settings 路由下九个工作台快捷键加 route 守卫（防隐形取消 Run/审批）、AX 状态行与 render 改同源逐行发布、stale 语义（加载清 stale/迟到响应重标）、auth_methods 解析 fail-closed；2026-09-02 原任务线审查提交 main 时再修两处：ui/accessibility/app.rs 文档注释归位（SET-3 函数误插到 project_ax_nodes 的 doc 与 fn 之间，“Settings 左栏”与“项目块 AX 投影”各归其位）、on_connected 重连后调用 refresh_provider_status() 清除断线 stale 标注。新增测试 3 条（解析主路径、畸形载荷 fail-closed、路由快捷键守卫）。已知缺口登记 SET-7：Settings 页 AX 卡片几何为固定估值、不随滚动，真窗口 VoiceOver 验收时重点核。

**目标**：先接真实只读状态，再开放写操作。

**写入集**：

- `apps/desktop/src/` 的 route/state/controller/UI/AX 最小文件集；
- `docs/gui-design.md`、`docs/spec/desktop.md`、`docs/spec/crates/desktop.md`；
- 除必要 client consumer 外不改 Host。

**完成条件**：

- `Local` 行 gear、Settings Rail、返回工作台与全宽内容区落地；
- 只显示 Host 返回的供应商、认证方法和状态；
- 无能力时隐藏写入口；断线保留 stale 只读状态；
- 进入/退出 Settings 不改变会话、草稿、Inspector、Timeline 或 Run；
- keyboard/AX 与 visible handler 同 gate。

**验证**：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`；随后正式 Host/Desktop 真窗口检查进入、返回、断线、1440/1080。

### SET-4 — 四家认证闭环 🟢

> 2026-09-02 完成：registry 扩为八通道（末尾追加 kimi-platform API-key 行与 kimi-code Device OAuth 行；端点经 MoonshotAI/kimi-cli 源码与官方文档 web 核对一致）；`auth_methods` 从 kind 派生改为 `ChannelPreset` 数据字段，xAI 声明 ["oauth","api_key"] 双认证；新增 `ChannelKind::KimiOAuth` 与 channels/kimi.rs（OAuth-only、固定 Chat Completions、版本固定 builtin 目录）；xai adapter 接受 ApiKey 凭证；替换语义双向落地（set-api-key 成功删旧 OAuth、oauth_finish 成功删旧 api key，删除失败 fail-closed 上报）；auth_remove env 命中不阻断已存条目清理。Desktop 供应商页写操作落地：API key secure 内联输入（掩码渲染、AX value 无明文、Copy/Cut no-op）、OAuth 等待（URL/user code/到期/Cancel）、Replace/Remove、AuthChanged 六态消费与 Succeeded 后状态重查，全部 descriptor 驱动无品牌分支，stale/断线三路径同 gate 禁写。审查（glm_reviewer ×2）修复七处：Host 侧 auth flight 加种类标记（api-key 验证拒绝 AuthCancel，防 Cancelled/Succeeded 双发）、providers spec §7 五 feature 回写、auth_remove env 语义恢复；Desktop 侧 Replace 终态对已连接 provider 改触发权威重查而非断言未连接、空输入 Verify gate 发布到 AX、焦点句柄精确回收、verify 命令 socket 失败对称重查。2026-09-02 推送前复查再修三处：Removed 置 pending_status_refresh（目录与 env 残留交权威重查）、空输入 Verify tooltip 仅在禁用时出现且入口复核 settings_action_enabled、secure 输入 AX/IME 同步剔除 CR/LF。新增定向测试 13 条（providers 5：kimi 凭证门与 builtin 3、xai 双凭证接受 1、kimi-code 端点 1；app 3：xai set-key 主路径/替换删旧 OAuth/取消 api-key flight 被拒；desktop 5：AuthChanged 六态解析应用/畸形 fail-closed/secure 掩码/AX 掩码+stale gate/Replace 终态重查）。真实账号端到端验收缺凭证，登记 SET-7。

**目标**：按 Host descriptor 完成认证，不在 Desktop 写 vendor switch。

**写入集**：`crates/providers/`、`crates/auth/`、`crates/app/`、`apps/desktop/` 及实际涉及包 Spec；协议只实现 SET-1 已接受形状。

**顺序**：

1. Z.AI/GLM Coding Plan API key（复用现有 adapter）；
2. DeepSeek API key（复用现有 adapter）；
3. Kimi Platform API key + Kimi Code OAuth（新增通道/refresh）；
4. xAI 复用现有 OAuth，并补 API-key adapter。

**完成条件**：每种方法具备开始、验证、成功、错误、取消、替换和移除；认证成功与目录状态分离；provider error 脱敏。

**验证**：先运行
`cargo test -p pawork-auth -p pawork-providers -p pawork-app --offline --lib --tests`，
再运行 `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`；同一时刻仅一个 Cargo 进程。真实账号验收不得把 token 写进脚本、日志或 fixture。

### SET-5 — 模型发现、固定回退与默认项 🟢

> 2026-09-02 完成：xAI `list_models` 改走远端 `GET {base}/language-models`（OAuth bearer 与 API key 均可，只保留 output_modalities 含 text 的可运行模型；修正旧版有凭证即误标 remote）；Kimi Code 经取证（官方 kimi-cli 使用 `https://api.kimi.com/coding/v1/models`，SET-D08 关闭为 Accepted）走远端 `/models`，OpenAI 风格 data[] 解析；两者已知 ID 沿用内置元数据、未知 ID 只给保守默认，形状不符/失败/无凭证一律 Err 由 Host 落 fixed_fallback。Z.AI Coding Plan 维持现状（`/models` 端点有官方文档迹象，探测失败自动固定回退）。`provider_auth_status` 顶层透出持久化默认项（生效配置 default pair 或 null）；`set_default_model` 写盘成功后同步内存配置，同会话重查即新默认。Desktop 新增「模型与默认项」区（按 provider 分组、Default 徽标、Set default、页级 Refresh，可见/键盘/AX/入口同 gate），Host 确认后同步 Composer selected_model，失效默认显式提示（目录空抑制误报、未连接/目录确缺才提示），无静默切换。审查（glm_reviewer）修复三处：set_default_model 内存配置不同步（P1）、空目录误报默认失效（P2）、kimi 失败路径测试缺失且 Spec 虚报（P2）。提交前复查再修：同会话 set_default_model 测试用 Drop 守卫恢复 HOME。新增定向测试 8 条（providers 3：xai 解析+过滤/远端失败 Err/kimi 形状不符 Err，另 kimi 解析主路径改造 1；app 3：default 字段有无 2 + set-重查串联 1；desktop 5：default 解析/畸形 fail-closed/确认同步/失效标志/刷新失败保留）。glm-coding 等通用 API-key 通道远端探测为既有行为未改。真实 API 与真窗口验收登记 SET-7。

**目标**：从连接到 Composer 形成可解释目录闭环。

**写入集**：`crates/providers/`、`crates/app/`、必要的 `crates/protocol/client/`、`crates/workspace/`、`apps/desktop/` 及对应 Spec。

**规则**：

- Kimi Platform、DeepSeek、xAI API key 优先请求官方 models endpoint；
- OAuth/供应商没有稳定目录 contract 时使用版本固定的官方/Models.dev 参考目录；
- 远端 ID 与固定 metadata 保守合并，未知能力不推断；
- 过滤当前 adapter 无法运行的模型；
- 刷新失败保留已有列表并标来源/错误；首期无持久缓存/后台轮询；
- 默认项只在 Host 确认后更新 Composer，失效时显式提示。

**完成条件**：四家均得到远端目录或诚实固定回退；模型来源/刷新时间/错误可见；Host 重启后默认项恢复。

**验证**：provider/app/workspace/desktop 受影响包定向测试；四家真实 API 或明确记录为何只能固定回退；Host/Desktop 重启复验。

### SET-6 — 其它 Settings 页 ⚪

SET-5 收口后才逐页立项，不预建通用设置框架：

| 顺序 | 页面 | 最小真实能力 | 明确不做 |
| --- | --- | --- | --- |
| 1 | 通用 | Host 已有、Global 层可持久化的通用配置（SET-6a 首期 = `proxy_url`） | 无来源的偏好大全 |
| 2 | 权限与审批 | 当前 approval mode/trust 的读取与受控修改 | 绕过 Policy、静默 Allow |
| 3 | 工具与 MCP | Host 权威 MCP list/test/config mutation | 假工具市场 |
| 4 | 终端 | 有明确宿主持久化语义的 shell/cwd/尺寸默认值 | Desktop 直写 PTY 配置 |
| 5 | 外观 | 本地 theme/字号等 presentation preference | 未实现的主题生态 |
| 6 | 高级 | 已有诊断/实例配置的安全入口 | 杂项垃圾桶 |
| 7 | 关于 | 构建版本、协议版本、数据目录的只读信息 | updater/release 宣称 |

每页激活时补一页小任务或在本文件增加独立切片；涉及 wire/config/Policy 时重复 ADR 判定。未激活的页不显示。

### SET-6a — 通用页（proxy_url）🟢

> 2026-09-02 完成：protocol 落地 `GeneralSettings` 查询与 `SetProxyUrl` 命令（`proxy_url` 必填字段——deserialize_with 取消 Option 隐式默认，缺字段解码错误、显式 null 清除）、registry 登记 since=V1_5 仅 GUI、API 1.5、golden 43→48 帧（含清除帧与清除回执）+ typegen 三产物；workspace 增 `write_proxy_url`（Some 覆盖 / None 移除键、未知字段保留、原子写回）；app 两 handler 按 ADR-047 D2 定序（校验=预构目标 client → Global 原子写 → 写锁内直接赋值+换入），`invalid_proxy_url` 归 ValidationFailed 且不回显原文 URL；desktop「通用」页（查询成功才显示导航；null 文案「未设置（跟随系统环境变量）」；生效边界文案；stale 禁写三路径同 gate；重连自动刷新）。审查（grok_reviewer ×2）修复：ADR 期 2 P1（生效面拆分、清除 wire 语义）+2 P2；实现期 4 P2（冻结契约回写、重连不刷新 stale 残留、错误码映射、Global 写 RMW 加进程锁）+P3 文案对齐三处。已知残留（不挡收口）：proxy 输入框 stale 时仅视觉禁用，仍可获得焦点编辑草稿（Save/Clear/AX 均 fail-closed 不可持久化）。提交前复查再修：选中导航项 track_focus/AX focused、Save 失败文案与 load 分开、HOME 锁毒化 into_inner、workspace Spec 补记进程写锁。protocol 150 / workspace 119+13+15 / app 180+6+15+2 / desktop 176 全绿。真实窗口验收登记 SET-7。

> 2026-09-02 立项：通用页最小真实能力锁定为 `proxy_url` 的读取、设置与清除（Global 层）。经主代理源码盘点与独立只读调研双确认：`PaworkConfig` 顶层键中 `default_*` 属「模型与供应商」（SET-5 已管）、`trust_workspaces` 属「权限与审批」、`mcp` 段属「工具与 MCP」、`profile` 与默认 provider/model 有双轨写风险排除；`proxy_url` 是唯一剩余 Host 已有、Global 层可持久化且有真实运行时语义的通用键。wire 演进走 [ADR-047](../docs/adr/ADR-047-general-settings-wire.md)（2026-09-02 用户确认 Accepted）。grok_reviewer 审查后修订：钉死「校验=完整构造目标客户端、先于写盘」以消除写后重建分叉；wire 上 `proxy_url` 字段必填、显式 `null` 才清除、缺字段即解码错误；生效边界拆分（OAuth/探测同会话生效，模型流量随下次装配生效）并写入页面文案；错误统一 `invalid_proxy_url` 不回显原文 URL。

**写入集**：

- `docs/adr/ADR-047-*.md`、`docs/architecture.md`、`docs/spec/contracts.md`；
- `crates/protocol/`：`GeneralSettings` 查询 + `SetProxyUrl` 命令 + registry 登记 + API 1.5 + golden/typegen；
- `crates/workspace/`：`write_proxy_url` 原子写回（保留未知字段，`None` 移除该键）；
- `crates/app/`：两个 handler + 校验期预构目标客户端、写盘后内存赋值并换入新 `core.http`（与 ADR-047 D2 同序，禁止写后再新建）；
- `apps/desktop/`：Settings 导航增「通用」页 + proxy 行读/设置/清除，可见/键盘/AX 同 gate，断线 fail-closed；
- 实际涉及包的包级 Spec。

**完成条件**：

- 通用页显示 Host 权威 `proxy_url` 生效值（=Global 持久值或 null）；未设置时诚实显示跟随系统环境变量；
- 设置经 Host 先行校验（与运行时同一构造路径，校验期完成客户端构造；非法 fail-closed 保旧值）、Global 层原子写盘、写后直接赋值同步内存并换入新 `core.http`；
- 页面文案诚实标注生效边界：新 OAuth/验证/目录探测同会话生效，当前活跃供应商连接的模型流量于下次装配（切换供应商/重启 Host）后生效；
- 断线 stale 只读，写动作 fail-closed；可见/键盘/AX 同 gate；
- golden/typegen 先行；无 Settings 时 CLI 与 GUI 读同一 Global 配置保持一致。

**停止条件**：ADR-047 已 Accepted（2026-09-02）；实施中冲突先收敛原因，不扩大写入集。

**验证**：`cargo test -p pawork-protocol --features typegen --offline --lib --tests` + `cargo test -p pawork-workspace -p pawork-app --offline --lib --tests` + `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`；单一 Cargo 进程纪律不变。

**定向回归上限**：主路径两条（设置 → 重查一致；清除 → 重查 null，同属一个 mutation 的两半）；关键失败路径一条（非法 URL fail-closed 保旧）。现有测试可覆盖时不新增。

### SET-6b — 权限与审批页（approval mode + workspace trust）🟢

> 2026-09-03 推送前复查再修：`set_approval_mode`/`workspace_trust` 补 ToolScheduler Arc-swap（否则之后 run 仍走启动时 ReadOnly/untrusted 闸门）、AX Press 补 `settings-nav-permissions`、信任开关 enable 改用 Host `workspace_id`（不再用 snapshot 注册表首项）。
>
> 2026-09-03 完成：ADR-048 Accepted 后三切片串行落地（glm_worker：protocol → app → desktop）。protocol 增 `PermissionsSettings` 查询 / `SetApprovalMode` 命令（since=V1_6、仅 GUI）+ `WorkspaceTrust` 死词汇开放 GUI（Approvals capability）+ API 1.6 + golden 48→55 帧 + typegen；app `ApprovalService` 运行时 setter + 三 handler（校验写入同锁）；desktop 权限与审批页（五档显式选择、会话信任开关、Global 默认只读行、生效边界文案、回执才生效、stale 三路径同禁、可见/键盘/AX 同 gate）。审查（glm_reviewer）修复四处：P2 冻结契约回写遗漏、P2 ROADMAP/plan 状态矛盾、P2 `set_approval_mode` 别名越契约（改严格五值 `approval_mode_from_wire`）、P3 信任开关 workspace_id 误取注册表首项——ADR-048 D1 实现期修订增补响应 `workspace_id`（Host 权威 attached id），golden 两响应帧同批钉死。protocol（含 55 帧 golden + typegen --check）/ app 206 / desktop 179 全绿。真窗口验收登记 SET-7。

> 2026-09-02 立项：最小真实能力锁定为「当前 approval mode 与 workspace trust 的读取 + 会话内受控修改」。经主代理源码实读与两路 glm_explorer 调研三方确认：`ApprovalMode` 五档仅 CLI 启动参数注入、无持久化无运行时查询/修改 API（GUI 用户不传参即永远 ReadOnly）；`workspace_trusted` 为内存态，Global 层 `trust_workspaces` 可持久化但 writer 无写函数；wire 上 `WorkspaceTrust` 自 R3 登记即无 handler（死词汇），全协议无 approval mode query/command；run 启动时快照 mode/trusted，进行中 run 不受影响（既有架构，作为诚实生效边界）。wire 演进走 [ADR-048](../docs/adr/ADR-048-permissions-settings-wire.md)（2026-09-02 用户确认 Accepted）。

**目标**：GUI 用户可查看当前审批模式与信任状态，并在会话内显式切换；所有变更不持久化、重启回默认（fail-closed 安全语义）。

**非目标**：不持久化 approval_mode；不写 Global `trust_workspaces`（只读展示，写留候选）；不改 Policy 决策链与进行中 run；不新增 `PermissionsChanged` 事件；不做「一键全允许」。

**写入集**（ADR-048 Accepted 后才动生产代码）：

- `docs/adr/ADR-048-*.md`、`docs/architecture.md`、`docs/spec/contracts.md`；
- `crates/protocol/`：`PermissionsSettings` 查询 + `SetApprovalMode` 命令 + `WorkspaceTrust` 开放 GUI + registry（since=V1_6）+ API 1.6 + golden/typegen；
- `crates/app/`：`ApprovalService` 运行时 setter（set_mode / set_workspace_trusted，`configure` 保持启动专用）+ 三 handler；
- `apps/desktop/`：Settings 导航增「权限与审批」页（五档选择、会话信任开关、Global 默认只读行、生效边界文案），可见/键盘/AX 同 gate，断线 fail-closed；
- 实际涉及包的包级 Spec。

**完成条件**：

- 页面显示 Host 权威三元组（当前 mode / 会话 trusted / Global 持久默认），来源语义不混淆；
- `SetApprovalMode` 会话内生效，之后启动的 run 用新 mode，进行中 run 不变；未知值 fail-closed 保旧；
- `WorkspaceTrust` 校验 workspace_id 匹配当前 attach，不匹配 Error 保旧；切换只影响之后启动的 run；
- 页面文案诚实标注：不持久化、重启回默认、进行中 Run 不受影响；
- 断线 stale 只读、写动作 fail-closed；可见/键盘/AX 同 gate；
- golden/typegen 先行；不新增 crate、依赖、schema 键。

**停止条件**：ADR-048 未获用户 Accepted 时，停在 ADR + 预期 golden 描述，不写生产 handler。

**验证**：`cargo test -p pawork-protocol --features typegen --offline --lib --tests` + `cargo test -p pawork-app --offline --lib --tests` + `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`；单一 Cargo 进程纪律不变。

**定向回归上限**：主路径两条（set mode → 重查一致；workspace_trust 匹配 id → 之后 run 生效）；关键失败路径一条（workspace_id 不匹配 fail-closed）。现有测试可覆盖时不新增。

### SET-6c — 工具与 MCP 页（MCP list/test/remove）🟢

> 2026-09-03 完成：ADR-049 Accepted 后三切片串行落地（glm_worker：protocol → workspace/app → desktop）。protocol 增 `McpTest` / `McpServerRemove`（since=V1_7、仅 GUI、非幂等）+ API 1.7 + golden 4 帧（回执形状复用 mcp_list 金样）+ typegen 三产物，35 个既有帧仅 api_version 6→7 机械重写；workspace 增 `write_mcp_server_remove`（Global 层 RMW+进程锁+原子写，缺失键 Ok(false) 不写盘）；app 两 handler（remove 定序「合并配置校验存在 + 跨层同名守卫 → Global 原子写 → pawork.mcp.* SecretRef 幂等清理 → 内存同步 shutdown slot+删 slot+重建 registry」，同会话生效）；desktop「工具与 MCP」页（复用 mcp_list 数据链与 ResourcesPanelState，每行 Test/Remove 两步确认，回执即权威生效值，stale 三路径同 gate，可见/键盘/AX 同 gate）。审查（glm_reviewer）修复 1 P1（Spec/契约回写，主代理收口）+3 P2（跨层同名守卫、mcp_test 回归、Secret 命名空间 fail-closed 测试）+P3。protocol 152 / workspace 119+13+15 / app 189 / desktop 181 全绿。真窗口验收登记 SET-7。

> 2026-09-03 立项：最小真实能力锁定为「Host 权威 MCP list/test/remove」。经主代理源码实读与两路 glm_explorer 独立只读核查三方确认：list 复用既有 mcp_list（V1_0 GUI 可用，Resources 页已消费，数据链零协议改动可复用；test 复用 Host 实装 mcp_test（CLI 已消费，GUI 无词汇；config mutation 首片取 remove——Host 零写路径，需 workspace writer 新增 write_mcp_server_remove（Global 层 RMW+原子写 + SecretRef 清理 + 内存同步（shutdown slot + 重建 registry）；add 不入本片（Secret 传输 + 新写封装属独立安全切片。wire 演进走 [ADR-049](../docs/adr/ADR-049-mcp-settings-wire.md)（2026-09-03 用户确认 Accepted）。

**目标**：GUI 用户可在 Settings 内查看 Host 权威 MCP server 清单（name/transport/state/tools/last_error），可对单个 server 执行 test，可从 Global 配置移除 server（含 SecretRef 清理，全部写路径经 Host。

**非目标**：不做 server add（登记候选）；不切换 trusted/auto_start；不扩展 mcp_list 响应加 endpoint；不做 enable/disable 概念；不动 Policy/MCP 装配语义。

**写入集**（ADR-049 Accepted 后才动生产代码）：

- docs/adr/ADR-049-*.md、docs/architecture.md、docs/spec/contracts.md；
- crates/protocol/：McpTest + McpServerRemove 命令 + registry 登记（since=V1_7 + API 1.7 + golden/typegen + 仅 GUI 通道保守；
- crates/workspace/：write_mcp_server_remove 原子写回（Global 层 RMW + 进程锁 + 原子写，保留未知字段；
- crates/app/：两 handler（mcp_test 复用 AppCore::mcp_test + McpServerRemove 按 ADR-049 D2 定序；
- apps/desktop/：Settings 导航增「工具与 MCP」页（复用 Resources 数据链，Test/Remove + stale 三路径同 gate，可见/键盘/AX 同 gate；
- 实际涉及包的包级 Spec + docs/spec/settings.md 页启用。

**完成条件**：

- 页面显示 Host 权威 MCP 清单与状态（复用 mcp_list，来源语义不混淆；
- test 单 server 现场验证并回写状态，未知 server fail-closed；
- remove 后盘/密/内存三处一致，重查不含，进行中 run 已快照工具不回溯撤销（文案诚实标注；
- 断线 stale 只读、写动作 fail-closed；可见/键盘/AX 同 gate；
- golden/typegen 先行；不新增 crate、依赖、schema 键。

**停止条件**：ADR-049 未获用户 Accepted 时，停在 ADR + 预期 golden 描述，不写生产 handler。

**验证**：`cargo test -p pawork-protocol --features typegen --offline --lib --tests` + `cargo test -p pawork-workspace -p pawork-app --offline --lib --tests` + `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`；单一 Cargo 进程纪律不变。

**定向回归上限**：主路径两条（mcp_test → 状态回写且重查一致 + remove → 三处一致重查不含；关键失败路径一条（未知 name 三处皆不动 fail-closed。现有测试可覆盖时不新增。

### SET-6d — 终端页（shell/columns/rows 默认值）🟢

> 2026-09-03 完成：ADR-050 Accepted 后三切片串行落地（glm_worker：protocol → workspace/app → desktop）。protocol 增 `TerminalSettings` 查询 / `SetTerminalSettings` 全态写命令（三字段必填、shell=null 显式清除、since=V1_8、仅 GUI、idempotent）+ API 1.8 + golden 5 新帧（39 既有帧仅 api_version 7→8 机械重写）+ typegen；workspace schema 增 `terminal: Option<TerminalConfig>` + `write_terminal_settings`（Global RMW+进程锁+原子写、shell=None 移除键）+ `strip_untrusted_layer` 整段剥离 `terminal`（ConfigWarning::TerminalIgnored）；app 两 handler（校验 fail-closed：shell trim 非空、含分隔符须存在、否则 PATH 可解析、columns/rows ∈ 2..=1000；定序校验→写盘→内存同步）+ terminal_create 应用配置 shell/size（策略闸 classification_shell 自动跟随）；desktop「终端」页（生效值展示、shell null 文案「未设置（跟随平台默认）」、三输入全态写 Save/Clear、生效边界文案、stale 三路径同 gate、可见/键盘/AX 同 gate、重连预热查询缓存）+ 新建终端初始尺寸取生效值。审查（glm_reviewer）修复 2 P2（冻结契约回写：architecture §3.2 API 1.8/64 帧 + ADR 索引 + CON-GUI-01/CON-REGISTRY-01 28/15；Disconnected 分支补 settings_terminal mark_stale）+ P3 状态矛盾同批收口。提交前复查再修：Save 空 shell 映射为 null（可只改尺寸）；protocol Spec 模块树 27/14 → 28/15。已知残留（不挡收口，符合 ADR 字面）：连接后 terminal_settings 查询在途窗口内新建终端仍按 80×24 resize 覆盖配置默认（skip resize 会致投影/PTY 尺寸错配渲染损坏，留待 TerminalCreate 响应携带实际尺寸的 wire 演进）；shell 校验 exists() 不排目录、Windows 无 PATHEXT 解析。protocol 154 / workspace 121+13+15 / app 192+6+15+2 / desktop 185 全绿；`cargo check -p pawork --offline` 通过。真窗口验收登记 SET-7。

> 2026-09-03 立项：最小真实能力锁定为「Global 层 `[terminal]` 段（shell/columns/rows）的读取与全态写 + terminal_create 应用配置默认 + Desktop 初始尺寸取生效值」。经主代理源码实读与两路 glm_explorer 独立只读核查三方确认：`PaworkConfig` 无终端键；`TerminalCreate` wire 无 shell/size；terminal_create 恒用 `PtyCreateSpec::default()`（shell=None 走 exec 兜底链 $SHELL//bin/sh/cmd.exe，size 恒 80×24）；resize 只作用会话无持久化；**Desktop 新建终端后立即按 80×24 下发一次 resize，会压掉宿主配置默认，必须同批处理**；Workspace 层若可设 shell 即仓库投毒任意命令执行，须整段剥离（同 trusted/auto_start 先例）。cwd 默认值属 per-workspace 语义、workspace 包无 Workspace 层写盘代码，不入本片、登记候选。wire/config 演进走 [ADR-050](../docs/adr/ADR-050-terminal-settings-wire.md)（2026-09-03 起草，**待用户 Accepted**）。

**目标**：GUI 用户可查看并设置终端默认 shell 与初始尺寸（Global 持久化）；之后创建的终端使用配置默认；已存在终端不回溯。

**非目标**：不做 cwd 默认值（登记候选）；不允许 Workspace 层 `[terminal]`（整段剥离）；不做部分字段 patch（全态写）；不做 resize 持久化、像素尺寸、shell args、env 配置；Desktop 不直写 PTY 配置。

**写入集**（ADR-050 Accepted 后才动生产代码）：

- `docs/adr/ADR-050-*.md`、`docs/architecture.md`、`docs/spec/contracts.md`；
- `crates/protocol/`：`TerminalSettings` 查询 + `SetTerminalSettings` 命令（三字段必填全态写，shell=null 清除）+ registry（since=V1_8、仅 GUI、idempotent）+ API 1.8 + golden/typegen；
- `crates/workspace/`：schema 增 `terminal: Option<TerminalConfig>`；`write_terminal_settings` Global RMW+原子写；`strip_untrusted_layer` 追加剥离 `terminal` 键 + ConfigWarning；
- `crates/app/`：两 handler（校验 fail-closed：shell 存在性/PATH 可解析，columns/rows ∈ 2..=1000；定序校验→写盘→内存同步）；`terminal_create` 应用配置 shell/size；
- `apps/desktop/`：Settings 导航增「终端」页（shell 输入、columns/rows 输入、Save，生效边界文案「只影响之后创建的终端」），新建终端初始尺寸改用生效值；可见/键盘/AX 同 gate，断线 fail-closed；
- 实际涉及包的包级 Spec + `docs/spec/settings.md` 页启用。

**完成条件**：

- 页面显示 Host 权威生效值（shell null=跟随平台默认；columns/rows 未设=80/24），来源语义不混淆；
- set 全态写经校验、Global 原子写、同会话重查一致、重启恢复；非法 shell/越界尺寸 fail-closed 保旧；
- 非 Global 层 `[terminal]` 被剥离并如实告警；
- 之后创建的终端 shell/size 与配置一致（策略闸分类自动跟随）；Desktop 新建初始尺寸不再硬编码 80×24；
- 断线 stale 只读、写动作 fail-closed；可见/键盘/AX 同 gate；
- golden/typegen 先行；不新增 crate、依赖。

**停止条件**：ADR-050 未获用户 Accepted 时，停在 ADR + 预期 golden 描述，不写生产 handler。

**验证**：`cargo test -p pawork-protocol --features typegen --offline --lib --tests` + `cargo test -p pawork-workspace -p pawork-app --offline --lib --tests` + `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`；单一 Cargo 进程纪律不变。

**定向回归上限**：主路径两条（set → 重查一致；terminal_create 应用配置 shell/size）；关键失败路径一条（非法值 fail-closed 保旧）；安全红线定向回归一条（非 Global 层 `[terminal]` 剥离）。现有测试可覆盖时不新增。

### SET-7 — 真窗口与人工收口 ⚪

**自动证据**：实际写入集定向门禁；protocol/Secret/config 三类关键回归；`git diff --check`。

**真实证据**：

- 四家供应商的可用认证路径；Kimi/xAI OAuth 的 device flow/refresh/取消；
- remote model list 或固定回退原因；
- API key 替换失败保旧、移除、Host/Desktop 重启、断线/重连；
- `pawork auth list` / `pawork models` 与 GUI 脱敏状态一致；
- 1440×1024、1080×720、100/125/150%、键盘/AX。
- SET-3 登记的已知缺口：Settings 页 AX 卡片几何为固定估值、不随滚动位移；1080×720 折叠线以下卡片 AX rect 与视觉错位，VoiceOver 走查时重点核，必要时修。

**人工签字**：视觉层级、secure input、OAuth 浏览器切换、VoiceOver。没有用户签字时只写“等待人工验收”。

## 4. 每片收尾清单

- [ ] 当前 diff 与写入集一致，无用户改动被覆盖。
- [ ] 只读过实际写入包的包级 Spec，源码冲突已同批回写。
- [ ] 未引入未证明必要的抽象、依赖、配置层或测试。
- [ ] 定向测试服务于本片已接受行为；无额外测试体系。
- [ ] Secret、wire、config、Desktop honesty 对应回归已跑或明确说明 none。
- [ ] ROADMAP、Feature Spec、产品 Spec、包级 Spec 和 history 状态一致。
- [ ] 已实现、自动验证、真实环境、人工验收分别记录。
- [ ] `git diff --check` 通过；Full workspace gate 保持 NOT RUN。

## 5. 阻塞与升级条件

- ADR-046 未 Accepted：不改生产 wire/handler。
- 官方认证或模型目录文档与实现冲突：以真实端点/源码为准，先回写本 Spec，不用猜测补兼容层。
- 需要同供应商多账户、任意自定义 provider、持久模型缓存或后台刷新：停止并另行立项。
- 需要新 crate、生产依赖、schema migration、Secret 新存储或 Desktop 直连业务包：停止并请求用户确认。
- 真实凭证缺失只阻塞 E3，不得用 mock 宣称真实通过。
- 任何发布需求都退出本任务，由用户另行建立发布计划。
