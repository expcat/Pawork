# B17 · CLI 帮助与附件检查裁决

> 状态：已完成；定向检查通过，独立 Review PASS。完成：2026-10-08。更新：2026-10-08。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决 CLI 文案一致性和低收益附件复查。

## 基线

`main`，HEAD `6f53a9cb02a898a75691625f3e5b8278c150b2a2`。开始时工作区干净。未做 B18 或 CU-10。不读用户 auth/config，不调用真实 Provider。

## 裁决

### 1. gateway 与附件帮助改为中文

只改这一族 clap 帮助，不改命令名、选项、默认值，也不翻译其它命令。

- `gateway`：本机可复用的 OpenAI 兼容模型 API。
- `gateway serve`：只在 127.0.0.1 上提供服务。`--port` 仍默认 17432，不为端口另写帮助。
- `gateway token`：按客户端签发、列出或撤销网关凭证。`issue` 的明文只显示一次，`--client` 是客户端名称；`list` 不含明文；`revoke` 按 id 撤销。这与现有 `GatewayTokenStore` 一致：只持久化摘要，列表只返回 id、client 和创建时间。
- `gateway status` / `shutdown`：查看本机网关是否在运行，停止正在运行的本机网关。停止在 Unix 上是 `kill -TERM`，在 Windows 上是 `taskkill /F`，帮助不写成单一信号。
- `chat` / `run` 的 `--video-url`：远程视频的 HTTP(S) URL，可重复，与 `--image` 合计最多 4 项。`chat` 同时写明只作用于本次 `--prompt`，REPL 与 `--json` 不接受；`run` 写明 `--json` 不接受。这是同一附件帮助里 `--image` 已经写明的拒绝，不是新限制。

### 2. 合计上限错误改为中文，语义不变

`run_one_turn` 在 `images.len() + video_urls.len() > 4` 时返回「--image 与 --video-url 每轮合计最多 4 项」。比较、拒绝顺序和 4 项合计不变，仍发生在 `local_image_parts` 之前。

`local_image_parts` 里图片数量、路径、类型，以及 1 字节到 8 MiB 的中文错误本来就存在。这些字符串是 no-op，本次不改。

### 3. 描述符类型复查保留

`local_image_parts` 先对路径做 `metadata`：不是普通文件，或大小不在范围内，就拒绝。然后 `File::open`，再查已打开文件是不是普通文件；查询失败也拒绝。之后才最多读取 8 MiB + 1 字节，并再查读到的字节数。

两次检查看的不是同一个对象。路径 `metadata` 看打开前的路径目标；描述符 `metadata` 看已经打开的文件。路径在两次调用之间被换成可打开的非普通文件时，第二次会拒绝，并且不把内容读进内存。

删除不能证明等价。稳定的非普通文件会先被路径检查挡住；若在打开前被换成 `/dev/null`、`/dev/zero` 这类能打开的非普通文件，没有第二次检查就会读到上限再按大小拒绝，或把空读报成大小错误。

保留也不消除 TOCTOU。两个普通文件之间的内容替换仍然发生在检查之后。`File::open` 若碰到会阻塞的 FIFO 或设备，会在第二次检查之前卡住；第一次路径检查只能拒绝当时已经不是普通文件的路径。

不新增打开标志，也不改读取上限。代码里只加了说明该区别的注释。

## 写入集

`crates/cli/src/lib.rs`、`crates/cli/src/chat.rs`、[cli Spec](../spec/crates/cli.md)、[ROADMAP](../ROADMAP.md)、本文件。

没有 Host 外壳或脱敏行为变化，不改 `apps/pawork` 与 pawork Spec。

## 验证

日志目录 `/tmp/pawork-b17-cli-copy`。

改动只有 clap 帮助、一条用户错误字符串，以及说明保留原因的注释。解析、默认值、端口、loopback、token、媒体数量、大小、MIME、URL 拒绝、REPL / JSON 限制和文件访问顺序都没有变。现有 clap 测试不断言帮助文本，附件测试不断言这条合计错误。因此不跑 Cargo，也不跑 Host 子进程或全 workspace。

未执行真实 `pawork … --help` 或附件运行时。仓库里的旧二进制不能当作这次源码的帮助输出。

| 命令 | 结果 |
| --- | --- |
| `rustfmt --edition 2021 --config skip_children=true --check crates/cli/src/lib.rs` | exit 0 |
| `rustfmt --edition 2021 --check crates/cli/src/chat.rs` | exit 0 |
| `git diff --check` | exit 0 |
| 三个任务文档的相对链接指向存在的文件 | 无缺失 |
| 静态差异对照 | 见日志 `static-diff.log`；只改上述写入集 |

## 完成记录

2026-10-08：已裁决并实现，必要定向检查通过。独立只读 Review（`01a11a28-5be2-74a3-96ae-947107ffa38d`，`gpt-6.1-sol` / `max`）最终 PASS，无剩余阻断或非阻断问题；审查读取既有日志，未重跑检查。指挥核对受审快照与工作区逐字节一致后，仅同步完成状态，ROADMAP 标为 ✅，按上述 5 文件写入集本地提交，不推送、不发布。纯文案与注释改动未运行 Cargo；真实新 help、附件运行时、Provider 与 Windows 执行均未验证。该结论只覆盖 B17，不关闭 B18、CU-10 或其它人工验收。
