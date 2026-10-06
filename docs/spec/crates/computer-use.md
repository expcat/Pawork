# pawork-computer-use

> 独立 computer use 库，2026-09-17 按用户授权新增，并按“不干扰本机键鼠”的要求改为隔离桌面。生产经 tools → Host；不改 GUI wire 或 domain 事件。

## 1. 职责与边界

纯 Rust 通过 RFB 3.8 操作专用容器内的 Xvnc 虚拟桌面。所有截图和输入都在独立显示器，不接触本机 HID、屏幕、焦点或剪贴板；不存在 macOS 回退实现。应用须运行于该环境，不能直接控制宿主已有应用。无 Pawork 内部依赖、GPUI、Provider、shell、自有 JS Runtime 或模型指定的路径/网络端点。容器需显式启动，库不管理 Docker 进程。

CU-02 起新增 `target` 模块：本机后台目标、观测与动作能力的有类型契约（纯数据类型与校验逻辑，不触平台 API、不触网）。本机探针与动作派发属下游任务（CU-03+），尚未接入生产路径与 tools；GUI 不直接访问。

## 2. 模块树

`src/lib.rs` 定义 Action / Input / Backend / Computer、观察身份、坐标转换、进程内串行、取消与定向测试。`src/rfb.rs` 为固定 `127.0.0.1:5905` 的 RFB 客户端、raw framebuffer → JPEG、虚拟键鼠事件。`src/target.rs` 为 CU-02 本机后台契约层：目标身份（AppIdentity / ProcessInstance / WindowIdentity / Scope）、宿主句柄注册表 `TargetRegistry`、观测几何与坐标换算、`NativeActionKind` 后台能力矩阵与 `TargetError` 拒绝语义、`NativeProbe` 宿主接缝；全部为纯数据 + 校验，单元测试用 fake probe。`desktop/` 提供独立桌面的 Dockerfile / compose / 启动脚本；`examples/isolated_probe.rs` 为隔离桌面人工 JSON 验收入口；`examples/macos_background_probe.rs` 为 CU-01 本机后台可行性人工探针（macOS-only，公开 API：ScreenCaptureKit / AX / CGEventPostToPid，结论见 [CU-01 完成记录](../../plan/cu-01-background-feasibility.md)），不参与产品路径。

## 3. 对外 API 面

`Computer::isolated()` 惰性连接专用桌面；`new(Arc<dyn Backend>)` 供宿主自定义（其隔离由宿主保证）。`execute(scope, action, cancelled)` 为同步调用。`status` 返回连接的 capture/input 可用性；`screenshot` 返回 Observation（id、连接 session_id、虚拟宽高、图像宽高）和 JPEG。输入动作携带 observation_id：click、move、drag、scroll、type_text、key。受限键名与 command/shift/control/option；Linux 快捷键用 control，command=Super，option=Alt。scroll 正值右/下，每 40 像素换算一步滚轮。

`target` 模块对外面（契约，尚无生产调用方）：`TargetRegistry::bind_window / begin_observation / issue_element / validate_window / consume_observation / consume_element`；句柄 `WindowHandle / ObservationHandle / ElementHandle` 为 serde 透明字符串（`{w,o,e}-<pid>-<宿主启动纳秒>-<进程级序号>`，命名空间保证 Host 重启且 PID 复用时新值永不撞旧值），模型只能回传宿主签发值，不能提交裸 PID、绝对路径或网络端点；`TargetObservation` 携带 Environment（`native_background` / `isolated_desktop`）、窗口句柄、图像像素与窗口点双尺寸；`image_to_window` 把模型图像像素换算为窗口坐标；`NativeActionKind` 十动作词汇（窗口截图、AX 读、AX 语义写、AX 按压、定向文本/键、菜单快捷键、菜单栏按压、指针、窗口移动、窗口缩放）配 `background_capability`（CU-01 实测矩阵，按应用族 × 动作 × 最小化态返回 Supported / Unsupported / Unverified）与 `require_background` 闸；`TargetError` 十变体覆盖全部拒绝语义。

## 4. 关键行为与语义

坐标从 JPEG 左上角开始，严格在 `[0,width) × [0,height)`，按虚拟尺寸缩放。截图最长 1280、≤512 KiB；raw framebuffer 单边 ≤4096、总像素 ≤8M，响应字节、矩形、消息与 8 秒 I/O 均有上限。每次全量截图覆盖全部像素后才返回，禁止残帧伪装截图。观察绑定 scope、连接身份和尺寸，60 秒失效；输入前即消费，错误不允许重用。断线后新连接身份使旧观察失效；不自动重试输入。取消/错误时尝试释放，传输失败断开由 Xvnc 释放客户端状态。成功仅代表投递，实际效果需截图复验。

target 契约沿用并细化同一租约模型：观测与元素句柄 60 秒 TTL、一次性消费——消费元素同时作废父观测与兄弟元素句柄；校验通过即消费，后续派发失败不回收。每次使用按固定顺序校验：取消 → 存在性（伪造/未知 → UnknownHandle，宿主重启后全部旧句柄落入此类）→ Scope（跨 Run / 跨 workspace → CrossRun）→ 过期或已消费 → StaleHandle → 进程实例（退出或重启 → ProcessRestarted）→ 窗口代际（销毁或标识符复用 → WindowReplaced）→ AX 树版本（树前进 → TreeChanged，AX 观测消费与元素句柄同校）；坐标非有限值或越出图像边界（含边缘本身）→ OutOfBounds。元素句柄只能由 AxTree 观测签发。后台能力以 CU-01 实测为唯一依据：仅 Supported 放行，Unsupported 与 Unverified 同拒（BackgroundUnsupported）且永不回退全局输入；指针动作三类应用族后台一律拒绝；AppKit 定向键在窗口最小化时拒绝；AppKit AXPress 与最小化态窗口移动/缩放未单独实测，按 Unverified 拒绝。用户接管同一目标没有系统级仲裁（CU-01 T6b），取消是唯一停止机制。

## 5. 依赖与 feature

serde / thiserror / image（仅 JPEG，已有 workspace lock 版本）；网络为 std::net。无平台专属框架和 `pawork-*` 依赖。独立 Cargo 元数据不继承 workspace；`publish=false`，License 待定。隔离桌面运行依赖 Docker / Xvnc，不改变 CLI / Desktop 的纯 Rust 构建链。macOS 探针 example 使用 target-gated dev-dependencies（objc / block / cocoa / core-foundation / core-graphics / libc，均复用 workspace 已锁定版本），仅 macOS 开发目标（测试与 example 构建）使用，不进入生产库依赖面，库本体仍无平台框架。

## 6. 红线与不变量

Pawork 集成通过 [tools](tools.md) 复用 ExternalPlugin / requires_approval / untrusted 与 ReadOnly 拒绝；构造不触网，审批在连接前。固定 loopback 端口、RFB 3.8、桌面名匹配只是配置检查，不是远程身份认证或隔离证明；部署必须使用所附虚拟桌面，不转发物理屏幕。容器不挂载宿主目录/设备/桌面/剪贴板，只发布 loopback 端口，拒绝第二个 viewer。服务器 None 认证仅限这个本地部署。环境缺失/断线直接失败，没有回退本机路径。截图为未信任数据，仍持久化并发送给模型，不提供通用视觉脱敏。应用焦点变化仍需新截图验证；隔离解决本机输入争抢，不保证容器完全不消耗宿主资源。

target 契约层不变量：契约代码不调用任何平台 API（活跃状态经 `NativeProbe` 宿主注入）；句柄只能由 `TargetRegistry` 签发，模型输入永远按未知句柄拒绝兜底；无后台实测支持的动作直接拒绝，不存在全局输入、前台激活或事后恢复状态的隐式降级。

## 7. 测试与 golden 资产

lib 回归验证坐标映射、单次消费、权限、取消、scope/连接变化、过期、非法坐标/未知字段/控制字符。RFB 定向测试覆盖握手→raw 截图→输入与关键拒绝路径；组合键在每个修饰键和主键发送前检查取消，既有 wire 回归确认 Control 按下后取消不会继续发送 S / Shift，且仍释放 Control。tools 证明审批拒绝前零后端访问、图片返回及序列化；app 证明审批/持久化/resume 不重执行；Providers 保留三协议图片。实际隔离桌面验收结果见路线图，历史 macOS HID 验收不算当前后端通过证据。

`target.rs` 十二项回归钉住契约拒绝语义：CU-01 能力矩阵（指针全族后台拒绝、AppKit 最小化拒定向键、Chromium 语义写拒绝、AppKit AXPress 与最小化窗口移动/缩放按 Unverified、Unverified 与 Unsupported 同拒）、观测一次性消费与跨 Run / 跨 workspace 拒绝（拒绝不消耗租约）、伪造句柄 UnknownHandle 与句柄 JSON 往返、进程级命名空间格式与跨 Host 实例同 PID 旧句柄拒绝、进程退出/重启、窗口销毁与标识符复用、AX 树变化（观测消费与元素句柄同校，截图观测不受树变化影响）、元素消费连带作废父观测与兄弟句柄（反向同样成立）、60 秒过期、取消先于消费、图像坐标映射与越界（含 NaN / 无穷 / 边缘）。

## 8. 相关文档

[部署说明](../../../crates/computer-use/README.md) · [待验收项](../../ROADMAP.md) · [设计](../../design.md#computer-use-首版) · [参照调研](../../references.md#computer-use-实现调研2026-09-17) · [架构](../../architecture.md) · [安全](../security.md) · [CU-02 任务文档](../../plan/cu-02-target-contracts.md)
