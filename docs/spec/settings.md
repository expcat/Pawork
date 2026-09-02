# Settings：模型与供应商

## 元数据

| 字段 | 值 |
| --- | --- |
| Feature ID / 名称 | `SETTINGS-01` / Settings 与模型供应商管理 |
| 状态 | **Accepted（产品范围已确认；ADR-046 拍板；SET-1/SET-2 已实现并通过定向测试，Desktop 与真实认证验收未开始）** |
| Owner | Pawork maintainers |
| 目标阶段 | Settings 活动线；不绑定发布版本 |
| 最近更新 | 2026-09-01 |
| 关联 | [ROADMAP](../../ROADMAP.md) · [任务书](../../plan/settings.md) · [GUI 设计](../gui-design.md) · [ADR-046](../adr/ADR-046-settings-auth-wire-and-secret-transit.md)（Accepted，2026-09-01） |

## 1. 问题、用户与目标

- **目标用户**：希望在 Desktop 内完成模型供应商连接、模型选择和日常设置，而不需要先记忆 CLI/env 细节的本机开发者。
- **当前问题**：Desktop 只有 Composer 模型选择器，没有 Settings 路由、凭证状态、添加供应商或登录流程；Provider/auth 能力散落在 CLI/AppCore，GUI 只消费模型列表。
- **用户场景/JTBD**：打开 Settings，添加一个供应商连接，以其支持的 API key 或 OAuth 登录，确认可用模型，并把其中一个设为下一轮默认模型。
- **成功指标**：首批四家供应商各至少一条真实认证路径可用；模型目录来源和回退可辨；重启后默认项恢复；Secret 泄漏种子为零；断线、取消、错误不会显示假成功。
- **非目标**：发布、安装器、自更新、同供应商多账户池、额度路由、团队 Secret、首批任意自定义端点、假 quota/假模型、Desktop 直连业务服务。

本功能是现有 CLI 能力的 Desktop 产品化，不创建第二套 Provider 或 auth 实现。

## 2. 当前状态与差距

| 能力 | 当前生产路径/证据 | 缺口 | 结论 |
| --- | --- | --- | --- |
| Settings 入口/路由 | SET-3 起 TaskRail `Local` 行 gear + AppRoute 顶层路由 + Settings Rail + 只读供应商页落地（[settings.rs](../../apps/desktop/src/ui/settings.rs)） | 其余设置页未启用（SET-6） | 已实现 |
| Provider 注册 | [channel registry](../../crates/providers/src/channels/registry.rs) 八行：chatgpt/xai/glm-coding/opencode-go/qwen-token-plan/deepseek/kimi-platform/kimi-code；SET-4 起 `auth_methods` 为数据字段，支持同供应商多认证方法 | — | 已实现 |
| API-key 通道 | [api_key.rs](../../crates/providers/src/channels/api_key.rs) 可请求 OpenAI-compatible `/models`；SET-2 增 `verify_api_key` 写前验证与 `auth_set_api_key` 非重放命令（verify-then-replace）；SET-4 起 xAI adapter 接受 API key，桌面端写操作已接通 | — | 已实现（真实账号验收 pending） |
| OAuth | AppCore/auth 已有 OAuth 基础；xAI Device Flow 已接入；SET-2 起 `AuthStart`/`AuthCancel`/`AuthRemove` 对 GUI 开放并有 handler，进度经 `AuthChanged` 六态下发；SET-4 起 Kimi Code Device Flow 接入（[kimi.rs](../../crates/providers/src/channels/kimi.rs)），桌面端等待/取消 UI 已接通 | — | 已实现（真实账号验收 pending） |
| 模型目录 | `ModelList` query 已对 GUI 开放；[provider_assembly.rs](../../crates/app/src/provider_assembly.rs) 已实现远端 probe 失败后静态回退；SET-2 起 `provider_auth_status` 返回目录三态（remote / fixed_fallback / unavailable）；SET-5 起 xAI 走远端 `/language-models`（按 output_modalities 过滤可运行模型）、Kimi Code 走远端 `/models`（与官方 kimi-cli 同端点），未知 ID 只给保守默认；Desktop 已有来源/时间/错误标签与显式刷新 | — | 已实现（真实 API 验收 pending） |
| 默认 provider/model | 配置已有 default provider/model 语义；SET-2 增 Global 层 `write_default_model_pair` 与 `set_default_model` 命令（校验可运行目录后落盘）；SET-5 起 `provider_auth_status` 透出持久化默认项、写盘后同会话内存同步，Desktop「模型与默认项」区可设默认并在 Host 确认后同步 Composer，失效显式提示不静默切换 | 重启恢复真窗口验收待 SET-7 | 已实现（真实环境验收 pending） |
| Secret 存储 | auth backend 使用独立 `auth.json`、原子写和权限收紧；config 排除 `api_key`；ADR-046 拍板 `ApiKeySecret` 非重放单帧内存传递，SET-4 Desktop secure input 只发掩码、明文不进 projection/日志 | — | 已实现 |

归档或历史实现不能代替当前生产路径。本功能只复用当前包，不从 V1/V2 复活账户池或设置库存。

## 3. 用户流程与需求

### 3.1 进入与退出 Settings

~~~mermaid
flowchart LR
    W["工作台\nTaskRail + Timeline + Inspector"] -->|"Local 行 Settings"| S["Settings\nSettings Rail + 全宽内容"]
    S -->|"← 返回工作台"| W
    S --> P["模型与供应商"]
    P --> A["添加连接"]
    A --> H["认证"]
    H --> M["验证并获取模型"]
    M --> D["设置默认模型"]
~~~

1. 用户从 TaskRail 底部 `Local` 行的 gear 进入 Settings。
2. 左栏整体替换为 Settings Rail，首项为 `← 返回工作台`；右侧不显示 Timeline、Composer 或 Inspector，完整空间交给当前设置页。
3. 返回后恢复原 active session、Timeline 位置、Composer 草稿、Inspector 状态和进行中的 Run；Settings 本身不取消 Run。
4. 断线时保留最后一次只读结果并标记 stale；禁用所有写入/验证/刷新动作，只提供 Reconnect/返回。

### 3.2 添加供应商

1. 用户选择“添加供应商”，看到首批四家 Host 权威 descriptor。
2. 选择供应商后，只显示 Host 声明的认证方式；不由 Desktop 依据名称猜测。
3. API key 使用 secure input；OAuth 显示授权 URL、device code/浏览器动作、有效期和取消入口。
4. Host 验证凭证。认证成功和模型目录成功分别给出结果；目录失败不能抹掉已成功的认证。
5. 成功后连接进入列表。用户可刷新模型、设默认项、更换认证方式或移除连接。

### 3.3 供应商与认证矩阵

| 产品连接 | 首期认证 | 端点/目录策略 | 当前代码差距 | 权威依据 |
| --- | --- | --- | --- | --- |
| Z.AI / GLM Coding Plan | API key | 先用现有 `https://api.z.ai/api/coding/paas/v4`；未确认稳定公开 list-model API 时使用有版本标记的固定目录 | 已有 `glm-coding` API-key adapter；缺 GUI 设置面 | [Z.AI API 介绍](https://docs.z.ai/api-reference/introduction) · [模型概览](https://docs.z.ai/guides/overview/overview) |
| Kimi Platform | API key | `https://api.moonshot.ai/v1`；已认证请求 `GET /v1/models` | 需新增 Kimi 通道/descriptor | [Kimi API 概览](https://platform.kimi.ai/docs/api/overview) · [List Models](https://platform.kimi.ai/docs/api/list-models) |
| Kimi Code | OAuth Device Code | managed endpoint `https://api.kimi.com/coding/v1`；若无稳定公开目录 contract，使用固定目录并标明来源 | 需新增 OAuth adapter、refresh 与 GUI 流程 | [Kimi Code 入门](https://moonshotai.github.io/kimi-code/en/guides/getting-started.html) · [环境变量/端点](https://moonshotai.github.io/kimi-code/en/configuration/env-vars.html) |
| DeepSeek | API key | `https://api.deepseek.com`；已认证请求 `GET /models` | 已有 API-key adapter；缺 GUI 设置面 | [DeepSeek API](https://api-docs.deepseek.com/) · [List Models](https://api-docs.deepseek.com/api/list-models/) |
| xAI / Grok | OAuth Device Flow、API key | API key 使用 `https://api.x.ai/v1/models`；OAuth 目录按 adapter 能力远端获取或固定回退 | OAuth 已有；当前 xAI adapter 明确 OAuth-only 且模型固定，需补 API-key adapter | [xAI Models API](https://docs.x.ai/developers/rest-api-reference/inference/models) · [xAI CLI 登录](https://docs.x.ai/build/cli/reference) |

[OpenCode Providers](https://opencode.ai/docs/providers/) 作为聚合产品的交互与供应商覆盖参考：它使用 Models.dev，并支持 DeepSeek、Moonshot/Kimi、Z.AI 和 xAI 的对应连接方式。Pawork 的运行时权限事实仍来自供应商自身；[Models.dev](https://github.com/anomalyco/models.dev) 只可作为实现时固定目录/元数据的参考，不作为在线登录或账号授权依据。

### 3.4 模型目录规则

1. **远端优先**：凭证可用且供应商有稳定目录接口时，由 Host 发起已认证查询。返回的 ID 集合代表该账号当前可见模型。
2. **固定回退**：目录接口缺失、超时或拒绝时，使用随代码版本固定的供应商目录；页面显示来源、快照日期和失败原因，不把回退写成“实时可用”。
3. **保守合并**：远端 ID 与固定元数据按 ID 合并。未知能力、上下文窗或工具支持为 unknown；不得凭品牌推断为支持。能力冲突取交集/fail-closed。
4. **可运行过滤**：只向 Composer 暴露当前 Pawork adapter 能构造请求和解析响应的模型；xAI 图像/视频等非聊天模型即使远端返回也不展示为可选。
5. **显式刷新**：用户动作触发刷新；首期不新增持久模型缓存或后台轮询。刷新失败保留当前列表并标 stale/fallback。
6. **选择有效性**：default model 必须属于所选连接当前可运行目录；目录变化导致失效时显示“默认模型不可用”，不静默换到另一供应商。

### 3.5 需求

| ID | 要求 | 优先级 | 验收证据 |
| --- | --- | --- | --- |
| SET-001 | 从 TaskRail 进入/退出 Settings，不改变会话、草稿、Inspector 或 Run。 | Must | Desktop 状态测试 + 真窗口 |
| SET-002 | Host 返回 provider descriptor、可用认证方法、连接/目录状态；Desktop 不硬编码品牌分支。 | Must | protocol golden + host/desktop 测试 |
| SET-003 | 添加向导覆盖选择、认证、验证、目录结果、取消、错误和重试。 | Must | controller 测试 + 四家真窗口 |
| SET-004 | API key 使用非重放、不可观测 Secret 路径，只写 auth backend；替换失败保留旧凭证。 | Must | 安全回归 + auth/app 测试 |
| SET-005 | Kimi/xAI OAuth 支持开始、等待、完成、取消、过期、刷新失败和移除。 | Must | OAuth 测试 + 真实账号 |
| SET-006 | 支持远端模型目录、固定回退、来源/时间/错误标签与显式刷新。 | Must | provider/app 测试 + 真实 API |
| SET-007 | 只把 adapter 可运行模型暴露给 Composer，元数据合并保守。 | Must | provider contract 测试 |
| SET-008 | 默认 provider/model 经 Host 持久化，重启恢复；失效时不静默跨供应商切换。 | Must | 配置测试 + Host 重启 |
| SET-009 | 认证成功与目录成功是两个独立状态；断线时读旧标 stale、写动作 fail-closed。 | Must | reducer/controller 测试 + 断线复验 |
| SET-010 | 可见、键盘和 AX 动作共用业务 gate；secure input 不在 AX value 中泄漏。 | Must | Desktop/AX 定向测试 + VoiceOver 人工 |
| SET-011 | 尚无真实 Host 能力的其它 Settings 页面不显示或明确 unavailable，不出现可点击假实现。 | Must | capability honesty 测试 + 人工走查 |

并发口径：同一 provider 同时只允许一个认证/刷新操作；重复提交显式返回 busy 或复用同一进度，不并发覆盖凭证。命令幂等不得通过持久化 Secret payload 实现。

## 4. 架构与契约影响

~~~text
GPUI Settings
  → pawork-client
  → 本机认证 GUI Connection Protocol
  → AppCore settings/auth facade
  → pawork-auth + pawork-providers + pawork-workspace config
~~~

- **写入集与消费者**：未来实现会触及 protocol/app/client、auth/providers/workspace 和 desktop；不新增 crate。
- **依赖红线**：Desktop 的业务依赖仍只能是 `pawork-client`；不得直接依赖 protocol/app/providers/auth/workspace。
- **现有可复用面**：GUI `ModelList` query、`AuthStart` / `AuthRemove` 类型、AppCore auth API、通用 API-key `/models`、静态+运行期 catalog merge。
- **需要补的最小能力**：provider/auth descriptor 与状态查询；GUI 可用的 OAuth 生命周期；非重放 API-key 写入；连接移除/替换；默认 provider/model 变更与确认。
- **冻结契约**：新增/改变 GUI command/query/response/capability 必须先起草 ADR-046，bump 兼容 minor，golden/typegen 先行；旧 Host/Client 无能力时隐藏 Settings 写入口，不能降级为 Desktop 直写文件。
- **Secret 命令**：不得进入持久 command ledger payload、response replay、事件或诊断。具体瞬时传递与失败恢复由 ADR 决定；这是实现硬前置，不以普通 `AppCommand` 直接落地绕过。
- **配置**：凭证仍不进入 `PaworkConfig`。默认 provider/model 优先复用现有字段和层级；若需改变写入优先级、文件形状或配置 schema，另列契约差异并先过 ADR/golden。
- **迁移**：首期不创建账户表或模型缓存表，预期无 SQLite migration。既有 CLI 凭证必须在 Settings 中以脱敏状态可见。

精确演进规则见 [contracts.md](contracts.md) 和 [architecture.md](../architecture.md) §3.2。

## 5. 安全与隐私

- 资产：API key、OAuth access/refresh token、device code、provider endpoint、默认连接与模型选择。
- 信任边界：Desktop 是经 token proof 认证的本机客户端，但仍不是 Secret 持久化 owner；Provider 响应与错误体是不可信外部输入。
- API key 输入使用 secure control；不回显完整值、不主动写入剪贴板、不发布 AX value；提交/取消/离开页面后清空 UI 缓冲。
- 新 key 先在 Host 内存中验证，成功后原子写入 auth backend；替换失败不破坏旧凭证。若未来允许“未验证保存”，必须由用户显式选择并另行登记，不作为首期默认。
- OAuth token 只由 Host 流程换取和持久化；Desktop 只显示授权 URL、用户码、到期时间与脱敏状态。取消/过期不写半成品。
- Provider 错误必须经过既有脱敏/有界化；禁止把 request header/body、token 或供应商原始敏感错误送入 GUI Diagnostic。
- 移除连接先确认目标 provider/auth method，只删除对应 auth backend 条目；不改会话事件或历史模型记录。
- 无 workspace path、Tool、Sandbox 或 PTY 新能力；这些安全面为 none。若后续 Settings 页改变 Policy/MCP/Terminal，则各自另立安全切片。
- 最低回归：真实形态 Secret 扫描、日志/事件/DB/fixture 负断言、替换失败保旧、OAuth 过期/取消、未知 provider/method fail-closed、断线期间写入拒绝。

## 6. Desktop / CLI / 客户端

### 6.1 信息架构

Settings 沿用参考设计的 1440×1024 深色语言和 8px 节奏，不另起 Dashboard 卡片墙：

- TaskRail 底部 `Local` 行右侧新增 Settings gear。
- Settings Rail 首项固定为 `← 返回工作台`；首个可用页为“模型与供应商”。
- 内容区使用“页面标题/说明 → 已连接供应商 → 添加供应商 → 模型与默认项”的单层结构。
- provider 行只显示名称、认证方法、连接状态、模型数/目录来源和操作菜单；不显示无权威来源的余额。
- 添加流程用同一内容区内的 stepper/panel，不弹出第二窗口。

未来导航顺序：模型与供应商 → 通用 → 权限与审批 → 工具与 MCP → 终端 → 外观 → 高级 → 关于。每页只在读写契约真实存在时启用；导航不是未来功能占位清单。

### 6.2 状态、键盘与可访问性

- 初始加载、空列表、已连接、认证中、等待 OAuth、验证失败、目录回退、stale/断线、移除确认均有独立文案。
- Tab 顺序：返回 → Settings 导航 → 页面标题/主操作 → provider 列表操作 → 向导控件。Escape 关闭菜单/取消未提交步骤；不静默删除已存凭证。
- 箭头键移动 Settings 导航和单选项；Enter/Space 激活；认证进行中禁用重复提交但保留取消。
- 稳定 AX identifiers 与本地化 label 分离；status 不只靠颜色；错误获得焦点或 Announcement；secure input 的值不进入语义树。
- 1440×1024 Settings Rail 约 288px；1080×720 收敛到 240px，内容单列且主操作不溢出。字号 100/125/150% 沿用 Desktop 现有缩放。

### 6.3 CLI 与其它客户端

- 既有 `pawork auth`、`pawork models` 继续可用，是诊断/恢复入口；Settings 不改变其参数语义。
- headless/ACP 不因 Desktop 功能自动获得 Secret 写入能力；registry 必须对各通道显式声明，首期可只对认证本机 GUI 开放。
- Desktop/CLI 对同一 auth backend 的状态必须一致；GUI 设置后 `pawork auth list` / `pawork models` 能以脱敏方式核对。

本轮沿用引用设计的信息架构，不生成新的 bitmap 基准；实现后的真实窗口才决定是否需要补一张 Settings 定稿图，避免用概念图覆盖可访问性与真实数据约束。

## 7. 实现切片

| 切片 | 写入集 | 前置 | 完成条件 | 可并行性 |
| --- | --- | --- | --- | --- |
| SET-1 契约 | docs/adr、protocol、schemas、client/app contract tests | 本 Spec | ADR Accepted；Secret/状态/wire 最小形状与兼容策略锁定 | 串行 |
| SET-2 Host 门面 | auth、providers、workspace、app；对应包 Spec | SET-1 | descriptor、状态、凭证操作、默认项由 Host 单点提供 | 串行 |
| SET-3 Settings 壳 | desktop；desktop 产品/包 Spec | SET-1 query 形状 | route、rail、返回、断线/空态、AX 接通 | 可与 SET-2 后半只读部分协调，默认串行 |
| SET-4 Provider auth | providers、auth、app；对应包 Spec | SET-2 | 四家认证矩阵完成，xAI API key/Kimi 两种连接补齐 | 串行 |
| SET-5 Catalog/default | providers、app、protocol/client、desktop、workspace | SET-2～4 | 远端/固定目录、刷新、过滤、默认项与 Composer 同步 | 串行 |
| SET-7 验收 | 测试/文档；仅修真实缺陷 | SET-3～5 | 定向门禁、四家真实账号、断线/重启、AX/窄窗证据 | 串行 |

其余 Settings 页不塞入 SET-1～5。模型与供应商收口后，每页按真实能力分别建小切片；不得以“完整设置中心”为由同时修改无关包。

## 8. 验证计划

| 需求 | E1 实现证据 | E2 自动化 | E3 真实环境 | E4 人工 |
| --- | --- | --- | --- | --- |
| SET-001/010/011 | desktop route/render/controller/AX | desktop 定向测试 | 正式 Host/Desktop，1440/1080 | 视觉、键盘、VoiceOver 签字 |
| SET-002/009 | protocol registry + app query | protocol golden/typegen + app/client tests | 断线/重连与旧 Host | 状态文案走查 |
| SET-003/004 | app auth facade + desktop wizard | auth/app/desktop；Secret 负断言 | 真实 API key，替换失败 | secure input/删除确认 |
| SET-005 | Kimi/xAI OAuth adapter | OAuth refresh/cancel/error tests | 真实账号与 device flow | 浏览器切换/超时走查 |
| SET-006/007 | provider list_models/catalog merge | provider contract + fallback/filter tests | 四家远端目录或明确固定回退 | 来源/降级文案 |
| SET-008 | workspace/app config writer | 配置层级与重启测试 | Host/Desktop 重启 | 默认项失效走查 |

受影响关键回归：协议/golden、Secret/脱敏、配置持久化。测试使用假 key/token 形态；真实凭证只在隔离实例中读取，输出前脱敏。任何 401/429/超时先记录真实类别，不用 mock 冒充 E3。

## 9. 运行、迁移与回滚

- 默认关闭未协商的 Settings 写能力；旧 Host/Client 组合只显示不兼容/只读，不静默降级。
- 既有 CLI auth 条目原样可见；不批量迁移、不重写凭证文件。
- 首期不建数据库表、不持久化模型缓存；固定目录随代码回滚。
- 新凭证验证失败保留旧凭证；移除后可通过 CLI 或 Settings 重新登录恢复。
- 诊断只记录 provider ID、auth method、阶段、错误类别和脱敏原因，不记录 Secret。
- 发布、License、三平台供应链、安装/升级/回滚门禁：**none，本 Feature 明确排除**。

## 10. 文档与收尾

- [x] ROADMAP、Feature Spec、任务书和 GUI 行为设计同步。
- [x] product/capabilities/desktop/verification/backlog 索引本 Feature，状态诚实标为未实现。
- [x] SET-1 后同步 contracts/architecture/ADR/protocol/client/app 包级 Spec。
- [x] SET-2 后同步 providers/workspace/app 包级 Spec（pawork-auth 零改动，无需回写）。
- [ ] SET-3～5 后同步 desktop 等实际写入集包级 Spec。
- [ ] 每片写入实际验证和已知缺口；完成过程压缩进 history。
- [ ] 模型与供应商真实验收后再决定是否补 Settings bitmap 基准。

## 11. 决策与开放问题

| ID | 问题/决策 | 结论 | 状态 |
| --- | --- | --- | --- |
| SET-D01 | Settings 是否替换工作台内容 | Settings Rail 替换 TaskRail，右侧占完整内容；返回恢复工作台状态 | Accepted |
| SET-D02 | 首批供应商 | Z.AI/GLM、Kimi、DeepSeek、xAI/Grok | Accepted |
| SET-D03 | 认证方法由谁决定 | Host descriptor 声明，Desktop 不硬编码品牌 | Accepted |
| SET-D04 | 首期连接数量 | 每 provider 一个活动连接；多账户池不在范围 | Accepted |
| SET-D05 | 模型目录优先级 | 已认证远端优先，固定目录有版本标记回退，第三方不作运行时权限源 | Accepted |
| SET-D06 | 发布是否进入计划 | 不进入；由用户后续单独指定 | Accepted |
| SET-D07 | API key 的 GUI wire/ledger 形状 | ADR-046 在 golden 前拍板 | Open / 硬前置 |
| SET-D08 | Kimi OAuth 模型目录 contract | SET-5 取证：官方 kimi-cli 实际请求 `https://api.kimi.com/coding/v1/models`（OpenAI 风格 `data[]`），按远端优先实现；形状不符/失败一律 Err，由 Host 落版本固定回退 | Accepted |
| SET-D09 | Z.AI General API preset | 首期不开放，只做 Coding Plan；后续按需求再决定 | Deferred |
