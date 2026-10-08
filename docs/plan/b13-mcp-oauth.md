# B13 · MCP OAuth 未接线模块裁决

> 状态：✅ 完成（已归档）。更新：2026-10-08。实现、必要检查与独立 review / 复审通过；未推送、未发布。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

在生产接入和归档之间裁决 MCP OAuth。裁决是归档，不接线。

## 事实

基线 `main` / `3c235c91a07ad348dd68c4dc18c01f4290bb9b2f`。开始时工作区干净。

生产装配是 `McpServerConfig::build_client` → `SecretResolvingConnector`。HTTP 分支把 SecretRef 解析成 header 后走 `DefaultConnector::http`，不经过 `OAuthHttpConnector`。Host 的 `mcp_secret_backend()` 使用独立 `mcp-auth.json`，不是 Provider 的 `FileBackend`。

`begin_pkce_login`、`complete_pkce_login`、`McpBearerProvider`、`OAuthHttpConnector` 的定义和调用都只在已删除的 `crates/mcp/src/oauth.rs`。包外没有消费者。`OAuthHttpConnector::new` 的参数 `HttpTransportConfig` 在私有 `mod transport`，包外无法命名。

`complete_pkce_login` 调用 auth 的 `store_oauth_token`，service 是 `pawork.<provider>.oauth`，不是 `pawork.mcp.*`。接上现行 backend 会把 MCP token 写进 Provider OAuth 命名空间，或写进 `mcp-auth.json` 但 SecretRef 不认该 service。这是第二套认证装配，和现行凭证边界冲突。

`McpConnector::should_reconnect_before_request` 默认返回 false，唯一覆写是 `OAuthHttpConnector`。manager 每次取 peer 都调用它，生产连接器永远是 false。`McpError::OAuth` 与 `from_auth` 只服务该模块。

不接线。接线需要宿主登录流程、callback 与 state 关联、把存储改到 `pawork.mcp.*` 加独立 `mcp-auth.json`、刷新轮换、transport 重建和失败关闭。那会扩大产品登录流程并改变凭证安全语义。本任务不实施该改动。

不保留。架构要求无消费者不合入。不为尚未存在的登录流程留第二套生产认证装配。库内 OAuth 测试不能当成 Host 登录或刷新已接线。

## 逐项裁决

| 对象 | 裁决 | 依据 |
| --- | --- | --- |
| `begin_pkce_login` / `complete_pkce_login` | 删除 | 只有本模块测试调用。存储命名空间不是 MCP。 |
| `McpBearerProvider` / `OAuthHttpConnector` | 删除 | 没有生产构造。包外无法构造 connector。 |
| `McpConnector::should_reconnect_before_request` 及 manager 调用 | 删除 | 唯一非默认实现属于已删除 connector。生产行为本来恒为 false。 |
| `McpError::OAuth` / `from_auth` | 删除 | 只由已删除模块构造。`McpError` 现为 9 个 variant。不是冻结 wire。 |
| mcp 的 dev-dependency `wiremock` | 移出 | 只被已删除测试使用。`Cargo.lock` 只从 `pawork-mcp` 依赖列表去掉这一项；包本身仍被 auth、app、providers 使用。 |
| `pawork-auth`、`reqwest` | 保留 | SecretBackend、locator、HTTP header 校验仍在使用。 |
| SecretRef、`mcp-auth.json`、URL/明文 HTTP 携密拒绝、SDK 隔离、transport Debug 脱敏、同表审批、stdio Sandbox、断连重连 | 保留 | 有生产消费者。断连重连不是 token 轮换重建。 |

## 删除测试的无效性

删掉的四项只覆盖已退出 API：未过期 bearer 不刷新、到期刷新并写回、PKCE 换码后元数据无明文、connector 注入 bearer 且 Debug 不泄漏。PKCE 那项使用匹配的 state，并不覆盖 state 拒绝。这些测试不能证明现行 SecretRef HTTP 路径。

仍有效的替代锚点在 `pawork-auth`，不是 MCP 产品验收：

- `exchange_pkce_code_rejects_state_mismatch`
- `store_oauth_token_keeps_plaintext_out_of_metadata`
- `oauth_debug_output_redacts_ephemeral_secrets`
- `update_oauth_token_persists_rotated_refresh_and_expiry`
- `request_resolution_auto_refreshes_and_persists_rotation`

本包仍保留 `rejects_invalid_transport_and_permissions`（含非 loopback 明文 HTTP 携密拒绝）、`rejects_url_userinfo_and_fragment`、`resolve_rejects_provider_and_oauth_namespaces`、transport/HTTP Debug 脱敏、工具权限/信任/预算、stdio Sandbox 与取消/重连。本任务没有新增测试。

## 恢复锚点

当前布局的恢复锚点是 `3c235c91a07ad348dd68c4dc18c01f4290bb9b2f` 的 `crates/mcp/src/oauth.rs`（blob `d5865e3963b2ba0c9188a71ff0e2d97bf2db758f`）。引入提交是 `fbda826decad4b8459e00f6fed6ef7bb113207ef`（2026-08-17）。tag `v2-final`（`088b539bfa35e4466f8fe7c3550c730b74c551d4`）只有旧路径 `extensions/mcp/src/oauth.rs`，blob `daa493631fa3b63db1b5296509ec0c40d74aa201`，不相同，不作为当前布局恢复点。不另存副本。复活前置登记为 [BK-MCP-01](../spec/backlog.md)。

改前 `cargo tree -p pawork-mcp --offline --edges normal --depth 1` 在 `/tmp/pawork-b13-mcp-oauth/tree-mcp-normal-before.txt`。直接生产依赖含 `pawork-auth` 与 `reqwest`，改后相同。

## 写入集

代码：删除 `crates/mcp/src/oauth.rs`；改 `crates/mcp/src/lib.rs`、`crates/mcp/src/manager.rs`、`crates/mcp/src/transport.rs`、`crates/mcp/Cargo.toml`。`Cargo.lock` 只从 `pawork-mcp` 去掉 `wiremock`。`lib.rs` 写明恢复锚点。

文档：[mcp Spec](../spec/crates/mcp.md)、[auth Spec](../spec/crates/auth.md) 的下游事实、[flows](../spec/flows.md) §4、[architecture](../architecture.md)、[security](../spec/security.md)、[capabilities](../spec/capabilities.md)、[spec 索引](../spec/README.md)、[backlog](../spec/backlog.md)、本文件、[ROADMAP](../ROADMAP.md)。Auth 生产流程、B6 alias 与 B8 导入副本未改。

## 验证

日志目录 `/tmp/pawork-b13-mcp-oauth`。Cargo 串行，每次 Cargo 前确认没有其它 `cargo` 进程。沿用仓库 kache，不 `cargo clean`。未调用真实 Provider，未写用户持久配置。

| 命令 | 结果 | 日志 |
| --- | --- | --- |
| `bash scripts/test.sh --log /tmp/pawork-b13-mcp-oauth/mcp-rerun.log mcp` | 61 passed，0 failed，exit 0，Elapsed 24s。实际 Cargo：`cargo test --offline --tests -p pawork-mcp`。编译无 `pawork-mcp` warning。 | `mcp-rerun.log` |
| `cargo test -p pawork-auth --offline --lib -- exchange_pkce_code_rejects_state_mismatch store_oauth_token_keeps_plaintext_out_of_metadata oauth_debug_output_redacts_ephemeral_secrets update_oauth_token_persists_rotated_refresh_and_expiry request_resolution_auto_refreshes_and_persists_rotation` | 5 passed，78 filtered out，exit 0。这只证明 auth 锚点仍在，不是 MCP 产品验收。 | `auth-anchors.log`、`auth-anchors.exit` |
| `cargo check -p pawork --offline` | exit 0，Finished in 20.57s。既有 `block` future-incompat 提示仍在。`HttpTransportConfig::with_auth_token` 的 dead_code 在改前就只被测试调用，不是本次删除造成的。 | `pawork-check.log`、`pawork-check.exit` |
| `cargo tree -p pawork-mcp --offline --edges normal --depth 1` | exit 0。改前改后直接生产依赖相同，仍含 `pawork-auth` 与 `reqwest`。 | `tree-mcp-normal-before.txt`、`tree-mcp-normal-after.txt` |
| `cargo tree -p pawork-mcp --offline --edges dev --depth 1` | exit 0。dev 边去掉 `wiremock`，保留 rmcp 与 tokio。 | `tree-mcp-dev-before.txt`、`tree-mcp-dev-after.txt` |
| `cargo tree -p pawork --offline --edges normal` | exit 0，709 行，stderr 空，与改前无差异。没有新增包或边，闭包未膨胀。 | `tree-pawork-before.txt`、`tree-pawork-after.txt` |
| `bash scripts/mock/gate.sh --level 0` | exit 0。本次改动 Markdown 共 10 个文件，相对链接存在；同一次包含 `git diff --check`。 | `links-l0.log`、`links-l0.err`、`links-l0.exit` |
| `rustfmt --edition 2021 --config skip_children=true --check crates/mcp/src/lib.rs crates/mcp/src/manager.rs crates/mcp/src/transport.rs` | exit 0。只这 3 个本次修改的 Rust 文件。 | `rustfmt-check.log`、`rustfmt-check.err`、`rustfmt-check.exit` |

第一次 `mcp.log` 在去掉 `should_reconnect_before_request` 后出现 `unused_mut`，已改成不可变绑定并以上表重跑覆盖。该日志不作为最终结果。未跑 tools、app、Desktop 或全 workspace。自动回归不表示 MCP OAuth 登录已实现，也不表示 BK-MCP-01 已激活。交接文件 `final.diff` 与 `commands.md` 在日志目录，于本计划定稿后生成；`final.diff` 是当时工作区的 `git diff --binary`。

```text
Validated: bash scripts/test.sh --log /tmp/pawork-b13-mcp-oauth/mcp-rerun.log mcp（实际 cargo test --offline --tests -p pawork-mcp，61 passed，exit 0，证据 mcp-rerun.log）；cargo test -p pawork-auth --offline --lib -- exchange_pkce_code_rejects_state_mismatch store_oauth_token_keeps_plaintext_out_of_metadata oauth_debug_output_redacts_ephemeral_secrets update_oauth_token_persists_rotated_refresh_and_expiry request_resolution_auto_refreshes_and_persists_rotation（5 passed，78 filtered out，exit 0，只作 auth 锚点，不是 MCP 产品验收）；cargo check -p pawork --offline exit 0；cargo tree -p pawork --offline --edges normal exit 0（709 行，与改前相同，无环诊断，闭包未膨胀）；bash scripts/mock/gate.sh --level 0 exit 0；rustfmt --edition 2021 --config skip_children=true --check crates/mcp/src/lib.rs crates/mcp/src/manager.rs crates/mcp/src/transport.rs exit 0；git diff --check exit 0
Targeted regressions: pawork-mcp lib 61（含明文 HTTP 携密拒绝、URL userinfo/fragment 拒绝、Provider/OAuth 命名空间拒绝、HTTP/transport Debug 脱敏、工具权限/信任/预算、stdio Sandbox、取消与断连重连、SDK 隔离）；pawork-auth 5 条安全锚点（state 拒绝、元数据无明文、Debug 脱敏、refresh 轮换落盘、请求前自动刷新）
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

## 完成记录

2026-10-08 按归档裁决删除无生产消费者的 MCP OAuth 装配，并完成上表必要检查。独立 review 指出 [auth Spec](../spec/crates/auth.md) 仍把 `http_client()` 写成“唯一 OAuth/MCP 默认构造”。该句已改为“唯一 OAuth 默认构造”。MCP HTTP 走 `DefaultConnector`，不消费这个函数。只改这一处措辞，不改生产代码、行为或安全语义，也不重跑已通过的 Cargo、测试和构建。复审前文档检查：`bash scripts/mock/gate.sh --level 0` exit 0（10 个 Markdown），`git diff --check` exit 0，日志 `links-l0-rereview.log`。同一 Sol max reviewer 增量复审最终 PASS，P3 已闭合，无剩余问题；实现与复审线程均正常 completed、error=null，指挥核对受审 diff 与工作区逐字节一致后同步完成状态。本任务不表示 MCP OAuth 登录、callback 或刷新已接线；HTTP 凭证仍只走 SecretRef 与独立 `mcp-auth.json`。本地提交由指挥在审查后执行；未推送、未发布。
