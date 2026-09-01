# Settings 活动线任务书

> 状态：SET-0～SET-2 已审查提交；SET-3 Settings 壳（gear/Settings Rail/返回工作台/只读供应商页）实现完成，待原任务线另行审查提交；写操作未开放。2026-09-01 重置时仓库内不存在旧 `plan/` 文档；本文件是新建后的唯一活动任务书。范围依据 [Settings Feature Spec](../docs/spec/settings.md)，顺序与状态以 [ROADMAP](../ROADMAP.md) 为准。

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

### SET-3 — Settings 壳与只读供应商页 🔵（实现完成，待另行审查提交）

> 2026-09-02 完成：TaskRail `Local` 行 gear（可见/键盘/AX 同 gate）+ AppRoute 顶层路由（Settings 与工作台互斥渲染，工作台状态全保留）+ Settings Rail（返回 + 唯一真实导航项）+ 只读供应商页（provider_auth_status 全量查询，auth 四态/catalog 三态，断线 stale 标注）。审查修复四处：Settings 路由下九个工作台快捷键加 route 守卫（防隐形取消 Run/审批）、AX 状态行与 render 改同源逐行发布、stale 语义（加载清 stale/迟到响应重标）、auth_methods 解析 fail-closed。新增测试 3 条（解析主路径、畸形载荷 fail-closed、路由快捷键守卫）。已知缺口登记 SET-7：Settings 页 AX 卡片几何为固定估值、不随滚动，真窗口 VoiceOver 验收时重点核。

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

### SET-4 — 四家认证闭环 ⚪

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

### SET-5 — 模型发现、固定回退与默认项 ⚪

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
| 1 | 通用 | Host 已有、可持久化的默认 workspace/session 行为 | 无来源的偏好大全 |
| 2 | 权限与审批 | 当前 approval mode/trust 的读取与受控修改 | 绕过 Policy、静默 Allow |
| 3 | 工具与 MCP | Host 权威 MCP list/test/config mutation | 假工具市场 |
| 4 | 终端 | 有明确宿主持久化语义的 shell/cwd/尺寸默认值 | Desktop 直写 PTY 配置 |
| 5 | 外观 | 本地 theme/字号等 presentation preference | 未实现的主题生态 |
| 6 | 高级 | 已有诊断/实例配置的安全入口 | 杂项垃圾桶 |
| 7 | 关于 | 构建版本、协议版本、数据目录的只读信息 | updater/release 宣称 |

每页激活时补一页小任务或在本文件增加独立切片；涉及 wire/config/Policy 时重复 ADR 判定。未激活的页不显示。

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
