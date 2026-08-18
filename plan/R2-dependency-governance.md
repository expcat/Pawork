# R2 — 依赖治理

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R2 行。依据 2026-08-18 全依赖用面审计(38 crate 逐调用点)与 crates.io 版本对比:本地化 3 项、清理死声明、升级去重 8+ 项、rmcp 3.x 专项。在 R1 合并后的 21 包布局上执行(Cargo.toml 面更小)。
>
> 审计总结论:除下表动作项外,其余全部依赖(dunce/semver/globset/url/sha2/smol/unicode-segmentation/chardetng/blake3/chacha20poly1305/zeroize/reqwest/tokio/ignore/regex/tracing/clap/landlock/libc/tempfile/wiremock/proptest/async-trait/ts-rs/gpui 等)经逐调用点核查**保留**——理由属三类:安全/平台正确性关键(不自研密码学与路径/PTY/文件监视平台层)、已随其它依赖免费在树(零边际成本)、用面充分(async-trait 37/41 trait 走 dyn 注入,原生 AFIT 不支持 dyn)。tokio 全仓逐 crate 手工最小 feature 集,为模范状态。

## 1. 本地化(新增本地代码合计 ≈ 90–120 行)

| # | 动作 | 现用面 | 实施 | 风险 |
| --- | --- | --- | --- | --- |
| L1 | rand → `getrandom::fill()` | 6 个生产调用点,全部「填充 N 字节随机」:`foundation/protocol/src/client_auth.rs:97`(32B token)、`providers/auth/src/oauth.rs:131,145`(PKCE verifier/state)、`storage/blob/src/protected.rs:814,948,975`(nonce/盲化/抖动)(remote wire 已随 R0 归档) | ~10 行 diff;getrandom 已在传递树;顺带统一 thread_rng/OsRng 混用为 OS 熵 | 极低;生产树删 rand/rand_core/rand_chacha 三包 |
| L2 | parking_lot → `std::sync` | 9 处短临界区、无 condvar/timeout:`execution/exec/src/pty/mod.rs:19`、`vcs/git/src/cache.rs:49,57,72`、workflow plan/goal 服务(R0 后按存留核查) | ~30 行;毒锁策略统一 `unwrap_or_else(PoisonError::into_inner)` 并注释 | 低;CLI 生产树真少一包(gpui 树仍带,仅影响 desktop) |
| L3 | base64 → 本地 `base64url` 模块(无填充 encode/decode) | 仅 auth 一包 8 处,全部 `URL_SAFE_NO_PAD`:PKCE verifier/challenge(`oauth.rs:132,139,146`)、JWT payload 解码(`oauth.rs:1093`) | ~80 行 + golden 测试(与 base64 crate 输出逐字节对拍后再删依赖);PKCE 定向回归必跑 | 低;算法简单无平台差异 |

## 2. 版本升级与去重(以 2026-08-18 crates.io 快照为准,执行时重查)

| # | 依赖 | 现状(声明 / lock) | 目标 | 动机与注意 |
| --- | --- | --- | --- | --- |
| U1 | notify + notify-debouncer-full | `7` / 7.0.0+8.2.0 双版本;debouncer `0.5` | notify `8` + debouncer `0.7` | 消除双份平台监视栈;notify 8 API 有变,file_index/git cache 两处消费点迁移 |
| U2 | windows | `0.58`(exec 独用)/ lock 0.57+0.58+0.61.3 三版本 | `0.61` | 与 gpui 树 0.61.3 去重(0.57 随 sysinfo 无法消除;0.62 待 gpui 跟进);迁移面 ~10 处 Win32 调用 |
| U3 | portable-pty | `0.8.1` | `0.9` | 甩 winapi/nix 0.25/lazy_static 老栈;~8 API 面迁移 |
| U4 | ts-rs | `11` | `12` | typegen 专用不入生产树;`schemas/` 重新生成后 diff 审查(允许注释/格式变化,类型形状不得变) |
| U5 | reqwest | `0.12` / 0.12.28+0.13.4 双版本 | `0.13` | 去重;`rustls-tls/stream/json` 三 feature 保持;redirect/proxy API 核对 |
| U6 | toml | `0.8` / 0.8.23+1.1.4 双版本 | `1.1` | 去重;config/resources/compat/import 解析点回归(config 44 测试兜底) |
| U7 | rusqlite | `0.32` | `0.40` | bundled SQLite 随升(安全修复);storage 迁移/backup(`migration.rs:174` Backup API)定向回归 |
| U8 | sha2 | `0.10` / 0.10.9+0.11.0 双版本 | `0.11` | 与 desktop 树(rust-embed)去重;PKCE S256 回归 |
| U9 | base64 | `0.22` | (随 L3 删除) | — |
| U10 | directories | `5` | 评估后升 `6` 或显式锁定注释 | macOS `dev.pawork.pawork` 目录语义兼容为硬条件;升级前写路径快照测试 |
| U11 | thiserror 1.x 残留 | lock 1.0.69+2.0.20 | 不强求 | 1.x 来自传递依赖,随上游自然消失 |

## 3. rmcp 专项(波 C)

- 现状:`=2.2.0` 精确锁定,生产依赖(stdio + streamable-http 两条 transport 都走 rmcp),用面集中在单文件 `codec.rs`(12 处 import 面);crates.io 已到 3.1.x(major)。
- 动作:在分支上升级 → MCP 契约 59 测试 + rmcp 隔离断言 → 真实 MCP server 冒烟(`npx @modelcontextprotocol/server-filesystem`,S9 同款)→ wire 行为逐项对比。
- 决议规则:兼容则升(锁 `=3.1.x`);任何 wire/行为破坏则**维持 =2.2.0** 并在 ROADMAP §4 登记原因与复评条件。协议库 API 波动大,精确锁定策略本身保留。

## 4. 波次拆分

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| A | 本地化 L1–L3(先对拍 golden 再删依赖) | protocol/auth/storage/exec/git/workflow 的调用点 + 根 Cargo.toml | 并行 ×2(L1+L3 / L2) |
| B | 升级 U1–U8、U10(逐项独立可回退;每项升完跑该消费面定向测试) | 各消费 crate Cargo.toml + 调用点迁移 | 串行推荐(lock 冲突面小但叠加诊断困难;U1/U3 可并行) |
| C | rmcp 专项(§3) | tools `mcp/` + 根 Cargo.toml | 串行 |
| D | 收口:`cargo tree -d`(duplicates)断言——notify/reqwest/toml/sha2/windows(0.58 位)多版本消失;记录前后 lock 包数与增量编译耗时对比 | 根 Cargo.lock、本任务书 | 串行(主代理) |

## 5. 验证

- 每项动作:消费面 `cargo test -p <crate>` 定向绿;L1/L3 的密码学相关点(PKCE、protected nonce)与 base64 对拍 golden 必跑。
- 波 D:`cargo tree --duplicates` 输出归档;`cargo check -p pawork -p pawork-desktop` 全绿。
- 真实冒烟(矩阵一组):OAuth 通道登录态刷新一次(验证 PKCE/base64url/sha2 改动)+ MCP `mcp list/test`(若波 C 升级)。

## 6. 退出标准

- [ ] rand/parking_lot/base64 退出直接依赖;encoding_rs/futures 死声明已在 R0 清理(核对)
- [ ] U1–U8 完成;U10 有决议;lock 多版本项(thiserror 1.x、windows 0.57 除外)清零
- [ ] rmcp 有决议并落地(升级或锁定登记)
- [ ] 全部消费面定向测试绿 + 冒烟通过;v3_plan §3 指针更新
