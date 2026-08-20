# R2 — 依赖治理

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R2 行。依据 2026-08-18 全依赖用面审计(38 crate 逐调用点)与 crates.io 版本对比:本地化 3 项、清理死声明、升级去重 8+ 项、rmcp 3.x 专项。在 R1 合并后的 21 包布局上执行(Cargo.toml 面更小)。
>
> 审计总结论:除下表动作项外,其余全部依赖(dunce/semver/globset/url/sha2/smol/unicode-segmentation/chardetng/blake3/chacha20poly1305/zeroize/reqwest/tokio/ignore/regex/tracing/clap/landlock/libc/tempfile/wiremock/proptest/async-trait/ts-rs/gpui 等)经逐调用点核查**保留**——理由属三类:安全/平台正确性关键(不自研密码学与路径/PTY/文件监视平台层)、已随其它依赖免费在树(零边际成本)、用面充分(async-trait 37/41 trait 走 dyn 注入,原生 AFIT 不支持 dyn)。tokio 全仓逐 crate 手工最小 feature 集,为模范状态。

## 1. 本地化(新增本地代码合计 ≈ 90–120 行)

| # | 动作 | 现用面 | 实施 | 风险 |
| --- | --- | --- | --- | --- |
| L1 | rand → `getrandom::fill()` ✅ 波 A 落地 | 6 个生产调用点(波 A 重验零漂移):`crates/protocol/src/client_auth.rs`(32B token)、`crates/auth/src/oauth.rs`(PKCE verifier/state)、`crates/storage/src/blob/protected.rs`(nonce/盲化/抖动) | getrandom 0.4.3 已在传递树(tempfile 链),直接声明复用同版本;统一 thread_rng/OsRng 为 OS 熵;feature 镜像:`client-auth`/`protected` 的 dep:rand→dep:getrandom | 极低;CLI 生产闭包删 rand/rand_core/rand_chacha(lock 整体因 desktop/dev 树仍留) |
| L2 | parking_lot → `std::sync` ✅ 波 A 落地 | 波 A 重验:git cache 已随 R0 归档,但 `crates/workflow/src/plan/service.rs` 仍是活的使用点(12 处 .lock());`crates/exec/src/pty/mod.rs` 40 处;共 52 处机械替换;另清 orchestration 死声明(源码零使用) | 毒锁策略统一 `unwrap_or_else(PoisonError::into_inner)` 并注释;无 condvar/timeout | 低;CLI 生产闭包真少三件套(parking_lot/parking_lot_core/lock_api,gpui 树仍带) |
| L3 | base64 → 本地 `base64url` 模块(无填充 encode/decode) ✅ 波 A 落地 | 波 A 重验:生产仅 4 操作点(非 8 处),全部 `URL_SAFE_NO_PAD`:PKCE verifier/challenge/state encode(`oauth.rs`)与 JWT payload decode;另 2 个测试文件(oauth.rs、default_credential.rs)随迁 | `crates/auth/src/base64url.rs` + golden 测试(先与 base64 0.22.1 逐字节对拍,绿后固化 13 组固定向量再整体删依赖);补 decode 错误路径测试;PKCE 定向回归已跑 | 低;算法简单无平台差异 |

## 2. 版本升级与去重(以 2026-08-18 crates.io 快照为准,执行时重查)

| # | 依赖 | 现状(声明 / lock) | 目标 | 动机与注意 |
| --- | --- | --- | --- | --- |
| U1 | notify + notify-debouncer-full | `7` / 7.0.0 单版本(波 B 重验 2026-08-19:notify 8.2.0 与 debouncer 均不在 lock——本项是纯升级非去重;inotify 0.10.2 仍在 lock,为 notify 7 linux 后端,升后复查去留;根 workspace 的 `notify-debouncer-full` 死声明一并移除) | notify `8` ✅ 波 B 落地(2026-08-20;8.2.0,debouncer 声明已删);debouncer 无消费者不再引入 | 波 B 三处消费点在 8.2 下源兼容;整阶段复核补齐 `Flag::Rescan` fail-safe:notify 后端溢出/MUST_SCAN_SUBDIRS 的空路径事件转换为每个 workspace root 的 Upsert,触发全量重扫,并有回归测试;inotify 随后端自然更新 0.10.2→0.11.5 保留(notify 8 linux 后端,不应去除) |
| U2 | windows | `0.58`(exec 独用)/ lock 0.57+0.58+0.61.3 三版本(波 B 重验:crates.io 已至 0.62.2,目标仍取 0.61 与 gpui 主闭包去重,不追 0.62) | `0.61` ✅ 波 B 落地(2026-08-20;0.58 退出 lock,与 gpui 主闭包统一 0.61.3) | 0.57 仅为 gpui `screen-capture` 可选 feature 在 Windows 目标下经 zed-scap→sysinfo 的 lock 残留,默认 Desktop 闭包未激活;0.62 待 gpui 跟进;波 B 仅 2 处适配:BOOL 改从 windows::core 导入、IsProcessInJob 第二参改 Option<HANDLE>(传 Some,语义不变);x86_64-pc-windows-msvc 交叉 check 绿 |
| U3 | portable-pty | `0.8.1` | `0.9` ✅ 波 B 落地(2026-08-20;0.9.0) | nix 0.25 与旧 serial 四件套+termios/ioctl-rs/memoffset 退出,nix 更新至 0.28;winapi/lazy_static 仍由 portable-pty 0.9 的 Windows 目标传递,不宣称退出;唯一适配:改用 0.9 官方 `ExitStatus::signal()` 删除 0.8 时代 Display 解析 hack,返回值逐字节等价 |
| U4 | ts-rs | `11` | `12` ✅ 波 B 落地(2026-08-20;12.0.1) | typegen 专用不入生产树;`schemas/` 重新生成后 diff 审查——波 B 实测 264 文件中 7 个索引签名去 `?`(ErrorContext×3/JsonValue×3/CompatImportReport×1)属类型形状变化,触发停止条件;**用户 2026-08-20 拍板接受新形状(决议 A)**,登记于此:wire/serde 形状不变(26 个 wire golden 绿),仓内无 TS 消费方(desktop 为 GPUI),语义变化=map 值类型从 `string|undefined` 收窄为 `string`,与 Rust 生产者实际序列化更一致;弃用开关 `with_v11_hashmap` 未采用 |
| U5 | reqwest | `0.12` / 0.12.28+0.13.4 双版本 | `0.13` ✅ 波 B 落地(2026-08-20;0.13.4 单版本) | 去重;**上游强制变更(波 B 实测,审查 F1 登记)**:0.13 删除 `rustls-tls` 无 webpki-roots 变体,改 `rustls`=aws-lc-rs+rustls-platform-verifier(TLS 信任面变化:内置 Mozilla 根→系统信任库,系统 CA/吊销策略开始生效;aws-lc-sys 引入 cmake 构建依赖)并补 `form`(serde_urlencoded 转 optional);stream/json 保持;redirect `Policy::none()`/回环代理语义实测不变(S13-F06/F07 维持);如需旧信任栈只能留 0.12 或 `rustls-no-provider` 自配,均不取 |
| U6 | toml | `0.8` / 0.8.23+1.1.4 双版本 | `1.1` ✅ 波 B 落地(2026-08-20;1.1.4) | 波 B 实测调用面零适配,47 个 config 兜底测试升级前后双绿;pawork CLI 闭包去重成立,但 0.8.23 经 cbindgen→gpui build-dep 残留 desktop 树(波 B 新发现,例外登记同 sha2) |
| U7 | rusqlite | `0.32` | `0.40` ✅ 波 B 落地(2026-08-20;0.40.2) | bundled SQLite 随升 3.46→3.53.2(安全修复);调用面零适配;backup/restore/迁移前自动备份/只读恢复定向回归双绿;control-plane optional :22 与 dev :30 经 workspace 继承自动同步;Backup API 实态在 `sqlite/mod.rs:164-191`(迁移调用点 `sqlite/migration.rs:127-129`) |
| U8 | sha2 | `0.10` / 0.10.9+0.11.0 双版本 | `0.11` ✅ 波 B 落地(2026-08-20;0.11.0) | 与 desktop 树(rust-embed)去重;零适配,RFC 7636 golden 字节不变;0.10.9 经 gpui_http_client 残留为例外(见波 D 行登记) |
| U9 | base64 | `0.22` | (随 L3 删除) | — |
| U10 | directories | `5` / 5.0.1(最新 6.0.0) | ✅ 决议升 `6` 成立(2026-08-20;6.0.0) | macOS `dev.pawork.pawork` 目录语义兼容硬条件满足:快照 golden×2(workspace + auth),v5/v6 下逐字节一致,消费点零适配;整阶段复核关闭审查 F3:auth 路径抽为纯解析函数,确定性覆盖非空/空/缺失 PAWORK_HOME,两条 macOS 快照均以 `BaseDirs::home_dir()` 为 oracle;Windows 注释修正为 `%APPDATA%\pawork\pawork\config` |
| U11 | thiserror 1.x 残留 | lock 1.0.69+2.0.20 | 不强求 | 1.x 来自传递依赖,随上游自然消失(波 D 实测:CLI 闭包 1.0.69←portable-pty 0.9→filedescriptor 0.8.3 仍在;desktop 树另有 async_zip/postage/tokio-socks 传递;同期 CLI 闭包新增传递残留 base64 0.22/0.23、syn 2/3,见波 D 行登记) |

## 3. rmcp 专项(波 C)✅ 2026-08-20 落地(决议:升级,锁 `=3.1.3`)

- 现状(升级前):`=2.2.0` 精确锁定,生产依赖(stdio + streamable-http 两条 transport 都走 rmcp),用面集中在单文件 `codec.rs`(实态 6 条 `use` / 17 个生产类型);crates.io 已到 3.1.x(major)。
- 动作:在分支上升级 → MCP 契约测试(波 C 64 条;整阶段复核为 InputRequiredResult fail-closed 新增 1 条,当前 65 条;原「59」为 S9A 快照计数) + rmcp 隔离断言 → 真实 MCP server 冒烟(`npx @modelcontextprotocol/server-filesystem`,S9 同款)→ wire 行为逐项对比。
- 决议规则:兼容则升(锁 `=3.1.x`);任何 wire/行为破坏则**维持 =2.2.0** 并在 ROADMAP §4 登记原因与复评条件。协议库 API 波动大,精确锁定策略本身保留。
- 结果(2026-08-20,波 C):兼容成立,升级落地,精确锁 `=3.1.3`。写入集:`crates/tools/Cargo.toml`(=3.1.3,删除死声明 `macros` dev feature)+ `crates/tools/src/mcp/codec.rs`(适配 `#[non_exhaustive] ServerResult` 与 `CallToolResponse`;整阶段复核抽出结果转换并锁定 InputRequiredResult fail-closed)+ 根 `Cargo.toml`(`rust-version` 1.85→1.88)+ `Cargo.lock`(830→826)。当前验证:`cargo test -p pawork-tools` 130/130 绿(65 条 MCP + 隔离断言全过)。波 C 当时另记录消费者 check 与真实 stdio `mcp list/test ± --json` 四项通过;2.2.0 基线原始输出未归档,故「逐字节一致」仅保留为历史人工结论,不作仓内可复现门禁。`serve()` 保持 legacy initialize,ping/call_tool 语义不变;自研 oauth.rs 与公共 API 零变化。

## 4. 波次拆分

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| A | 本地化 L1–L3(先对拍 golden 再删依赖)✅ 2026-08-19 收口 | protocol/auth/storage/exec/workflow 的调用点 + orchestration 死声明 + 根 Cargo.toml | 并行 ×2(L1+L3 / L2) |
| B | 升级 U1–U8、U10(逐项独立可回退;每项升完跑该消费面定向测试)✅ 2026-08-20 收口(九项全落地;lock 836→830;CLI 直控面多版本清零;默认 desktop 闭包例外 sha2/toml/thiserror,windows 0.57 为可选 screen-capture 的 lock 残留;U4 形状变化用户拍板 A;U5 TLS 栈切换登记) | 各消费 crate Cargo.toml + 调用点迁移 | 串行推荐(lock 冲突面小但叠加诊断困难;U1/U3 可并行) |
| C | rmcp 专项(§3)✅ 2026-08-20 收口(决议升级锁 =3.1.3;MSRV 1.85→1.88;lock 830→826) | tools `mcp/` + 根 Cargo.toml + Cargo.lock | 串行 |
| D | 收口 ✅ 2026-08-20:归档的默认目标 `cargo tree --duplicates` 输出 [R2-cargo-tree-duplicates-2026-08-20.txt](R2-cargo-tree-duplicates-2026-08-20.txt) 可复现 notify 8.2.0/reqwest 0.13.4 单版本及默认 desktop 的 sha2 0.10.9、toml 0.8.23、thiserror 1.x 例外;windows 0.58 退出由 Cargo.lock 证明,0.57 仅在 Windows 目标显式启用 `gpui/screen-capture` 时经 zed-scap→sysinfo 激活,不在归档的默认闭包。**CLI 闭包传递残留**:base64 0.22.1(reqwest/hyper-util)/0.23.1(rmcp)、syn 2.x(tracing-attributes/thiserror 1/ICU derives)与 3.x(async-trait/clap/serde/tokio macros)、thiserror 1.x(portable-pty→filedescriptor);pawork 直控面已单版本。lock 836→830→826(净 -10);当时本机增量编译约 2.1s→2.3s、干净 check 单元 242→221及 xAI OAuth/MCP stdio 冒烟均无原始日志归档,保留为历史人工记录而非仓内可复现门禁;`cargo check -p pawork -p pawork-desktop` 当时双绿 | 根 Cargo.lock、本任务书 | 串行(主代理) |

## 5. 验证

- 每项动作:消费面 `cargo test -p <crate>` 定向绿;L1/L3 的密码学相关点(PKCE、protected nonce)与 base64 对拍 golden 必跑。
- 波 D:`cargo tree --duplicates` 输出归档;`cargo check -p pawork -p pawork-desktop` 全绿。
- 真实冒烟(矩阵一组):OAuth 通道登录态刷新一次(验证 PKCE/base64url/sha2 改动)+ MCP `mcp list/test`(若波 C 升级)。

## 6. 退出标准

- [x] rand/parking_lot/base64 退出直接依赖(波 A ✅;encoding_rs 为 tools 非死依赖见 manifest 注释、futures 三crate在用,均已核对)
- [x] U1–U8 完成;U10 有决议(波 B ✅ 2026-08-20);整阶段复核已补 notify Rescan fail-safe 与 U10 确定性路径测试。pawork 直控面多版本已清零;闭包上游残留与默认/可选 Desktop 口径已按 §4 波 D 行登记
- [x] rmcp 有决议并落地(波 C ✅ 2026-08-20:升级锁 `=3.1.3`,见 §3 结果)
- [x] 全部消费面定向测试绿;整阶段复核后 tools 为 130/130。波 C/D 的真实 stdio 与 OAuth 冒烟为历史人工通过记录,原始输出未归档,不再表述为仓内可复现的逐字节门禁
- [x] 波 D 收口登记(2026-08-20:默认目标 tree 归档 + lock 包数 + 默认/可选 Desktop 例外 + CLI 传递残留);无工件的编译数字与外部冒烟已明确降为历史记录

## 7. 整阶段复核（2026-08-20）

- 审查范围：按 `v3_plan.md` 对已标记完成的 R2 波 A–D 做源码、依赖树、测试覆盖与文档证据复核，并由 Grok 4.6 分别只读审查本地化语义、升级/依赖树、rmcp 兼容与状态记录。
- 修复结果：补齐 notify 8 的 `Flag::Rescan` 全量重扫、directories 6 的确定性路径 golden、rmcp 3.1 `InputRequiredResult` fail-closed 回归；同时修正 PTY 毒锁注释、Windows 目录说明及依赖残留/历史冒烟证据口径。
- 自动门禁：`cargo test -p pawork-workspace`、`-p pawork-auth`、`-p pawork-tools`、`-p pawork-exec`、`-p pawork-protocol -p pawork-workflow`、`-p pawork-storage --features protected` 全绿；typegen `--check`、`cargo check -p pawork -p pawork-desktop`、Windows 目标 `cargo check -p pawork-exec --target x86_64-pc-windows-msvc` 全绿；默认与可选 Desktop/CLI `cargo tree` 断言与 `Cargo.lock` 版本断言成立。
- 人工/外部门禁：本轮未重跑 OAuth 登录与真实 MCP server；既有通过记录缺少原始工件，已统一标为历史人工记录，不作为仓内可复现门禁。
- 状态：R2 保持完成，`v3_plan.md` 当前指针仍为 R3 波 A；本轮不跨阶段。
