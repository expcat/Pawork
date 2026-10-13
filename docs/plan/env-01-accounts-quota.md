# ENV-01 · 真实账号与额度环境验收

> 状态：正式额度查询与拒绝路径已核实；自然临期刷新、双账号、非零权威读数与倒计时待真实账号条件。更新：2026-10-13。

[返回 ROADMAP](../ROADMAP.md#环境待验)

## 目标

补齐 OAuth 刷新、双账号与权威额度的真实环境证据。

## 当前依据与读取范围

[auth Spec](../spec/crates/auth.md)、[Settings Spec](../spec/settings.md)、[验证规格](../spec/verification.md)。

## 验收范围与完成条件

- 每项有账号 / 端点环境、日期和实际结果，Secret 脱敏。
- 网络、账号或额度缺失按真实阻塞记录，mock 不能替代权威来源；不把缺额度显示为零余额通过。

## 已核实（2026-10-12）

- 正式 GUI 协议 `account_quota` 查询现有 Go 账号：rolling5h / weekly / monthly 三窗均为 `failed` / `forbidden`，used / limit / remaining / reset 均为空，`from_cache=false`；失败窗口不触发额度切换。未认证 `/usage` 经代理与直连均为 HTTP 403。
- 当前存有各一个 Go、xAI、Kimi Code 账号及 GLM / Qwen 默认 Secret 槽，没有同 Provider 双账号；xAI OAuth 已过期，不在自然临期窗口。
- 查询前后 auth / config / desktop 摘要一致，未改持久认证或设置。

现行源码锚点：额度读口 `crates/app/src/provider_quota.rs`（失败窗口无读数、`headroom` 对非 Ok 窗口返回 None）；切换回归 `gui_host::tests::settings::go_account_quota_routes_next_run_and_preserves_manual_selection`。

## 剩余待验

1. **自然临期刷新**：OAuth 凭证（如 xAI）进入 `needs_refresh` 窗口后走正式刷新，记录新的到期时间；不要用久已过期的 token 或重新登录代替。
2. **双账号切换**：同一 Provider 第二个真实账号。额度切换只实现于 Go API key + `WhenExhausted`：当前账号三窗为权威 Exact 且 remaining 为 0、另一账号 remaining > 0 时才切换。前置：第二套 Go 账号且 `/usage` 返回 ok。
3. **非零权威读数**：`GET /usage` 三窗为 ok、整数 percent、未来 `resetsAt`，视图 used / limit / remaining 为 Exact。前置：账号不再 403。
4. **倒计时与过期**：成功响应带 `resetsAt` 后核对绝对重置时刻；`resetsAt <= now` 不得当成有效额度。
