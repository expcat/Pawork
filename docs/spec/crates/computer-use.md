# pawork-computer-use

> 独立 computer use 库，2026-09-17 按用户授权新增，并按“不干扰本机键鼠”的要求改为隔离桌面。生产经 tools → Host；不改 GUI wire 或 domain 事件。

## 1. 职责与边界

纯 Rust 通过 RFB 3.8 操作专用容器内的 Xvnc 虚拟桌面。所有截图和输入都在独立显示器，不接触本机 HID、屏幕、焦点或剪贴板；不存在 macOS 回退实现。应用须运行于该环境，不能直接控制宿主已有应用。无 Pawork 内部依赖、GPUI、Provider、shell、自有 JS Runtime 或模型指定的路径/网络端点。容器需显式启动，库不管理 Docker 进程。

## 2. 模块树

`src/lib.rs` 定义 Action / Input / Backend / Computer、观察身份、坐标转换、进程内串行、取消与定向测试。`src/rfb.rs` 为固定 `127.0.0.1:5905` 的 RFB 客户端、raw framebuffer → JPEG、虚拟键鼠事件。`desktop/` 提供独立桌面的 Dockerfile / compose / 启动脚本；`examples/isolated_probe.rs` 为人工 JSON 验收入口。

## 3. 对外 API 面

`Computer::isolated()` 惰性连接专用桌面；`new(Arc<dyn Backend>)` 供宿主自定义（其隔离由宿主保证）。`execute(scope, action, cancelled)` 为同步调用。`status` 返回连接的 capture/input 可用性；`screenshot` 返回 Observation（id、连接 session_id、虚拟宽高、图像宽高）和 JPEG。输入动作携带 observation_id：click、move、drag、scroll、type_text、key。受限键名与 command/shift/control/option；Linux 快捷键用 control，command=Super，option=Alt。scroll 正值右/下，每 40 像素换算一步滚轮。

## 4. 关键行为与语义

坐标从 JPEG 左上角开始，严格在 `[0,width) × [0,height)`，按虚拟尺寸缩放。截图最长 1280、≤512 KiB；raw framebuffer 单边 ≤4096、总像素 ≤8M，响应字节、矩形、消息与 8 秒 I/O 均有上限。每次全量截图覆盖全部像素后才返回，禁止残帧伪装截图。观察绑定 scope、连接身份和尺寸，60 秒失效；输入前即消费，错误不允许重用。断线后新连接身份使旧观察失效；不自动重试输入。取消/错误时尝试释放，传输失败断开由 Xvnc 释放客户端状态。成功仅代表投递，实际效果需截图复验。

## 5. 依赖与 feature

serde / thiserror / image（仅 JPEG，已有 workspace lock 版本）；网络为 std::net。无平台专属框架和 `pawork-*` 依赖。独立 Cargo 元数据不继承 workspace；`publish=false`，License 待定。隔离桌面运行依赖 Docker / Xvnc，不改变 CLI / Desktop 的纯 Rust 构建链。

## 6. 红线与不变量

Pawork 集成通过 [tools](tools.md) 复用 ExternalPlugin / requires_approval / untrusted 与 ReadOnly 拒绝；构造不触网，审批在连接前。固定 loopback 端口、RFB 3.8、桌面名匹配只是配置检查，不是远程身份认证或隔离证明；部署必须使用所附虚拟桌面，不转发物理屏幕。容器不挂载宿主目录/设备/桌面/剪贴板，只发布 loopback 端口，拒绝第二个 viewer。服务器 None 认证仅限这个本地部署。环境缺失/断线直接失败，没有回退本机路径。截图为未信任数据，仍持久化并发送给模型，不提供通用视觉脱敏。应用焦点变化仍需新截图验证；隔离解决本机输入争抢，不保证容器完全不消耗宿主资源。

## 7. 测试与 golden 资产

lib 回归验证坐标映射、单次消费、权限、取消、scope/连接变化、过期、非法坐标/未知字段/控制字符。RFB 定向测试覆盖握手→raw 截图→输入与关键拒绝路径；组合键在每个修饰键和主键发送前检查取消，既有 wire 回归确认 Control 按下后取消不会继续发送 S / Shift，且仍释放 Control。tools 证明审批拒绝前零后端访问、图片返回及序列化；app 证明审批/持久化/resume 不重执行；Providers 保留三协议图片。实际隔离桌面验收结果见路线图，历史 macOS HID 验收不算当前后端通过证据。

## 8. 相关文档

[部署说明](../../../crates/computer-use/README.md) · [实施与验收](../../ROADMAP.md#computer-use首版2026-09-17) · [设计](../../design.md#computer-use首版) · [参照调研](../../references.md#computer-use实现调研2026-09-17) · [架构](../../architecture.md) · [安全](../security.md)
