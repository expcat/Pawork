# pawork-cli

`pawork` 二进制的子命令与 ACP 通道。依赖 app / client / domain / engine / protocol / storage(session) / transport。

## 职责

clap 解析、六运行模式（chat / run / headless / acp / gui / service）与运维子命令。CLI 与 Core **同进程**：`run()` 加载 `AppCore` 后分发。ACP 不持有 Provider 凭证、不构造第二个 Core、不消费 GUI 帧。GuiHost 经 app，不直连 Core crate 以外的实现细节。

## 模块树

```
src/
  lib.rs          # Cli / Command / run()
  chat.rs  sessions.rs  auth.rs  gui.rs  headless.rs  acp.rs
  mcp.rs  import.rs  ops.rs  service.rs  usage.rs  vcs.rs
  plan.rs  tasks.rs  agents.rs  approval.rs  adapter.rs  render.rs  error.rs
  channels/acp/{mod,adapter,command_host,host,map,wire}.rs
tests/
  acp_fixtures.rs  acp_floor.rs
fixtures/v1/  fixtures/v2/
```

仅 `pub mod channels`；其余模块私有。

## 对外入口/API 面

- `pub async fn run() -> ExitCode`
- `Cli`（`#[command(name = "pawork")]`）与 `Command`。全局：`--provider` / `--model` / `--instance` / `--json` / `--approval-mode`。
- ACP：`AcpHost`、`AcpCommandHost`、`AcpClientAdapter`；`PROTOCOL_VERSION`。Core 经 `AcpCommandHost` 注入。

**21 个顶级 clap 名：**

`chat`、`sessions`（list/show/export/import/fork）、`run`、`models`、`auth`（list/set-key/login/logout）、`gui`（serve）、`diff`、`rollback`、`mcp`（list/test）、`import`、`headless`、`acp`（serve）、`service`（install/start/stop）、`status`、`watch`、`shutdown`、`doctor`、`usage`、`tasks`、`plan`、`agents`（demo）。

`service` / `status` / `doctor` / `watch` / `shutdown` 在加载 `AppCore` **之前**运行。裸 `acp`（无 `serve`）是错误。

## 依赖与被依赖

- **依赖**：如上。protocol feature `adapter`。无 crate feature。
- **被依赖**：仅 `apps/pawork`。

## 红线与注意事项

- `--json` 或非 TTY → `DenyAllApprovals`（fail-closed）。`gui` / `headless` / `acp` 用 `GuiApprovalHost`。
- `--json` / `--json-stdio`：stdout **只**承载 JSONL，日志走 stderr。
- `gui serve`：`TokenStore` 写 `gui.token`；socket 目录 `0o700`。
- 三通道命令可用性必须查 protocol registry，禁止再维护一份名字表。
- CLI resume 对未决审批 seal Denied（与 GUI keep-pending 不同，有意）。

## 相关文档

- [docs/design.md](../../docs/design.md) §4
- [docs/headless-json-migration.md](../../docs/headless-json-migration.md)
- [代码地图总索引](../../docs/code-map/README.md)
