# B19 · Desktop 终端响应与实例路径裁决

> 状态：✅ 完成；定向自动回归通过，独立 review / 复审 PASS。更新：2026-10-08。基线 `4d0318441a738a79967f7c7f0528bbaba0617321`。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 裁决

删除 Desktop 解析 `terminal_create` Data 时对 `id` 的回退，只保留字符串 `terminal_session_id`。`id` 不是正式回执，也不是 1.0–1.27 里按 minor 切换的旧形状。

删除三个没有生产调用的路径函数：`default_socket_path`、`default_token_path`、`token_path_for_instance`。生产入口仍是 [main.rs](../../apps/desktop/src/main.rs) 的 `socket_path_for_instance` 和 [controller connect](../../apps/desktop/src/controller/mod.rs) 的 `token_path_for_socket`。

| 符号 | 裁决 | 依据 |
| --- | --- | --- |
| `terminal_session_id` 解析里的 `id` | 删除 | 正式 Host 与 golden 从不发这个键；受支持 minor 没有另一套回执 |
| `default_socket_path` | 删除 | 只调用 `socket_path_for_instance(None)`；生产 main 已直接用后者 |
| `default_token_path` | 删除 | 只调用 `token_path_for_instance(None)`；无生产调用 |
| `token_path_for_instance` | 删除 | 只被上一个 wrapper 和旧测试调用；bin-only，无仓库外 Rust 消费者 |
| `socket_path_for_instance` | 保留 | 生产 socket 入口 |
| `token_path_for_socket` | 保留 | 生产 token 入口；同目录推导不改 |

命令、事件和 write / resize / close 参数里的 `terminal_session_id` 不改名。`workspace_add` 回执仍读 `id`。

## 事实

登记项把方向写成「`id` 回退为 `terminal_session_id`」。基线 `4d031844` 中 [controller/mod.rs](../../apps/desktop/src/controller/mod.rs) 的 helper 是先 `data.get("terminal_session_id")`，缺键才 `data.get("id")`，然后 `as_str`。唯一生产调用是 [controller/terminal.rs](../../apps/desktop/src/controller/terminal.rs) 的 `terminal_create`。

正式 Host [terminal_create_payload](../../crates/app/src/gui_host/handlers/terminal.rs) 写入 `terminal_session_id`，以及 `sandboxed` / `policy` / `approval_mode` / `note`。golden [server_response_terminal_create.json](../../crates/protocol/tests/golden/server_response_terminal_create.json) 同形，没有 `id`。`AppResponse::Data` 是无类型 `Value`，Host 构造时不看协商 minor。

版本边界：

- `terminal_create` 在 registry 的 `since` 是 `V1_0`。`SUPPORTED_API_VERSIONS` 是 1.0–1.27。Desktop 用 `ClientConfig::default` 的整张支持表握手，协商取共同最高 minor。
- protocol 里按版本改响应形状的 `for_api_version` 只作用于 `TimelinePage`（低于 1.14 去掉思考字段）。终端 create 回执没有对应变换。
- 首次 Host 实现（`a9c2ce21`）的回执就是 `{"terminal_session_id": ...}`。`29dfbc20` 增加 `uncontrolled` / `note`，键不变。`691dc83e` 换成现在的 policy 字段，身份键仍是 `terminal_session_id`。仓库里没有受支持 minor 把终端身份写成 `id`。
- Desktop 的 `id` 分支自 `5df88a27` 就在，没有对应的 Host 形状。零调用不能单独当删除理由；这里删除依据是整段受支持版本的回执形状。

收窄后的解析：两边都有时仍取 `terminal_session_id` 字符串。该键存在但不是字符串时仍拒绝，因为原来的 `or_else` 只在缺键时才看 `id`。只有 `id` 时，从接受改为 `TerminalCreateFailed`（unexpected response）。Error / Accepted / Artifact 仍拒绝。事件上的 `workspace_id` 仍来自请求参数。write / resize / close 不经过这个 helper。

这不是 schema、golden 或 wire 演进。client / protocol 源码与它们的 Spec 未改。

路径：`pawork-desktop` 是 bin-only。`socket_path_for_instance` 的调用者是 `main` 和已删除的 wrapper。`token_path_for_socket` 的调用者是 `connect`。三个删除符号没有生产调用。

保留的推导不变：`pawork-gui.sock` → 同目录 `gui.token`；`pawork-gui-{instance}.sock` → 同目录 `gui-{instance}.token`；其它文件名仍落到同目录 `gui.token`。`instance_file_name` 仍把空白和 `default` 当成无后缀。`default_data_dir` 的环境顺序不变。token 缺失、不可读或为空仍 fail-closed。本次不读、不写真实 token、用户 config、data 或 auth。

旧测试 `default_token_path_aligns_with_a5_gui_token` 没有整删。有效断言迁到 `socket_instance_path_derives_same_directory_token`：默认 socket 文件名、`default` 与 `None` 同路径、`dev` 的 socket 文件名、默认 socket 的同目录 `gui.token`，以及 `/tmp/pawork-gui.sock` 与 `/tmp/pawork-gui-dev.sock` 两条标准推导。

## 未纳入

不改 B20、B21、CU-10。不加依赖，不改 golden / schema / wire，不做全局文档修正，不改 GUI 渲染。不把路径函数挪到新 adapter，也不扩大公开面。实现者不操作完成状态与提交；独立审查和完成元数据由指挥负责。未推送或发布。

## 写入集

- `apps/desktop/src/controller/mod.rs`
- `apps/desktop/src/platform.rs`
- `docs/plan/b19-desktop-terminal-paths.md`
- `docs/spec/crates/desktop.md`
- `docs/ROADMAP.md`

## 验证

```text
cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders -- terminal_create_receipt_accepts_only_terminal_session_id terminal_commands_use_workspace_relative_cwd socket_instance_path_derives_same_directory_token
```

同一命令跑了两次，都是 exit 0：3 passed；0 failed；272 filtered out。第一次编译 client 与 desktop，`Finished test profile in 4m 45s`。随后只收紧 helper 注释，第二次覆盖最终源码，`Finished test profile in 11.14s`。证据目录 `/tmp/pawork-b19-desktop-terminal-paths`。

另有 `rustfmt --edition 2021 --config skip_children=true --check`（上述两个 Rust 文件，exit 0）和 `git diff --check`（exit 0）。

未跑整个 Desktop、App、Client，未跑 GUI probe / spawn，未跑真实 Provider，未跑 `cargo check -p pawork`。Host 与 protocol 源码未改，正式 Host 类型检查不是这次行为变化的必要门禁。真实窗口未验证。

```text
Validated: cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders -- terminal_create_receipt_accepts_only_terminal_session_id terminal_commands_use_workspace_relative_cwd socket_instance_path_derives_same_directory_token（exit 0，3 passed / 272 filtered out）；rustfmt --check；git diff --check
Targeted regressions: controller terminal create 回执解析与既有 terminal 命令 wire；platform socket→token 同目录推导
Full workspace gate: NOT RUN
```

## 完成记录

2026-10-08：按现行 Host、golden 和 1.0–1.27 无版本分支的回执形状，去掉 `id` 回退；删除三个零生产调用的路径 wrapper，并把原测试里的路径与归属断言留在现行入口。三项定向测试通过。独立审查 `01a11a62-d368-7003-b31a-3e9d371fb06f`（`gpt-6.1-sol / max`）初审的两项 P3 文档问题已修正，同一审查者增量复审最终 PASS，阻断与非阻断问题均无。纯文字修复未重跑 Cargo。指挥核对受审快照（27133 字节，SHA256 `39b7b947cfc6299026af4208fd11a2c05d1f69e69067891e206f1f46922cd6cd`）与工作区一致后，仅同步完成元数据；受审源码与 Spec 不变。不是历史二进制、真窗口或产品集成验收。
