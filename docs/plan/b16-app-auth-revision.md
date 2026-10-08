# B16 · App 子 Core 认证修订与重导出收窄

> 状态：✅ 完成；自动验证通过，独立 Review PASS。完成：2026-10-08。更新：2026-10-08。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

核实 same-provider 子 Core 首轮冗余重装配的原因，并按实际消费者收窄 crate 内重导出。

## 基线

`main`，HEAD `62e1017965dc878b1a8ce597f23c67262b69a822`。开始时工作区干净。未做 B17 或 CU-10。不读用户 auth/config，不调用真实 Provider。

## 裁决

### 1. 修订属于子 Core 所持的 adapter

`from_parts_with_protocol` 把 `provider_auth_revision` 写成 `None`。同 Provider 的 `child_core` 把父 adapter 交给这个构造器之后没有回填修订。子 Core 下一次 `provider_needs_rebuild` 看到 `None != Some(索引修订)`，凭证没变也会再装配一次。

修订必须属于这次实际持有的 adapter，不能把父修订抄到新装配的 adapter 上，装配之后也不再读后端：

- 父 adapter 可复用：快照带回父 Core 上的修订、adapter 和 credential。
- 父修订过期：快照带回同一次 `assemble_provider` 的 adapter、credential 和 `auth_revision`。`assemble_provider` 仍在 OAuth 刷新之前记下修订；父路径这个时序不改。

`provider_needs_rebuild` 不变。pending、stale、账号索引修订不一致、临近过期的 OAuth，以及后端读取失败，仍然重装配。OAuth 过期、后端错误、跨 Provider 子 Core、未知 provider/model 仍走原路径。

`ProviderSnapshot` / `provider_snapshot` 承担这次快照。`request_provider_snapshot` 只取其中的 adapter。`child_core` 在 `from_parts_with_protocol` 之后写入 `prepared.auth_revision`。reasoning protector 的复用方式不变。

### 2. 根重导出收窄，不删类型

有 crate 内消费者的名称改为 `pub(crate)`：

- `EventHub`、`HubError`、`DEFAULT_HUB_CAPACITY`，供 `GuiEventBus` 与 `GuiHostAdapter` 使用。
- `IdempotencyStore`、`IdempotencyCheck`、`IdempotencyError`、`should_cache`、`DEFAULT_IDEMPOTENCY_CAPACITY`，供 GUI 宿主使用。

`HubSubscription` 与 `IdempotencyStats` 不再从根导出。类型和方法留在私有模块；包外没有代码按这两个名字使用它们。

`GuiEventBus::hub` 与 `replay` 改为 `pub(crate)`。它们是唯一把上述类型重新暴露到公开签名的方法；包外生产代码和测试都没有调用。CLI 导入不变。事件顺序、`Lagged` / `ReplayUnavailable`、SQLite 幂等语义不变。

不改 [auth Spec](../spec/crates/auth.md) 与 [flows](../spec/flows.md)。账号选择、写入、刷新和重装配条件没有变。

## 写入集

`crates/app/src/provider_assembly.rs`、`crates/app/src/subagents.rs`、`crates/app/src/lib.rs`、`crates/app/src/gui_host/bus.rs`、[app Spec](../spec/crates/app.md)、[ROADMAP](../ROADMAP.md)、本文件。

## 验证

日志目录 `/tmp/pawork-b16-app-auth-revision`。Cargo 串行，每次前确认没有其它 cargo / rustc。沿用 kache，不 `cargo clean`。两条新测试只用 `MemoryBackend`，shell 不改 HOME，不读用户 auth。

| 命令 | 结果 | 日志 |
| --- | --- | --- |
| `cargo test -p pawork-app --offline --lib -- subagents::tests hub:: idempotency gui_event_bus` | 32 passed，0 failed，269 filtered out，exit 0。编译 Finished `test` in 9.31s，测试 0.37s。`pawork-mcp` `with_auth_token` dead_code 与 `block` future-incompat 是既有提示。 | `lib-tests.log` |
| `cargo check -p pawork --offline` | exit 0，Finished `dev` in 19.05s。同上两条既有提示。另有 5 条 `pawork-app` dead_code，见下。 | `pawork-check.log` |

第一次同过滤测试也是 32 passed（Finished `test` in 56.87s，测试 0.38s），但新 helper `indexed_mock_parent` 有 `unused_mut`。去掉 `mut` 后重跑上表第一条，该警告消失。第一次日志保留为 `lib-tests-before-unused-mut.log`。

`rustfmt --edition 2021 --config skip_children=true --check` 覆盖上述四个 Rust 文件，exit 0。不把未编译的 Desktop 或其它包记为通过。

`cargo check` 的 5 条新警告都在非测试库构建里：`HubError` 的 `Lagged` / `Closed` / `Empty`，`EventHub::capacity` / `subscribe`，`HubSubscription` 及其 `recv` / `try_recv`，`IdempotencyStore::stats`。它们原先靠根 `pub use` 算作对外 API，所以不报 dead_code；测试仍覆盖这些路径。类型和方法按裁决保留，不加 `allow`。

未跑 Desktop、全 workspace 或真实 Provider。独立只读 Review 已在自动检查后完成，结论 PASS，无剩余问题；reviewer 未编译或重跑测试。受审 `final.diff` 在日志目录，于实现计划定稿后生成；指挥核对与工作区逐字节一致后，仅同步完成元数据。字节数和 SHA256 写在交接里，不回写本文件。

## 保留边界

OAuth 刷新仍可能在 `assemble_provider` 读完修订之后抬高后端修订；新 adapter 记下的是刷新前的值。这是既有父装配行为。之后的账号选择、写入或删除、刷新、临近过期 OAuth、后端读取失败仍会重装配。跨 Provider、未知目标、取消与工具权限不在本次写入集。

## 完成记录

2026-10-08：已实现。定向 lib 回归与正式宿主类型检查通过；独立只读 Review PASS，阻断与非阻断问题均为无。指挥只同步已验证的完成状态，并精确提交本任务 7 文件；不推送、不发布。该完成仅覆盖 B16，不关闭 B17、CU-10、Windows 或其它人工验收条件。
