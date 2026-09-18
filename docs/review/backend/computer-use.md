# pawork-computer-use Review

> 独立 computer use 库（2026-09-17 按用户授权新增）：纯 Rust 经 RFB 3.8 操作专用容器内 Xvnc 虚拟桌面，截图与输入全部隔离在独立显示器，不触本机 HID / 屏幕 / 焦点 / 剪贴板。src 2 个 .rs 文件共 1 132 行（lib 574 / rfb 558，含 4 个内联测试），另有 examples/isolated_probe.rs 35 行与 desktop/ 部署三件套；无 pawork-* / GUI / Provider 依赖，生产消费方仅 pawork-tools。注意：crates/computer-use/src/rfb.rs 与 docs/spec/crates/computer-use.md 当前有用户未提交的在途改动（组合键逐键取消检查 + wire 回归），本页按工作区现状记录。

## 1. 职责与边界

纯 Rust 通过 RFB 3.8 操作专用容器内的 Xvnc 虚拟桌面。所有截图和输入都在独立显示器，不接触本机 HID、屏幕、焦点或剪贴板；不存在 macOS 回退实现。应用须运行于该环境，不能直接控制宿主已有应用。无 Pawork 内部依赖、GPUI、Provider、shell、自有 JS Runtime 或模型指定的路径 / 网络端点。容器需显式启动（desktop/compose.yaml），库不管理 Docker 进程。Pawork 集成经 tools 包复用 ExternalPlugin / requires_approval / untrusted 语义，构造不触网、审批在连接前。

## 2. 依赖关系

| 方向 | 包 | 用途 |
|---|---|---|
| 本包依赖 | 无 pawork-* | 独立库；Cargo 元数据不继承 workspace（自有 version/edition/publish=false） |
| 被依赖（生产） | pawork-tools | 唯一消费方，适配为 browser/computer 工具族之一经 Host 调度 |

| 外部 crate | 用途 |
|---|---|
| serde（derive） | Action / Point / Button 等模型 serde（deny_unknown_fields 防未知字段注入） |
| thiserror 2 | Error 枚举派生 |
| image =0.25.10（仅 jpeg feature） | raw framebuffer 缩放（Triangle）与 JPEG 编码（质量 75/50/30 逐级降级） |
| dev: serde_json | Action JSON 解析自测与 probe |

网络仅 std::net（TcpStream，固定 127.0.0.1:5905，connect 超时 1s）；无平台专属框架。

## 3. 文件清单

| 相对路径 | 行数 | 职责 |
|---|---:|---|
| src/lib.rs | 574 | Action / Input / Backend / Computer、观察身份（observation_id / scope / 连接 session / 尺寸绑定、60s 失效、单次消费）、坐标换算、进程内串行（DESKTOP_LOCK）、取消与 2 个定向测试 |
| src/rfb.rs | 558（在途） | RFB 3.8 客户端：握手（版本 / None 认证 / 共享位 / 桌面名 Pawork-Isolated 校验）、raw framebuffer 全覆盖捕获 → JPEG、虚拟键鼠事件（含逐键取消检查）、2 个 wire 级测试 |
| examples/isolated_probe.rs | 35 | 人工 JSON 验收入口（stdin 逐行 Action，截图写参数路径） |
| desktop/（Dockerfile / compose.yaml / start.sh） | — | 独立桌面部署：Xvnc + 仅发布 127.0.0.1:5905，不挂载宿主目录 / 设备 / 桌面 / 剪贴板 |
| README.md | — | 部署说明与宿主集成口径 |

## 4. 类型与方法功能列表

| 名称 | 种类 | 签名/功能 |
|---|---|---|
| `MAX_IMAGE_EDGE = 1280` / `MAX_IMAGE_BYTES = 512 KiB` | const | 截图最长边与 JPEG 字节预算 |
| `Error` | enum（6） | Unsupported / Permission / Invalid / Stale（观察过期或桌面已变）/ Cancelled / Backend(String) |
| `Button` / `Point` | enum / struct | Left/Right/Middle；`{x, y}`（f64，deny_unknown_fields） |
| `Action` | enum（serde tag="action" snake_case + deny_unknown_fields） | Status / Screenshot / Click{observation_id, x, y, button, clicks(1..=2)} / Move / Drag{from, to} / Scroll{delta_x, delta_y（非零且 ≤2000）} / TypeText{1..4096 字节无控制字符} / Key{key, modifiers ≤4 且限 command/shift/control/option} |
| `Backend` | trait | `permissions()` / `desktop()` / `capture()` / `input(input, cancelled)`——同步 seam；实现须在取消 / 错误时也配平每个按键 / 按钮的 down/up |
| `Computer::new(Arc<dyn Backend>)` | fn | 宿主自定义后端（隔离由宿主保证） |
| `Computer::isolated()` | fn | 惰性连接固定 127.0.0.1:5905 的 RFB 后端；构造不触网 |
| `execute(scope, action, cancelled)` | fn | 同步调用：全局 DESKTOP_LOCK 串行；Status 返回 permissions；Screenshot 换新观察；输入先消费观察再派发 |
| `Observation` / `Output` / `Permissions` / `Desktop` | struct | 观察身份（observation_id = pid-sequence、desktop session_id / 虚拟宽高、图像宽高）；输出聚合；capture/input 可用位；连接身份 |
| `keysym(key)` | fn（私有） | 单字符限 ASCII 小写 / 数字；命名键 return/tab/space/backspace/escape/delete/home/end/page_up/page_down/方向键 |

## 5. 关键行为与契约

- 观察即授权凭证：Screenshot 生成观察（绑定 scope、连接 session、虚拟尺寸），60 秒失效；任何输入先 `latest.take()` 单次消费（失败也不可重用）；scope 不匹配、桌面代际变化（截图 / 输入都推进 DESKTOP_GENERATION）、连接身份变化（断线重连得新 session_id）均判 Stale；输入前逐项校验坐标严格在 `[0,width) × [0,height)`（拒绝 NaN / Inf）并按虚拟尺寸缩放。
- RFB 全量截图：请求 full non-incremental framebuffer，逐矩形覆盖 `covered` 位图，全部像素覆盖后才编码返回（禁止残帧伪装截图）；无请求消息（Bell 丢弃、ServerCutText ≤64KiB 丢弃且永不触宿主剪贴板）有界处理 ≤64 条。
- 有界性：framebuffer 单边 ≤4096 且 ≤8M 像素；矩形数 ≤4096；累计响应字节 ≤8× 面积；总 I/O 预算 8 秒（IO_BUDGET，逐读写设 timeout）；JPEG 超预算按质量 75/50/30 降级重编码，仍超则失败。
- 输入语义：Linux 快捷键用 control（command=Super 0xffeb、option=Alt 0xffe9）；文字逐字符 Unicode keysym（>0xff 走 0x01000000 码位），不经剪贴板；scroll 每 40 像素一个滚轮步进（垂直 / 水平、正负方向对应 button mask 8/16/32/64）；drag 12 步插值每步 12ms。
- 取消与释放：每次输入动作在派发前与逐键 / 逐步间检查 `cancelled`；取消 / 错误仍逆序释放全部 held 键与按住的指针；传输失败直接丢弃连接（不重试输入），由 Xvnc 释放客户端状态；重连后新 session_id 使旧观察失效。
- 端点硬编码：仅 127.0.0.1:5905；RFB 3.8、共享位 =1（不抢已有 viewer，Xvnc 拒绝第二客户端）、桌面名必须逐字节等于 Pawork-Isolated——这些是配置错误检查，不是远程身份认证或隔离证明；None 认证仅限该本地部署。
- 成功仅代表投递，实际效果需新截图复验。

## 6. 测试资产

全部内联（无独立 tests/ 目录，未跑 cargo——本页为静态审查）：

| 位置 | 验证点 |
|---|---|
| lib.rs tests（2） | 观察像素 → 虚拟桌面坐标映射与缩放；观察单次消费、scope 变化、连接身份变化均 Stale；权限拒绝 / 取消 / 过期 / 非法坐标（含 NaN / Inf）/ 未知字段 / 控制字符在虚拟派发前被拒 |
| rfb.rs tests（2，在途） | 握手 → raw 截图（含丢弃 Bell / 剪贴板消息）→ Unicode 键 / 组合键 / 点击 / 取消中的 drag（仍释放按钮）wire 逐字节断言；组合键在 Control 按下后取消不发送 S / Shift 且仍释放 Control（每个修饰键与主键发送前检查取消）；错误桌面名 / 超尺寸 framebuffer / 越界矩形被拒且不发任何输入 |

examples/isolated_probe.rs 为人工隔离桌面 JSON 验收入口；实际隔离桌面验收结果记录在路线图（历史 macOS HID 验收不算当前后端通过证据）。

## 7. 协作关系

```mermaid
graph LR
  tools[pawork-tools<br/>computer 工具适配 / 审批] --> cu[pawork-computer-use]
  host[CLI Host<br/>Policy / 审批调度] --> tools
  cu --> rfb[RFB 3.8 客户端<br/>固定 127.0.0.1:5905]
  rfb --> xvnc[容器内 Xvnc 虚拟桌面<br/>desktop/compose.yaml]
  cu --> deps[serde / thiserror / image(jpeg)<br/>+ std::net]
```

