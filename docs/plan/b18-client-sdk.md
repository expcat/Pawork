# B18 · Client SDK MockTransport 与版本 API 裁决

> 状态：✅ 完成。更新：2026-10-08。基线 `ad55b4d33064c4b753ce3b0633e7c772c9864b6f`。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 裁决

保留，不收缩、不降可见性、不改行为。

| 符号 | 公开路径 | 仓库内调用 | 裁决 |
| --- | --- | --- | --- |
| `push_responses` | `pawork_client::headless::mock::MockTransport` | 无 | 保留 |
| `fail_next_read` | 同上 | 无（仅私有字段与 `read_line` 消费） | 保留 |
| `sent_count` | 同上 | 无 | 保留 |
| `assert_sent_json` | 同上 | 无 | 保留 |
| `sdk_version_string` | `pawork_client::headless::version::sdk_version_string` | 无 | 保留 |

`MockTransport` 在 [headless/mod.rs](../../crates/client/src/headless/mod.rs) rustdoc 与 [client Spec](../spec/crates/client.md) 的命名稳定面里。四个方法是该类型的公开方法，不是可改成 `cfg(test)` 或 `pub(crate)` 的内部助手。`sdk_version_string` 不在 `headless` 根 re-export，也不在命名稳定面清单里；`pub mod version` 使它公开可达。未列入清单不等于私有。

删除任一符号都是不兼容收缩。本任务不实施收缩，也不加 deprecated 双轨、兼容包装或版本框架。crate 仍为 workspace `0.1.0`，不 bump，不新造 CHANGELOG。

## 事实

- 派发时工作区为 `main` @ `ad55b4d3`，干净。本任务只改下列写入集。
- `SDK_API_VERSION` 是 `pawork_protocol::API_VERSION` 的别名。`crates/protocol/src/app/version.rs` 中 `API_VERSION = V1_27`。headless rustdoc 原先写当前 1.26，已改为 1.27。未改协议常量、schema、golden 或 wire。
- `headless` 根对 version 模块中的符号仅重导出 `SDK_API_VERSION` 与 `SDK_VERSION`；crate 根也不导出 `sdk_version_string`。拼接保持源码 `format!("pawork-sdk {SDK_VERSION} (protocol {SDK_API_VERSION:?})")`。`ApiVersion` 使用派生 Debug。本任务不渲染、不改写该字符串。
- `crates/client/tests/client_tests.rs` 的 22 个测试使用 `new` / `push_response` / `push_event` / `sent_lines`。四个待裁决方法零调用。`fail_next_read` 是私有错误槽的唯一公开注入口，`read_line` 会取出它。`push_responses` 入队后只 `notify_one` 一次，与逐条 `push_response` 不同。
- workspace `publish = false`。仓库内没有 CHANGELOG，也没有具名仓库外消费者引用这五个符号。rustdoc 仍把 mock 写成测试与下游集成用途。零仓库调用不能当成外部无人使用。

## 未纳入

- [contracts.md](../spec/contracts.md) 的 `CON-GUI-01` 仍把 GUI API 锚在 1.26、支持范围 1.0–1.26。这与现行 `API_VERSION` 1.27 不一致，但是全局契约目录，不是本任务要改的 headless rustdoc。保持只读。
- 不改握手版本门、unknown / unsupported 错误、背压、订阅、close、生产 spawn / `StdioTransport`，也不改 B19、CU-10 或其它包。

## 写入集

- `crates/client/src/headless/mod.rs`（rustdoc 当前 1.26 改为 1.27）
- `docs/plan/b18-client-sdk.md`
- `docs/spec/crates/client.md`
- `docs/ROADMAP.md`

## 验证

无可执行行为变化。未运行 Cargo，未重跑 `client_tests`、transport、spawn、probe、Desktop 或真实 Provider。不能把这些既有测试记为本次通过。

定向检查：公开符号仍在、rustdoc 已是 1.27、相对链接存在、`git diff --check`、`rustfmt --edition 2021 --config skip_children=true --check crates/client/src/headless/mod.rs`（exit 0，只查该文件）。证据目录 `/tmp/pawork-b18-client-sdk`。

```text
Validated: 静态公开面 / rustdoc / 相对链接 / git diff --check / headless/mod.rs 的 rustfmt --check（见证据 commands.md）
Targeted regressions: none（未重跑 headless client_tests 或 transport）
Full workspace gate: NOT RUN
```

## 完成记录

2026-10-08：裁决为保留，并修正 headless 模块文档中的协议当前版本。定向静态、链接、diff 与单文件格式检查通过；独立 Review 的 P3 重导出范围表述已修正，同一 reviewer 增量复审最终 PASS，无剩余阻断或非阻断问题。指挥核对受审快照一致后同步完成状态；不是产品集成验收，未运行 Cargo 或运行回归。本地提交由指挥处理，未推送或发布。
