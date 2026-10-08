# B14 · AppCore 同步装配双轨裁决

> 状态：✅ 完成。完成日期：2026-10-08。已实现与必要检查通过，独立 review PASS；未推送、未发布。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

核实测试专用同步构造路径并决定最小收敛方式。

## 当前线索与读取范围

登记项把 `AppCore::from_resolved` 和 `from_config` 都写成测试专用同步装配。核实后只有 `from_resolved` 是同步双轨。

[app Spec](../spec/crates/app.md)、[验证规格](../spec/verification.md)。

## 实施范围

- 核对生产和测试构造链，比较直接使用现有装配路径的成本。
- 只消除已证明没有用途的同步双轨，不新增第二套适配或测试框架。

## 完成条件

- 装配消费者已迁移或保留理由明确，测试未误用无 reactor 路径。
- 受影响现有 AppCore 测试通过，公开面变更同步 app Spec。
- 独立 review PASS 后由指挥核对证据并同步完成状态。

## 完成记录

基线 `2ffc7690507e9ef2271ba5734d43ca28357e0778`。开始时工作区干净。

2026-10-08：独立只读 review 使用 `gpt-6.1-sol` / `max`，结论 PASS，无阻断或非阻断问题。审查者与指挥均核对受审 `final.diff`（24,356 字节，SHA256 `22a4bcd0648c4d2593ddb845d42074da9126f4478a29caf6b5c639ebd92d4f03`）与工作区逐字节一致；审查后只同步完成元数据。五个消费者已迁移，受影响回归与正式宿主类型检查通过，公开面已同步 app Spec，满足本任务完成条件。

## 裁决

删除 `AppCore::from_resolved`。保留 `AppCore::from_config`。

`load` / `load_for_catalog` 经 `load_with` 调用异步 `from_config_inner`。`from_config` 也是这个函数的薄包装，`allow_pending = false`，`assemble_provider` 的刷新开关为 true。它接受调用方传入的 `SecretBackend`。正式宿主 `apps/pawork` 与 `crates/cli` 只调用 `load` / `load_for_catalog`，不调用这两个低层入口。

`from_resolved` 是另一套同步构造。它固定 `FileBackend::new()`，用 `futures::executor::block_on` 调用 `assemble_provider`，刷新开关为 false，并且没有 `allow_pending`。开始时全仓库只有 `crates/app/src/provider_assembly.rs` 的五个 `#[test]` 调用它。`McpConfig::from_resolved` 是另一个函数，不在本次范围内。

因此 `from_config` 是现有异步注入入口，不是第二套装配。`from_resolved` 没有生产消费者，还会读真实 auth 文件，并在没有 Tokio reactor 的路径上关掉 OAuth 刷新。不把 `block_on` 挪到新桥接层，也不为它留兼容包装。

生产 `load` / `load_for_catalog`、CatalogOnly 缺凭证 fail-closed、`allow_pending`、provider/model 覆盖、proxy 校验、auth revision、OAuth 刷新、Secret、不透明持久存储、MCP、原生目标与 usage 装配都没有改。没有做 B15 / B16，也没有改子 Core 认证修订或设置错误。

## 测试迁移

五个同步测试改到 `#[tokio::test]`，经 `from_config` 加 `MemoryBackend` 走现行严格装配。不读写用户 auth 文件，不调用真实 Provider。`remove_env` 只清掉缺凭证用例自己的 `PAWORK_API_KEY_APP_CORE_MISSING_KEY`，避免环境变量把 fail-closed 判成成功。

删掉的只有 `from_resolved_requires_provider_and_model` 里「空配置返回 `MissingDefaultProvider`」这一条。同一断言已在 `chat_load_still_fails_closed_without_defaults`，它调用 `from_config_inner(..., false)`。未知 provider、缺凭证与 env 名、CLI 覆盖后的 Debug 脱敏、默认 Messages 协议、未知协议 fail-closed 都留在现行路径。OAuth 刷新成功与 `invalid_grant` 关闭本来就直接断言 `oauth_credential(..., true)`，不经过已删除的 `refresh = false` 路径。没有新增测试。

## 写入集

`crates/app/src/app_core.rs`、`crates/app/src/provider_assembly.rs`、[app Spec](../spec/crates/app.md)、本文件、[ROADMAP](../ROADMAP.md)。依赖方向、冻结 wire 和产品安全边界没有变，不改 architecture 或 flows。

## 验证

日志目录 `/tmp/pawork-b14-app-construction`。Cargo 串行，每次前确认没有其它 `cargo` 进程。沿用 kache，不 `cargo clean`。

| 命令 | 结果 | 日志 |
| --- | --- | --- |
| `cargo test -p pawork-app --offline --lib -- provider_assembly::tests` | 14 passed，0 failed，285 filtered out，exit 0。编译 Finished in 2m 03s，测试 0.13s。无 `pawork-app` warning。既有 `pawork-mcp` `with_auth_token` dead_code 与 `block` future-incompat 提示仍在。 | `provider-assembly.log` |
| `cargo check -p pawork --offline` | exit 0，Finished `dev` in 2m 48s。同上两条既有提示，不是本次引入。 | `pawork-check.log` |

未跑 Desktop、其它包或全 workspace。自动检查不表示真实 Provider 或人工验收。交接 `final.diff` 在日志目录，于本计划定稿后生成。
