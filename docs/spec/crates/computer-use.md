# pawork-computer-use

> 独立 computer use 库，2026-09-17 按用户授权新增，并按“不干扰本机键鼠”的要求改为隔离桌面。生产经 tools → Host；不改 GUI wire 或 domain 事件。

## 1. 职责与边界

纯 Rust 通过 RFB 3.8 操作专用容器内的 Xvnc 虚拟桌面。所有截图和输入都在独立显示器，不接触本机 HID、屏幕、焦点或剪贴板；不存在 macOS 回退实现。应用须运行于该环境，不能直接控制宿主已有应用。无 Pawork 内部依赖、GPUI、Provider、shell、自有 JS Runtime 或模型指定的路径/网络端点。容器需显式启动，库不管理 Docker 进程。

CU-02 起新增 `target` 模块：本机后台目标、观测与动作能力的有类型契约（纯数据类型与校验逻辑，不触平台 API、不触网）。本机探针与动作派发属下游任务（CU-03+），尚未接入生产路径与 tools；GUI 不直接访问。

CU-16 起新增 `approval` 模块：应用与网站范围的 Host 审批契约。授权把 `TargetIdentity`（整应用或浏览器内单源网站）绑定 workspace/run `Scope`，由宿主在显式用户审批后签发 once / for-run 授权；`TargetRegistry` 在绑定、观测、校验与派发路径上先查台账授权，未授权、已撤销、跨目标与受保护目标先于任何 probe 调用拒绝；网站目标另经 `NativeProbe::current_origin` 接缝复核窗口当前页面身份（含宿主捕获前预检 `require_authorized`），同窗口跨源导航即拒。Pawork 自身 UI 与 OS 权限界面内置为受保护目标。OS 权限（Screen Recording / Accessibility / Automation）与 Pawork 目标授权分离：契约不授予、不变更系统权限，缺失由 CU-03+ 探针如实报告。

## 2. 模块树

`src/lib.rs` 定义 Action / Input / Backend / Computer、观察身份、坐标转换、进程内串行、取消与定向测试。`src/rfb.rs` 为固定 `127.0.0.1:5905` 的 RFB 客户端、raw framebuffer → JPEG、虚拟键鼠事件。`src/target.rs` 为 CU-02 本机后台契约层：目标身份（AppIdentity / ProcessInstance / WindowIdentity / Scope）、宿主句柄注册表 `TargetRegistry`（CU-16 起内置 `TargetAuthorizer`，授权闸接入全部 gated 路径）、观测几何与坐标换算、`NativeActionKind` 后台能力矩阵与 `TargetError` 拒绝语义、`NativeProbe` 宿主接缝；全部为纯数据 + 校验，单元测试用 fake probe。`src/approval.rs` 为 CU-16 目标审批契约：`TargetIdentity`（整应用或单源网站）、`GrantKind`（once / for-run）、`TargetAuthorizer`（授权台账、受保护 bundle、撤销）；同为纯数据 + 校验。`desktop/` 提供独立桌面的 Dockerfile / compose / 启动脚本；`examples/isolated_probe.rs` 为隔离桌面人工 JSON 验收入口；`examples/macos_background_probe.rs` 为 CU-01 本机后台可行性人工探针（macOS-only，公开 API：ScreenCaptureKit / AX / CGEventPostToPid，结论见 [CU-01 完成记录](../../plan/cu-01-background-feasibility.md)），不参与产品路径。

## 3. 对外 API 面

`Computer::isolated()` 惰性连接专用桌面；`new(Arc<dyn Backend>)` 供宿主自定义（其隔离由宿主保证）。`execute(scope, action, cancelled)` 为同步调用。`status` 返回连接的 capture/input 可用性；`screenshot` 返回 Observation（id、连接 session_id、虚拟宽高、图像宽高）和 JPEG。输入动作携带 observation_id：click、move、drag、scroll、type_text、key。受限键名与 command/shift/control/option；Linux 快捷键用 control，command=Super，option=Alt。scroll 正值右/下，每 40 像素换算一步滚轮。

`target` 模块对外面（契约，尚无生产调用方）：`TargetRegistry::bind_window / begin_observation / issue_element / validate_window / consume_observation / consume_element`；句柄 `WindowHandle / ObservationHandle / ElementHandle` 为 serde 透明字符串（`{w,o,e}-<pid>-<宿主启动纳秒>-<进程级序号>`，命名空间保证 Host 重启且 PID 复用时新值永不撞旧值），模型只能回传宿主签发值，不能提交裸 PID、绝对路径或网络端点；`TargetObservation` 携带 Environment（`native_background` / `isolated_desktop`）、窗口句柄、图像像素与窗口点双尺寸；`image_to_window` 把模型图像像素换算为窗口坐标；`NativeActionKind` 十动作词汇（窗口截图、AX 读、AX 语义写、AX 按压、定向文本/键、菜单快捷键、菜单栏按压、指针、窗口移动、窗口缩放）配 `background_capability`（CU-01 实测矩阵，按应用族 × 动作 × 最小化态返回 Supported / Unsupported / Unverified）与 `require_background` 闸；`TargetError` 十三变体覆盖全部拒绝语义（CU-16 增 `NotAuthorized` / `ForbiddenTarget` / `SiteChanged`）。

`approval` 模块对外面（CU-16，契约，尚无生产调用方）：`TargetIdentity`（serde `tag="kind"` snake_case）两变体——`application`（整应用，bundle id + 族）与 `website`（浏览器内单源网站；构造要求 browser 族，origin 归一化为小写 `http(s)://host[:port]`，host 限域名字符且含字母数字，端口限 1..=65535 并规范化前导零，拒路径 / 查询 / 片段 / userinfo / 空 host / 非法端口，IPv6 字面量暂按 fail-closed 拒绝）；`TargetIdentity::validate` 在授权入口（`grant / check / spend` 与 `bind_window`）统一复核，公开枚举字段与派生反序列化不能绕过构造校验。`GrantKind::{Once, ForRun}` 对应宿主 `ApprovedOnce` / `ApprovedForRun` 决定；`TargetAuthorizer::new(protected_bundle_ids)` 在永远内置的 `MINIMUM_PROTECTED_BUNDLES`（Pawork 自身 UI `dev.pawork.desktop` 与 macOS 权限界面 systempreferences / securityagent / usernotificationcenter / coreservicesuiagent，大小写不敏感、宿主只能加不能减）之上追加宿主保护集，`grant / revoke / revoke_scope / check / spend`。`TargetRegistry` 相应增 `with_authorizer / authorizer / grant / revoke / revoke_scope / require_authorized`（宿主预检闸：连接、启动、捕获、读树前调用，非消耗；先查台账，网站目标再复核窗口当前 origin，跨源 / 不可得即 SiteChanged）；`bind_window` 收 `TargetIdentity`；`ValidatedWindow.app` 更名 `target: TargetIdentity`；`NativeProbe` 增 `current_origin` 页面身份接缝；`TargetError` 十三变体（CU-16 增 `NotAuthorized` / `ForbiddenTarget` / `SiteChanged`）。

## 4. 关键行为与语义

坐标从 JPEG 左上角开始，严格在 `[0,width) × [0,height)`，按虚拟尺寸缩放。截图最长 1280、≤512 KiB；raw framebuffer 单边 ≤4096、总像素 ≤8M，响应字节、矩形、消息与 8 秒 I/O 均有上限。每次全量截图覆盖全部像素后才返回，禁止残帧伪装截图。观察绑定 scope、连接身份和尺寸，60 秒失效；输入前即消费，错误不允许重用。断线后新连接身份使旧观察失效；不自动重试输入。取消/错误时尝试释放，传输失败断开由 Xvnc 释放客户端状态。成功仅代表投递，实际效果需截图复验。

target 契约沿用并细化同一租约模型：观测与元素句柄 60 秒 TTL、一次性消费——消费元素同时作废父观测与兄弟元素句柄；校验通过即消费，后续派发失败不回收。每次使用按固定顺序校验：取消 → 存在性（伪造/未知 → UnknownHandle，宿主重启后全部旧句柄落入此类）→ Scope（跨 Run / 跨 workspace → CrossRun）→ 过期或已消费 → StaleHandle → 进程实例（退出或重启 → ProcessRestarted）→ 窗口代际（销毁或标识符复用 → WindowReplaced）→ AX 树版本（树前进 → TreeChanged，AX 观测消费与元素句柄同校）；坐标非有限值或越出图像边界（含边缘本身）→ OutOfBounds。元素句柄只能由 AxTree 观测签发。后台能力以 CU-01 实测为唯一依据：仅 Supported 放行，Unsupported 与 Unverified 同拒（BackgroundUnsupported）且永不回退全局输入；指针动作三类应用族后台一律拒绝；AppKit 定向键在窗口最小化时拒绝；AppKit AXPress 与最小化态窗口移动/缩放未单独实测，按 Unverified 拒绝。用户接管同一目标没有系统级仲裁（CU-01 T6b），取消是唯一停止机制。

CU-16 起校验链在 StaleHandle 之后、进程实例 probe 之前插入授权裁决：绑定、观测签发、窗口校验与动作消费均先查 `(Scope, TargetIdentity)` 授权（无授权 / 已撤销 / 跨目标 → NotAuthorized，受保护 bundle → ForbiddenTarget），台账拒绝零 probe 调用（回归以计数钉住）；宿主捕获前预检 `require_authorized` 同序执行——先查台账、再复核网站目标当前 origin，跨源或 origin 不可得即 SiteChanged，拒绝时宿主不得连接 / 捕获 / 读取目标；真实平台后端的捕获 / 输入访问以该预检接线为前提，随 CU-03+ 落地后验证。once 授权只在其余校验全部通过后、动作真正放行时才消耗——绑定需要有效授权但不消耗，陈旧目标或拒绝路径不烧掉用户的单次批准；一次观测或一次输入各为一个动作。元素句柄签发不重复查授权（其父观测已带授权），撤销在观测与派发之间到达时由 `consume_observation` / `consume_element` 的先于 probe 复查阻断。授权只覆盖被批准目标：整应用授权不覆盖其内任何网站，单源网站授权不覆盖浏览器与其它源；网站目标在绑定与每次 live 校验时经 `NativeProbe::current_origin` 复核窗口当前 origin（归一化比较，尾斜杠 / 大小写 / 端口拼写不能绕行），同窗口跨源导航或 origin 不可得一律 SiteChanged，先于派发拒绝。身份在授权入口统一 `validate` 复核，公开枚举与反序列化不能绕过构造校验。授权仅存内存，随 run 结束（`revoke_scope`）或 Host 退出消亡；已执行动作是历史，撤销不回写。

## 5. 依赖与 feature

serde / thiserror / image（仅 JPEG，已有 workspace lock 版本）；网络为 std::net。无平台专属框架和 `pawork-*` 依赖。独立 Cargo 元数据不继承 workspace；`publish=false`，License 待定。隔离桌面运行依赖 Docker / Xvnc，不改变 CLI / Desktop 的纯 Rust 构建链。macOS 探针 example 使用 target-gated dev-dependencies（objc / block / cocoa / core-foundation / core-graphics / libc，均复用 workspace 已锁定版本），仅 macOS 开发目标（测试与 example 构建）使用，不进入生产库依赖面，库本体仍无平台框架。

## 6. 红线与不变量

Pawork 集成通过 [tools](tools.md) 复用 ExternalPlugin / requires_approval / untrusted 与 ReadOnly 拒绝；构造不触网，审批在连接前。固定 loopback 端口、RFB 3.8、桌面名匹配只是配置检查，不是远程身份认证或隔离证明；部署必须使用所附虚拟桌面，不转发物理屏幕。容器不挂载宿主目录/设备/桌面/剪贴板，只发布 loopback 端口，拒绝第二个 viewer。服务器 None 认证仅限这个本地部署。环境缺失/断线直接失败，没有回退本机路径。截图为未信任数据，仍持久化并发送给模型，不提供通用视觉脱敏。应用焦点变化仍需新截图验证；隔离解决本机输入争抢，不保证容器完全不消耗宿主资源。

target 契约层不变量：契约代码不调用任何平台 API（活跃状态经 `NativeProbe` 宿主注入）；句柄只能由 `TargetRegistry` 签发，模型输入永远按未知句柄拒绝兜底；无后台实测支持的动作直接拒绝，不存在全局输入、前台激活或事后恢复状态的隐式降级。

审批契约层不变量（CU-16）：授权只能由宿主在显式用户审批后签发——模型侧只能触发检查，永不能签发或扩大授权；自动审批 resolver 不能满足 Policy AskUser，因此永远产生不了授权，不另造审批框架。台账授权拒绝（NotAuthorized、ForbiddenTarget）先于一切 probe 调用（回归以 probe 调用计数钉住）；SiteChanged 裁决须经 `current_origin` 接缝（不触碰进程实例与捕获后端）；生产捕获与 Policy 接线待 CU-03+ 验证。`MINIMUM_PROTECTED_BUNDLES` 内置 Pawork 自身 UI 与 macOS 权限界面（大小写不敏感、宿主只能加不能减、默认构造即生效）：受保护目标永不可授权、永不可绑定，Agent 无法经 Pawork 审批界面或系统权限界面自我批准；契约不存在任何变更 OS Screen Recording / Accessibility / Automation 权限的 API，系统权限缺失只能如实报告。网站目标 origin 不可得即 fail-closed（SiteChanged）。逐目标持久授权不存在：持久允许仅走已裁决的 ApprovalMode / workspace trust 设置口径，新增逐目标持久授权须先过 ADR。

## 7. 测试与 golden 资产

lib 回归验证坐标映射、单次消费、权限、取消、scope/连接变化、过期、非法坐标/未知字段/控制字符。RFB 定向测试覆盖握手→raw 截图→输入与关键拒绝路径；组合键在每个修饰键和主键发送前检查取消，既有 wire 回归确认 Control 按下后取消不会继续发送 S / Shift，且仍释放 Control。tools 证明审批拒绝前零后端访问、图片返回及序列化；app 证明审批/持久化/resume 不重执行；Providers 保留三协议图片。实际隔离桌面验收结果见路线图，历史 macOS HID 验收不算当前后端通过证据。

`target.rs` 十二项回归钉住契约拒绝语义：CU-01 能力矩阵（指针全族后台拒绝、AppKit 最小化拒定向键、Chromium 语义写拒绝、AppKit AXPress 与最小化窗口移动/缩放按 Unverified、Unverified 与 Unsupported 同拒）、观测一次性消费与跨 Run / 跨 workspace 拒绝（拒绝不消耗租约）、伪造句柄 UnknownHandle 与句柄 JSON 往返、进程级命名空间格式与跨 Host 实例同 PID 旧句柄拒绝、进程退出/重启、窗口销毁与标识符复用、AX 树变化（观测消费与元素句柄同校，截图观测不受树变化影响）、元素消费连带作废父观测与兄弟句柄（反向同样成立）、60 秒过期、取消先于消费、图像坐标映射与越界（含 NaN / 无穷 / 边缘）。

`approval.rs` 与 `target.rs` 的 CU-16 回归钉住目标审批语义：origin 归一化与非法 origin / 非浏览器族网站拒绝（含空 host、非法端口、非域名字符、IPv6 字面量，端口前导零归一）、`TargetIdentity` serde 往返、手工构造与反序列化身份在授权入口统一拒绝、默认构造内置保护 Pawork UI 与 macOS 权限界面（大小写绕行无效）、授权仅覆盖被批准目标与被批准 scope（跨应用、跨网站源、跨 run / workspace 均拒绝）、once 恰好消耗一次且 presence 检查不消耗、撤销只影响被撤销目标且可重新授权、`revoke_scope` 只清本 run、受保护 bundle 永不可授权；注册表层覆盖未授权 / 跨目标绑定先于 probe 拒绝（probe 调用计数为零）、once 不被绑定或陈旧目标提前消耗（先观测后输入各需一次授权）、撤销阻断在途租约派发且先于 probe、重新授权后在途租约可放行、已消费历史不被回写、受保护目标授权与绑定均先于 probe 拒绝、网站目标绑定钉住当前 origin 且同窗口跨源导航在捕获前预检 / 观测 / 窗口校验 / 派发四路按 SiteChanged 拒绝（origin 不可得 fail-closed，导航回已授权源后预检与在途租约恢复可用）、`require_authorized` 预检与 gated 路径一致。

## 8. 相关文档

[部署说明](../../../crates/computer-use/README.md) · [待验收项](../../ROADMAP.md) · [设计](../../design.md#computer-use-首版) · [参照调研](../../references.md#computer-use-实现调研2026-09-17) · [架构](../../architecture.md) · [安全](../security.md) · [CU-02 任务文档](../../plan/cu-02-target-contracts.md)
