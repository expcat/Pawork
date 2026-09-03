# ADR-051：Settings「关于」页的 Host 数据目录握手元数据（API 1.9）

- **状态**：Accepted（用户于 2026-09-03 确认；D1～D5 已实现并通过定向门禁）
- **日期**：2026-09-03

## 背景

SET-6g 只有在构建版本、实际协商协议版本和数据目录都有权威来源时才启用「关于」页，且不得以 updater / release 占位。当前源码事实是：

- Desktop 构建版本可直接取本 crate 的 `env!("CARGO_PKG_VERSION")`，来源明确；
- 实际协商 GUI API 已由 `GuiClient::api_version()` 保存并在高级页消费；
- Host 使用的数据目录由 `AppCore::load_with` 从 `AppLoadOptions.data_dir` 或 `default_data_dir_outcome()` 决定，但握手与 `SessionInfo` 均未携带它；
- Desktop 自行复刻默认目录解析、从 socket endpoint 反推，或 shell-out `pawork doctor`，都不能证明得到当前 Host 真正使用的目录；`--socket` 覆盖进一步使 endpoint 与数据目录没有必然关系。

因此，本片唯一缺失契约是「当前已认证 Host 实际使用的数据目录」。

## 决策

### D1 — GUI Host 启动时只解析一次数据目录

- `pawork gui serve` 在加载 `AppCore` 前消费一次 `default_data_dir_outcome()`，把同一个 `PathBuf` 写入 `AppLoadOptions.data_dir`，并传给 `run_gui`。
- `AppCore` 的 session / artifact / protected / control-plane 路径，以及 GUI socket / PID / token 路径，均继续由这一个值派生；`--socket` 只覆盖监听 endpoint，不改变数据目录。
- 不给 `AppCore` 新增仅为 UI 服务的 getter，也不在 Desktop 复制 Host 目录解析。

### D2 — Accepted 握手追加可选 `host_data_dir`

- `HandshakeResponse::Accepted` 追加 `host_data_dir: Option<String>`，使用 `#[serde(default, skip_serializing_if = "Option::is_none")]`；`Rejected` 不携带该字段。
- `HandshakeService` 增最小 builder，由 `pawork gui serve` 注入 D1 的路径展示值。正式 GUI Host 在认证成功的 `Accepted` 中发送；旧 Host 和未配置该元数据的测试/通用 Host 均返回缺省。
- `pawork-client::SessionInfo` 原样保存该可选字段；Desktop 不把缺失解释成默认目录。
- 路径只用于本机已认证 About UI 展示，不写日志、事件、ledger、数据库或诊断文件，也不作为后续文件操作输入。

### D3 — About 只展示三项权威只读信息

- `Desktop build`：`apps/desktop` 的 `env!("CARGO_PKG_VERSION")`；标签明确这是 Desktop 构建，不冒充 Host 二进制版本。
- `GUI API`：当前连接实际协商版本，不显示客户端支持上限。
- `Host data directory`：D2 的 `host_data_dir`。
- 当前连接没有非空 `host_data_dir` 时不显示 About 导航；仅用 `trim()` 判断全空白字段不可用，合法路径值本身原样展示。已在 About 页时若连接丢失或元数据清空，路由退回高级页，避免展示旧路径。
- 页面不新增按钮，不展示更新检查、release channel、License、安装器状态或版本比较占位；render 与 AX 共用同一只读行数据。

### D4 — API 1.9，golden / typegen 先行

- `SUPPORTED_API_VERSIONS` 追加 1.9；不新增 command、query、capability 或 registry 项，现有 28 command / 15 query 保持不变。沿用 ADR-046 D5 至 ADR-050 的已接受策略：初始未发布阶段 minor 只作记账，不为 1.8 建运行时发送分支。
- golden/typegen 先行：当前 64 个 GUI fixture 数量不变，其中 44 个引用 `API_VERSION` 的 fixture 预期仅把 minor 8 机械更新为 9；Accepted 握手 fixture 另增 `host_data_dir`。实现时先核对实际数量，不把这批可预期更新误判为扩范围。
- 补一条缺字段仍可解码且 About 不启用的证据；旧 Host 缺字段时新 Desktop fail-closed 隐藏 About。旧客户端按现有 serde 宽容读取忽略新增字段，不另建兼容实现。

### D5 — 最小写入集与验收

生产写入集限定为：

- `crates/protocol/`、`schemas/`：可选握手字段、API 1.9、golden/typegen；
- `crates/client/`：`SessionInfo` 保存字段；
- `crates/cli/`：单次解析并把同一路径交给 Core 与握手；
- `apps/desktop/`：About 路由、只读 render/AX 和断线清空；
- 上述包级 Spec、Settings/契约/架构/GUI 文档与完成后的 history 状态。

不改 config/schema、SQLite、Provider、Secret、Policy、App query/command registry，不新增 crate、依赖、偏好框架或测试体系。自动验证使用现有 protocol/client/cli/Desktop 测试；Desktop 只扩展既有 Settings 主路径，覆盖「有元数据启用 + 丢失后隐藏」这一主路径与关键失败态。真窗口在 SET-7 对照 `pawork doctor --json` 的数据目录与当前协商 API，人工视觉/键盘/VoiceOver 单独签字。

## 否决支

- **从 endpoint 反推数据目录**：`--socket` 可指向任意位置，结论不可靠。
- **Desktop 复刻 `default_data_dir`**：只能得到 Desktop 进程的推断值，不能证明 Host 的加载输入。
- **Desktop shell-out `pawork doctor`**：引入第二进程、实例参数同步与额外失败面，仍不如当前连接直接声明。
- **新增 AppQuery**：数据目录是连接建立时已固定的只读元数据；为一个不可变字符串增加请求/响应、registry 和加载态没有必要。
- **同时传 Host build version**：当前完成口径只要求权威构建版本，明确标注的 Desktop build 已满足；Host 版本未被当前需求证明必要，留待真实诊断需求另行立项。

## 后果与回滚

- 正向：About 的三项内容各有单一事实源；旧 Host/Client 组合自然隐藏页面，不会用猜测补值。
- 代价：Accepted 握手多一个可选本机路径字段，API minor 增至 1.9；该路径只在认证成功后发送，minor 仍不承担运行时门控。
- 回滚：移除 About 消费并停止注入字段即可隐藏页面；在初始未发布阶段可随同 API 1.9 golden 回退，不保留第二套路径推断逻辑。

## 实施状态

- 2026-09-03 已实现 API 1.9、可选 `host_data_dir`、GUI Host 单次数据目录解析与 `SessionInfo` 透传；Desktop About 只在当前连接携带非空路径时显示，合法路径值原样呈现，断线或缺字段立即隐藏并退回高级页。
- 64 个 GUI fixture 数量保持不变，44 个引用当前 `API_VERSION` 的 fixture 从 1.8 更新为 1.9；command/query 仍为 28/15，typegen 已同步。
- protocol、client/CLI、Desktop 定向门禁已通过；正式 Host 数据目录对照、真窗口视觉/Tab/Enter/VoiceOver 仍待 SET-7，不以自动测试冒充人工验收。
