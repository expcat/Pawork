# pawork（apps/pawork）

CLI 唯一正式宿主二进制。依赖 **仅** `pawork-cli`。

## 职责

composition root：安装带脱敏的 tracing，再 `pawork_cli::run()`。不存在独立 daemon / rpc 入口。R1 波 A 自 diagnostics 迁入 `redact.rs`。

## 模块树

```
src/
  main.rs      # tokio main → install_logging → pawork_cli::run
  redact.rs    # Redactor / RedactingFmtLayer（crate-private）
```

无库表面、无 `tests/`。

## 对外入口/API 面

进程入口 `main`。无 `pub` API。日志：`RUST_LOG`（默认 `warn`）→ stderr；stdout 留给协议 / JSON。

`Redactor` 对敏感键与 bearer / JWT / `sk-` / `rk-` / `pk-` / `api-` / query-token 等模式替换为 `[REDACTED]`，作用于全部 tracing 字段。

子命令清单见 [crates/cli/MODULE.md](../../crates/cli/MODULE.md)。

## 依赖与被依赖

- **依赖**：`pawork-cli`；`tracing` / `tracing-subscriber` / `regex` / `tokio`。
- **被依赖**：无 Cargo 依赖方。运行时消费者：`pawork-client` headless spawn（`PAWORK_BIN` 或 `pawork`）、desktop `--probe*`、`pawork gui serve` 作为 GUI 宿主进程。

## 红线与注意事项

- CLI 与 Core 同进程同二进制；不要再加第二条宿主。
- Secret 不进终端与日志：漏过 `RedactingFmtLayer` 即违规。
- 本包不要直接依赖 `pawork-app` / engine / providers；装配链是 `pawork` → cli → app。
- 纯 Rust；禁止引入 JS runtime。

## 相关文档

- [AGENTS.md](../../AGENTS.md) §2 / §3
- [crates/cli/MODULE.md](../../crates/cli/MODULE.md)
- [代码地图总索引](../../docs/code-map/README.md)
