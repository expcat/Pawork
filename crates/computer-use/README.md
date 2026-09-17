# pawork-computer-use

纯 Rust 隔离桌面观察与输入库，无 Pawork 内部依赖。所有截图和键鼠输入经 RFB 3.8 进入专用 Linux 虚拟显示器，不读取本机屏幕、不发送本机 HID 事件、不访问本机焦点或剪贴板。宿主无需 Screen Recording / Accessibility 权限。

## 启动独立桌面

需要已经运行的 Docker Engine / Docker Desktop：

```sh
docker compose -f crates/computer-use/desktop/compose.yaml up -d --build
```

若网络无法连接 Alpine 官方源，可先使用 `docker compose -f crates/computer-use/desktop/compose.yaml build --build-arg ALPINE_MIRROR=https://mirrors.aliyun.com/alpine` 完成构建，再执行 `up -d`；默认仍使用官方源。

容器内 Xvnc 创建自己的显示器、指针、键盘和应用会话。只映射 `127.0.0.1:5905`，不挂载宿主目录、Docker socket、设备或桌面会话。应用必须运行在容器内；不能直接操作 macOS 已打开的应用。资源限制减少竞争，但不承诺零 CPU / 内存影响。停止用同一 compose 文件的 `stop`；桌面 home/tmp 为临时存储，停止后其中的数据不保留，重要结果须先由操作者取出。

库仅支持这个固定本地端点，连接在首个已授权调用时建立。RFB 版本、桌面名称 `Pawork-Isolated` 与尺寸检查用于发现误接；名称不是安全证明，隔离保证来自所附部署的 Xvnc 和容器边界。不要把这个端口转发到用户的物理桌面。仅本地 None 认证，不支持连接任意公网 VNC。服务器拒绝第二个控制客户端，避免两个 Host 争抢同一虚拟桌面；库不会抢断已有连接。环境不可用直接失败，**无本机桌面回退路径**。

## 宿主集成

宿主先授权，再调用 `Computer::execute(scope, action, cancelled)`。`scope` 为运行身份；先 `screenshot` 获取 `observation_id`，随后一个输入消费该观察。坐标是返回 JPEG 的像素，左上角为原点；按虚拟显示器大小换算。每次输入后重新截图确认效果。

```rust,no_run
use pawork_computer_use::{Action, Computer};
let computer = Computer::isolated(); // 不在构造时连接网络
let screen = computer.execute("authorized-run", Action::Screenshot {}, &|| false)?;
let observation = screen.observation.unwrap();
// Use observation.observation_id in one approved input, then observe again.
# Ok::<(), pawork_computer_use::Error>(())
```

操作：`status`、`screenshot`、`click`、`move`、`drag`、`scroll`、`type_text`、`key`。Linux 应用快捷键使用 `control`；`command` 映射 Super，`option` 映射 Alt。文字用 Unicode keysym，不经过剪贴板。scroll 以每 40 像素约等于一个滚轮步进换算。`status` 的 `capture/input` 表示隔离连接就绪，不是 macOS TCC 授权。

截图最长 1280 像素、JPEG ≤512 KiB；虚拟 framebuffer ≤4096 单边且 ≤8M 像素，原始响应、矩形、附带消息和总 I/O 时间有界。观察绑定 scope / 连接身份 / 尺寸，60 秒有效。断线和失败不重试输入；重新连接必须重新截图。取消仍尝试释放按键/按钮，断线由 Xvnc 清理客户端输入。

`examples/isolated_probe.rs` 逐行读取人工输入的 JSON，将截图写入启动参数指定的路径；模型接口不接受路径、网络地址、shell 或容器配置。运行：

```sh
cargo run -p pawork-computer-use --example isolated_probe -- /tmp/computer-proof.jpg
```

`Backend` 是宿主自行注入与测试的接口；自定义 backend 的隔离由其实现者负责。默认 `Computer::isolated()` 只包含上述 RFB 实现。独立 Cargo 元数据可打包校验；当前 `publish=false`，License 待定，未发布 crates.io。
