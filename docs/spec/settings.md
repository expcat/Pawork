# Settings：模型与供应商

## 元数据

| 字段 | 值 |
| --- | --- |
| Feature ID / 名称 | `SETTINGS-01` / Settings 与模型供应商管理 |
| 状态 | **Accepted（SET-1～SET-6g 已实现并通过各自定向门禁；SET-012 Network 本机代理闭环已获 E1～E3；SET-6h 供应商级代理开关（ADR-052）已实现并通过 protocol/workspace/app 定向门禁，真窗口验收通过（2026-09-05）；真实账号矩阵的完整人工验收仍 pending）** |
| Owner | Pawork maintainers |
| 目标阶段 | Settings 活动线；不绑定发布版本 |
| 最近更新 | 2026-09-05（SET-6h 供应商级代理开关与 Network 本机代理配置边界） |
| 关联 | [GUI 设计](../gui-design.md) · [AGENTS.md](../../AGENTS.md) |

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
| Settings 入口/路由 | SET-3 起 TaskRail `Local` 行 gear + AppRoute 顶层路由 + Settings Rail + 只读供应商页落地（[settings/](../../apps/desktop/src/ui/settings/)）；SET-6a 的代理入口现显示为 Network，SET-6b～6d 依次启用权限与审批、工具与 MCP、终端；SET-6e/6f 启用始终可用的本地外观页与高级连接诊断页；SET-6g 启用由当前握手权威元数据驱动的 About 页 | — | 已实现；Network 与 Global `config.toml` 共用 Host 权威读写路径 |
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
2. 左栏整体替换为 Settings Rail（默认 English，可在 Appearance 页切换为简体中文），首项为 `← Back to workspace`；右侧不显示 Timeline、Composer、Inspector 或 RunStatusBar，完整空间交给当前设置页。
3. 返回后恢复原 active session、Timeline 位置、Composer 草稿、Inspector 状态和进行中的 Run；Settings 本身不取消 Run。
4. 断线时保留 Host-backed 页最后一次只读结果并标记 stale；禁用所有写入/验证/刷新动作。高级页仍可查看连接失败原因与 endpoint，并提供同源 Reconnect/返回；旧握手摘要不得冒充当前连接。

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
| SET-010 | 可见、键盘和 AX 动作共用业务 gate；secure input 不在 AX value 中泄漏。 | Must | Desktop/AX 定向测试 + 键盘人工走查 |
| SET-011 | 尚无真实 Host 能力的其它 Settings 页面不显示或明确 unavailable，不出现可点击假实现。 | Must | capability honesty 测试 + 人工走查 |
| SET-012 | Network 页读写用户 Global `proxy_url`；配置文件位于 workspace 外，workspace `.pawork/config.toml` 不得覆盖代理。 | Must | workspace/app 定向测试 + Host 重启真窗口 |
| SET-013 | 供应商级代理开关：配置了 Global `proxy_url` 后，provider 概览行可切换走代理 / 直连；写只落 Global 层 `config.toml`，回执即写后状态，非 Global 层 `use_proxy` 一律剥离。 | Must | protocol/workspace/app 定向测试 + 真窗口 |

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
- **冻结契约**：新增/改变 GUI command/query/response/capability 必须先起草对应 ADR、bump 兼容 minor，并以 golden/typegen 先行；SET-1 由 ADR-046 承载，SET-6g 的可选 Accepted 握手字段由 ADR-051 Accepted 承载并随 API 1.9 落地。SET-6h 供应商级代理开关由 ADR-052 承载并随 API 1.10 落地。旧 Host/Client 无能力时隐藏对应 Settings 入口，不能降级为 Desktop 直写文件或本地推断。
- **Secret 命令**：不得进入持久 command ledger payload、response replay、事件或诊断。具体瞬时传递与失败恢复由 ADR 决定；这是实现硬前置，不以普通 `AppCommand` 直接落地绕过。
- **配置**：凭证仍不进入 `PaworkConfig`。Network 沿用既有 Global `proxy_url`、`general_settings` / `set_proxy_url` wire 与原子 writer，不改变 schema 或优先级；文件由 `config_dir_for_app` 定位在 workspace 外。workspace 层代理继续剥离。供应商级 `use_proxy` 经原子 writer `write_provider_use_proxy` 只写 Global 层 `[[providers]]` 条目（缺条目时新增最小条目），非 Global 层 `use_proxy` 与 `base_url` 同红线剥离。默认 provider/model 仍复用现有字段和层级。
- **迁移**：首期不创建账户表或模型缓存表，预期无 SQLite migration。既有 CLI 凭证必须在 Settings 中以脱敏状态可见。

精确演进规则见 [contracts.md](contracts.md) 和 [architecture.md](../architecture.md) §3.2。

## 5. 安全与隐私

- 资产：API key、OAuth access/refresh token、device code、provider endpoint、默认连接与模型选择。
- 信任边界：Desktop 是经 token proof 认证的本机客户端，但仍不是 Secret 持久化 owner；Provider 响应与错误体是不可信外部输入。
- API key 输入使用 secure control；不回显完整值、不主动写入剪贴板、不发布 AX value；提交/取消/离开页面后清空 UI 缓冲。
- 新 key 先在 Host 内存中验证，成功后原子写入 auth backend；替换失败不破坏旧凭证。若未来允许“未验证保存”，必须由用户显式选择并另行登记，不作为首期默认。
- OAuth token 只由 Host 流程换取和持久化；Desktop 只显示授权 URL、用户码、到期时间与脱敏状态。取消/过期不写半成品。
- Provider 错误必须经过既有脱敏/有界化；禁止把 request header/body、token 或供应商原始敏感错误送入 GUI Diagnostic。
- 高级页只发布非 Secret 握手摘要、当前 socket endpoint、resume/ack；断线即清 runtime/API/capabilities。不得显示 GUI token、token path，不得从 socket 路径推断 data directory 或配置实例名。
- SET-6g 边界：Host data directory 仅在认证成功的 Accepted 握手中可选发布，只用于本机 About 原样展示；API 1.9 沿用初始未发布阶段 minor 只记账策略，不新增 1.8 运行时分支。路径不得进入日志、事件、ledger、数据库或后续文件操作输入；缺字段、仅空白字段或断线时 About fail-closed 隐藏。
- 移除连接先确认目标 provider/auth method，只删除对应 auth backend 条目；不改会话事件或历史模型记录。
- 无 workspace path、Tool、Sandbox 或 PTY 新能力；这些安全面为 none。若后续 Settings 页改变 Policy/MCP/Terminal，则各自另立安全切片。
- 最低回归：真实形态 Secret 扫描、日志/事件/DB/fixture 负断言、替换失败保旧、OAuth 过期/取消、未知 provider/method fail-closed、断线期间写入拒绝。

## 6. Desktop / CLI / 客户端

### 6.1 信息架构

Settings 沿用参考设计的 1440×1024 深色语言和 8px 节奏，不另起 Dashboard 卡片墙：

- TaskRail 底部 `Local` 行右侧新增 Settings gear。
- Settings Rail 首项固定为 `← Back to workspace`；八页导航默认 English（Appearance 页可切换简体中文，当次会话生效、不持久化），首个可用页为 `Models & providers`，内容最大宽 820px。
- 内容区使用稳定的 page header / section / field / feedback 层级；不显示工作台 RunStatusBar。
- provider 默认层为 64px 概览行，只显示名称、认证方法、连接状态、目录 availability / 模型数和可用动作；普通 render 与 AX summary 不发布 masked credential、endpoint、catalog error、raw model id 或无权威来源余额。endpoint / 错误仅在连接、等待或删除确认详情显示；API key editor 仅在 Connect / Replace 后展开。
- 默认模型使用独立 section；认证成功与目录成功继续分开表达，Remove 仍需二次确认。
- 添加流程用同一内容区内的 stepper/panel，不弹出第二窗口。

导航顺序：Models & providers → Network（SET-6a）→ Approvals（SET-6b）→ Tools & MCP（SET-6c）→ Terminal（SET-6d）→ Appearance（SET-6e）→ Advanced（SET-6f）→ About（SET-6g）。Network / Terminal 使用 label-help-feedback；Approvals 的五档 mode 为整行 radio，mouse / Enter / Space / AX Press 同源；Appearance 有随 `TextScale` 即时变化的正文 / control 样例，并提供 English / 中文 语言切换（同源按钮，即时重渲染）；Advanced / About 使用固定 label 列的 definition list。Host capability 与既有读写 / stale 边界不变；Appearance / Advanced 离线仍可进入。

### 6.2 状态、键盘与可访问性

- 初始加载、空列表、已连接、认证中、等待 OAuth、验证失败、目录回退、stale/断线、移除确认均有独立文案。
- Tab 顺序：返回 → Settings 导航 → 页面标题/主操作 → provider 列表操作 → 向导控件。Escape 关闭菜单/取消未提交步骤；不静默删除已存凭证。
- 箭头键移动 Settings 导航和单选项；Enter/Space 激活；认证进行中禁用重复提交但保留取消。
- 稳定 AX identifiers 与本地化 label 分离；status 不只靠颜色；错误获得焦点或 Announcement；secure input 的值不进入语义树。
- 普通 provider AX summary 不携带 masked credential；secure input 只发布等长掩码。stale 时输入与所有写动作 disabled，且 disabled 节点不发布 Press。
- 1440×1024 Settings Rail 约 288px；1080×720 收敛到 240px，内容单列且主操作不溢出。字号 100/125/150% 沿用 Desktop 现有缩放。

### 6.3 CLI 与其它客户端

- 既有 `pawork auth`、`pawork models` 继续可用，是诊断/恢复入口；Settings 不改变其参数语义。
- Host 级 PID、socket 存活与握手自检仍由 pre-Core `pawork --instance <name> doctor` 负责；高级页不 shell-out、不从 endpoint 猜 instance。About 直接显示认证握手声明的当前 Host data directory，并在 SET-7 与 `doctor --json` 对照；不能用 `doctor` 反向填充 UI。
- headless/ACP 不因 Desktop 功能自动获得 Secret 写入能力；registry 必须对各通道显式声明，首期可只对认证本机 GUI 开放。
- Desktop/CLI 对同一 auth backend 的状态必须一致；GUI 设置后 `pawork auth list` / `pawork models` 能以脱敏方式核对。

P2 视觉方向见 [desktop-ui-p2-settings-v4.png](../../design/desktop-ui-p2-settings-v4.png)；它不替代真实数据或安全证据。2026-09-05 本机八页、English/中文、三档字号、窄窗与键盘走查已完成 E3，记录见 [Desktop Spec §8](desktop.md#8-gui-收尾验收记录2026-09-05)；四家真实认证/目录矩阵与 E4 签字仍独立登记。

## 7. 实现索引

SET-1～SET-6h 已落地：契约在 protocol/client，认证与目录由 auth/providers/app 提供，Global 配置由 workspace writer 持久化，Desktop 提供八页 Settings 与供应商代理开关。模块、API、边界及对应验证以各包 Spec 和下表为准；已完成的切片排期不再保留。

## 8. 验证与证据

| 需求 | E1 实现证据 | E2 自动化 | E3 真实环境 | E4 人工 |
| --- | --- | --- | --- | --- |
| SET-001/010/011 | desktop route/render/controller/AX | desktop 定向测试 | 正式 Host/Desktop，1440/1080 | 视觉、键盘签字 |
| SET-002/009 | protocol registry + app query | protocol golden/typegen + app/client tests | 断线/重连与旧 Host | 状态文案走查 |
| SET-003/004 | app auth facade + desktop wizard | auth/app/desktop；Secret 负断言 | 真实 API key，替换失败 | secure input/删除确认 |
| SET-005 | Kimi/xAI OAuth adapter | OAuth refresh/cancel/error tests | 真实账号与 device flow | 浏览器切换/超时走查 |
| SET-006/007 | provider list_models/catalog merge | provider contract + fallback/filter tests | 四家远端目录或明确固定回退 | 来源/降级文案 |
| SET-008 | workspace/app config writer | 配置层级与重启测试 | Host/Desktop 重启 | 默认项失效走查 |
| SET-012 | workspace Global `config.toml` + Desktop Network | workspace/app 既有 proxy 持久化与安全剥离测试 | GUI 保存后重启 Host，真实 Provider 直连当前代理 | Network 文案、键盘与 AX 一致 |
| SET-6h | provider 行开关 + `set_provider_use_proxy` 闭环 | protocol 1.10 golden + workspace writer / 非 Global 剥离 + app handler 定向测试 | 配置全局代理后真窗口切换开关并持久化到 Global `config.toml` | 开关文案、键盘与 AX 一致 |
| SET-6e 外观 | desktop render + `TextScale` + AX tree | 离线导航/AX Press/根字号/selected 定向回归 | 正式窗口 100/125/150% 与重启 | 视觉、Tab/Enter 签字 |
| SET-6f 高级 | desktop handshake 摘要 + render/AX 同源行 | 离线/连接两态、Reconnect gate、旧握手清空 | 正式 Host 对照 API/capabilities/endpoint/resume/ack | 视觉、Tab/Enter 签字 |
| SET-6g 关于 | Accepted 握手可选 `host_data_dir` + desktop render/AX 同源行 | 握手 present/absent + About 启用/清空隐藏 | 正式 Host 对照 build、协商 API 与 `doctor --json` data directory | 视觉、Tab/Enter 签字 |

受影响关键回归：协议/golden、Secret/脱敏、配置持久化。测试使用假 key/token 形态；真实凭证只在隔离实例中读取，输出前脱敏。任何 401/429/超时先记录真实类别，不用 mock 冒充 E3。

SET-012 本机证据（2026-09-05，macOS）：GUI `Settings → Network` 将代理保存为 `http://127.0.0.1:38081`，落盘到 workspace 外的用户 Global `config.toml`；旧临时转发端口 `7890` 保持关闭。Host 重启后页面恢复同一值，远端 OpenCode Go 目录从静态回退恢复为 29 项，`opencode-go / glm-5.3-flash` 真窗口请求返回精确 `proxy-ok` 并进入 `Run completed`。E2 同批通过 `pawork-workspace` 150 个测试、app 3 个 proxy handler 测试及 Desktop 186 个 bin 测试；E4 用户签字未由此自动推定。

SET-6h 本机证据（2026-09-05，macOS）：Global `config.toml` 配置 `proxy_url` 后，真窗口各 provider 行显示 `走代理` 开关；中文模式点击 xAI 开关 `走代理`→`直连`，文件即时写入 `[[providers]] id="xai" use_proxy=false`，再点恢复 `use_proxy=true`。重启 Host 并移除 `proxy_url`（Host 启动缓存配置）后开关不再渲染（AX 树 + 截图），行内其它按钮不变；恢复代理配置并重启 Host 后开关重新显示。E2 同批通过 protocol 144、workspace/app 定向门禁与 Desktop 188/188；E4 用户签字未由此自动推定。

## 9. 运行、迁移与回滚

- 默认关闭未协商的 Settings 写能力；旧 Host/Client 组合只显示不兼容/只读，不静默降级。
- 既有 CLI auth 条目原样可见；不批量迁移、不重写凭证文件。
- 首期不建数据库表、不持久化模型缓存；固定目录随代码回滚。
- 新凭证验证失败保留旧凭证；移除后可通过 CLI 或 Settings 重新登录恢复。
- 诊断只记录 provider ID、auth method、阶段、错误类别和脱敏原因，不记录 Secret。
- 发布、License、三平台供应链、安装/升级/回滚门禁：**none，本 Feature 明确排除**。

## 10. 文档与收尾

- [x] Feature Spec 与 GUI 行为设计同步，已完成的活动规划文档已清理。
- [x] product/capabilities/desktop/verification/backlog 索引本 Feature，状态随各切片诚实同步。
- [x] SET-1 后同步 contracts/architecture/ADR/protocol/client/app 包级 Spec。
- [x] SET-2 后同步 providers/workspace/app 包级 Spec（pawork-auth 零改动，无需回写）。
- [x] SET-3～6g 已同步各实际写入集包级 Spec。
- [x] SET-3～6g 已逐片写入实际验证和已知缺口，并压缩进 history。
- [x] 视觉方向沿用 P2 阶段图；真窗口证据留在验收任务，不向仓库新增 bitmap。

## 11. 决策与开放问题

| ID | 问题/决策 | 结论 | 状态 |
| --- | --- | --- | --- |
| SET-D01 | Settings 是否替换工作台内容 | Settings Rail 替换 TaskRail，右侧占完整内容；返回恢复工作台状态 | Accepted |
| SET-D02 | 首批供应商 | Z.AI/GLM、Kimi、DeepSeek、xAI/Grok | Accepted |
| SET-D03 | 认证方法由谁决定 | Host descriptor 声明，Desktop 不硬编码品牌 | Accepted |
| SET-D04 | 首期连接数量 | 每 provider 一个活动连接；多账户池不在范围 | Accepted |
| SET-D05 | 模型目录优先级 | 已认证远端优先，固定目录有版本标记回退，第三方不作运行时权限源 | Accepted |
| SET-D06 | 发布是否进入计划 | 不进入；由用户后续单独指定 | Accepted |
| SET-D07 | API key 的 GUI wire/ledger 形状 | ADR-046 已拍板非重放 `ApiKeySecret` + ledger 只缓存脱敏响应 | Accepted |
| SET-D08 | Kimi OAuth 模型目录 contract | SET-5 取证：官方 kimi-cli 实际请求 `https://api.kimi.com/coding/v1/models`（OpenAI 风格 `data[]`），按远端优先实现；形状不符/失败一律 Err，由 Host 落版本固定回退 | Accepted |
| SET-D09 | Z.AI General API preset | 首期不开放，只做 Coding Plan；后续按需求再决定 | Deferred |
| SET-D10 | About 如何获得当前 Host 数据目录 | ADR-051 Accepted：API 1.9 Accepted 握手追加可选 `host_data_dir`；GUI Host 与 Core 共用同一次解析，缺字段、空字段或断线时隐藏 About | Accepted（已实现并通过定向门禁） |
