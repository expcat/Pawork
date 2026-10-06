# CU-01 · macOS 本机后台操作可行性与能力基线

> 状态：等待人工验收（P0）。前置：无。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

证明用户继续操作本机时，Agent 能在后台观察和操作目标应用；先取得真实支持边界，再进入原生重构。

## 当前依据与读取范围

2026-10-06 静态对比固定于 Pawork `87d878f65db634e64974cb920d0bfa76ad965f57` 和 LCU `21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a`。Pawork 已是 Rust RFB 引擎；LCU 是官方已安装运行时的包装与接入层，其 MIT 代码不提供底层引擎源码。原生输入与实时验收的实测结果见下方完成记录。

[computer-use Spec](../spec/crates/computer-use.md)、[browser Spec](../spec/crates/browser.md)、[现行设计](../design.md#computer-use-首版)、[架构](../architecture.md)。

## 对比与目标边界

| 维度 | 已核实现状 | 本次升级目标 |
| --- | --- | --- |
| 引擎 | Pawork 是纯 Rust；LCU 调用原 runtime / Sky / REPL | 独立 Rust 能力实现，不依赖官方应用组件 |
| 桌面 | Pawork 只操作专用 Xvnc；无应用/窗口/AX 树 | 本机目标身份、窗口观测和受控元素动作 |
| 浏览器 | WebKit 基本 DOM；Chrome/Edge 是只读上下文 | 内置与外部浏览器的真实截图、操作和标签页 |
| 图片 | 进入模型并持久化；GUI 时间线仅投影文本 | 保留模型通路，补用户可查看证据 |
| 平台 | LCU Linux 部分 GTK/Qt 路径会激活窗口；Windows 仍是候选 | 不照搬会干扰用户的输入回退 |

依据：[LCU README](https://github.com/amontlabs/lcu/blob/21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a/README.md)、[实现约定](https://github.com/amontlabs/lcu/blob/21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a/AGENTS.md)、[Provenance](https://github.com/amontlabs/lcu/blob/21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a/docs/PROVENANCE.md)、[Linux 输入限制](https://github.com/amontlabs/lcu/blob/21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a/docs/upstream/openai-linux-window-input.md)、[官方 Computer Use](https://learn.chatgpt.com/docs/computer-use)、[官方 Browser](https://learn.chatgpt.com/docs/browser)。引擎接口对齐不等于已证明模型任务成功率与 Codex 相同。

## 实施范围

- 以 AppKit、Electron 和浏览器应用建立“应用 × 动作 × 窗口状态”矩阵；覆盖窗口被遮挡、用户在其它应用持续输入、取消和用户接管。
- 分别验证 ScreenCaptureKit 窗口截图、AX 读取/语义动作、定向文本/按键和指针动作；独立记录权限、签名/进程归属与调用线程要求。
- 连续观察用户鼠标、前台窗口、输入焦点和剪贴板；使用用户侧文本结果与目标侧 UI/文件事实核对，不能只比较操作前后两张截图。
- 只使用公开 API 和独立 Rust 实现。`CGEventPostToPid`、Private event source、进程锁或切回焦点都不能单凭 API 名称证明无干扰；需要实测。
- 明确普通后台、最小化和锁屏的实际支持边界；锁屏能力另见 [CU-24](cu-24-macos-locked-use.md)，不把未执行环境记为支持。

## 完成条件与验证

- 用户继续正常键鼠操作，Agent 不抢焦点、不移动用户指针、不串入用户输入或污染剪贴板；目标动作及新观测同时有证据。
- 用户开始操作同一目标时能停止竞争；没有前台激活、全局输入或事后恢复状态的隐式降级。
- 记录每类动作的通过、不支持和失败。未证明原生路径满足目标时，阻塞相关实现；容器通过不能替代本机后台验收。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

2026-10-06 实测完成。环境：macOS 26.6.2 arm64，双显示器（主屏 3440 + 副屏 2560）；探针为 crates/computer-use/examples/macos_background_probe.rs（公开 API：ScreenCaptureKit 单帧窗口截图、HIServices AX、CGEventPostToPid；adhoc linker-signed 无 TeamID），经 `cargo build -p pawork-computer-use --example macos_background_probe` 构建。Screen Recording 与 Accessibility 授权归属责任进程链（ChatGPT → CodexCLI → zsh），探针作为子进程继承；多次重编译（cdhash 变化）后授权不失效。目标应用：TextEdit 12.0（AppKit）、VS Code（Electron，能力矩阵首测 1.137.0、指针单元复测 1.140.0）、Chrome 154（独立 `--user-data-dir` 实例，不触碰用户已有 Chrome 会话）。证据（截图、watch JSONL、各单元日志）在 /tmp/cu01/，不入仓库。用户侧输入由探针 session 级事件模拟（user-typed.txt 落盘核对），未经真实人工在场操作。

能力矩阵（✅=实测通过，❌=实测无效或不支持）：

| 动作 | TextEdit（AppKit） | VS Code（Electron） | Chrome（浏览器） |
| --- | --- | --- | --- |
| SCK 窗口截图（遮挡/后台） | ✅ 返回目标窗口自身内容，非遮挡者 | ✅ | ✅ |
| SCK 窗口截图（最小化） | ✅ 实时渲染（AX 插入后内容变化） | ✅ 实时渲染编辑器 | ✅ 实时渲染页面 |
| AX 读取 | ✅ / ✅（含 AXValue 全文与 AXSelectedTextRange 光标） | ✅（须先开启完整 AX 树） | ✅（AXWebArea、omnibox AXValue） |
| AX 语义写/按压（遮挡/后台） | ✅ AXSetValue 替换与 AXSelectedText 插入均落地 | ❌ 插入返回 success 但 delta=0（Monaco 代理 textarea 不回写文档） | ✅ AXPress 执行 JS 与导航（按钮→标题 CU01-BTN） |
| AX 语义动作（最小化） | ✅ 同上 | 未单独测（后台已 ❌） | ✅ AXPress 链接→导航成功（标题 CU01-PAGE2） |
| CGEventPostToPid 定向文本/按键（遮挡/后台） | ✅ 落地；依赖窗口为 app 内 key（AXMain=true 可恢复，不抢前台） | ❌ 静默丢弃（AXMain=true 也不落）；前台 ✅ | ❌ 静默丢弃 |
| 定向按键（最小化） | ❌ 静默丢弃 | ❌ | 未测（后台已 ❌） |
| 定向菜单快捷键（Cmd+S） | ❌ 后台不路由（文件未变） | 前台 ✅；后台无法构造脏缓冲（见下） | 未测 |
| 菜单栏 AXPress | ❌ 非激活时菜单不暴露子项 | 菜单项暴露且按压返回 success；效果与 auto-save 混淆，未定论 | — |
| CGEventPostToPid 指针点击 | ❌ caret 不动（点击落在窗口内，AXSelectedTextRange 不变） | ❌ caret 不动（V3 复测：两次点击分别落在编辑器文本区与空白区，AXSelectedTextRange 不变） | ❌ 按钮未触发（标题不变） |
| AX 写窗口位置 | ✅ AXPosition/AXSize 生效 | ❌ AXPosition 被忽略（AXSize 生效） | ❌ AXPosition 被忽略（AXSize 生效） |

关键过程事实：

- Electron/Chromium 完整 AX 树：VS Code 默认仅 12 节点（窗口壳），写 AXManualAccessibility=1（success）后约 2 秒内物化为 1054 节点，编辑器内容以 AXTextArea 全文暴露；AXEnhancedUserInterface 由 Chromium 自行置位。Chrome 写 AXManualAccessibility 返回 attribute_unsupported(-25205)，但 AX 客户端活动后树同样自行开启。
- VS Code 后台 Cmd+S 未定论的原因：该 profile 开 files.autoSave=onFocusChange，退后台即自动落盘；而后台按键/语义写均无效，无法构造「后台脏缓冲」。
- TextEdit 的 te.txt 最终落盘是其自身 autosave 行为，与定向 Cmd+S 无关（按压当时文件 md5/mtime 未变）。
- 系统级 AXFocusedUIElement 持续返回 -25204(cannotComplete)，watch 采应用级焦点属性兜底，系统级错误码留作诊断字段。
- 审查修复轮（同日）：探针异步完成回调原只保存 NSError 裸指针（autoreleasepool 排空后悬空），改为回调内转成 String、成功对象 retain；Condvar 改 wait_timeout_while 防虚假唤醒误报超时。V1 指针单元点击坐标 (300,300) 在 VS Code 实际窗口 (4120,320) 之外，证据无效，已用 V3（坐标取自 axread 返回的元素 frame）复测替代。T5b 并发单元输入阶段无重叠（sleep 800ms > 用户命令总耗时 664ms）且落盘串缺尾字符，已用 T5b2 重做替代。

无干扰与并发：

- 全程 watch 采样（T1/T2/T3/T5/T5b/T6b/V1/C1 各单元）：frontmost 始终为用户侧应用（Terminal 或 TextEdit），鼠标坐标不变，剪贴板 changeCount 与内容 hash 不变；目标动作均配独立事实（文件 md5/mtime、SCK 窗口标题、AX 全文、截图像素）。
- 异目标并发（T5b2，替代证据无效的 T5b：原实验输入阶段无重叠且落盘串缺尾字符）：用户侧 Terminal session 输入与 agent 侧 TextEdit 定向按键各发 200 字符并发，墙钟区间重叠 211.8ms / 215ms；user-typed.txt 逐字等于发送串，TextEdit AX 全文含完整 agent 串，互不串扰。watch 记录 frontmost 始终为 Terminal。
- 同目标接管竞争（T6b）：TextEdit 前台时用户侧 session 输入与 agent 侧定向输入并发三轮，全文证据显示两路事件按事件粒度交错落地（如 `USER-5-…AGENT-5-…uuu…aaa…`），无 OS 层仲裁、无丢失、无错乱——用户接管同一目标时没有系统级互斥。该单元只证明竞争会持续交错；agent 层协作停止/取消本身未验证（见遗留缺口）。

线程要求：`capture`/`keys`/`axread` 的 `--spawn`（新建线程）变体全部成功（截图 38575 字节、按键 +15 字符经 AX 复验、AX 读一致），ScreenCaptureKit 单帧截图、AX、CGEventPostToPid 均无主线程要求。

签名/进程归属：探针为 adhoc linker-signed（无 TeamID、无 Info.plist），TCC 授权跟随责任进程而非二进制 cdhash；未授权 denied 态未在本机验证（revoke 会改动用户 TCC 配置，不安全）。锁屏环境未执行，见 [CU-24](cu-24-macos-locked-use.md)，不记为支持。

对后续任务的设计含义：

- 无干扰动作集按应用×动作限定：SCK 窗口截图与 AX 读取在三类应用的遮挡与最小化态均可用；AX 语义写/按压则按族分裂——TextEdit 语义写两态可用、Chrome 页面 AXPress 两态可用（含 JS 执行与导航）、VS Code 编辑器语义写静默无效。指针动作三类应用后台均无效，无 AX 控件的目标（画布、自定义渲染）无法后台操作。
- 文本注入按应用族分裂：AppKit 可直接定向按键；Chromium 系（Electron/浏览器）后台按键无效；VS Code 编辑器语义写无效，只能前台或经其扩展/CLI 通道；Chrome 页面文本控件（textarea）的语义写未验证，不能记为可行。
- 窗口布景：Chromium 系忽略 AXPosition，遮挡场景不能用 AX 移动其窗口。

遗留缺口：真实人工在场验收（用户侧输入由探针模拟）；Chrome textarea 语义写未验证；用户接管后的协作停止/取消行为未验证（T6b 只证明竞争会继续交错）；VS Code 后台菜单 AXPress 的实际效果未与 auto-save 区分；未授权 denied 态与锁屏未验。ROADMAP 状态：等待人工验收。
