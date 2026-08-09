# Phase 1 Review：配置系统、SQLite Actor、诊断与 CLI 骨架

- **日期**：2026-08-08
- **评审基线**：`main` @ `de76839`（工作树干净）
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置；后续再研究是否采纳）
- **范围**：ROADMAP.md Phase 1 的 12 个任务（P1-1 ~ P1-12）的完成情况、所引入包是否合适、是否存在更优替代或自实现替换的必要；另附「优先级 P1」标签任务现状。安全漏洞与优化点一并列出。

### 1. 结论摘要

1. **完成度可信**：P1-1 ~ P1-12 全部 🟢，12 个交付 crate 于 2026-08-08 复跑 `cargo test` 共 80 项测试全部通过；`clippy -D warnings` 与 `fmt --check` 干净。
2. **包选型总体合理**：P1 实际引用的包（rusqlite / blake3 / ignore / notify / tracing / clap / toml / serde / directories / dunce / regex 等）使用面都足够大、生态成熟，**没有发现「只用了很小一部分、自实现更有利」的包**，不建议自实现替换。反向看，基线已正确地把 metrics 采集、配置合并（未用 config-rs）、日志 redaction 划为自实现，边界划分是对的。
3. **主要问题在基线管理而非选型**：`uuid`、`tracing-appender`、`similar` 在 workspace 基线中声明但全仓库零引用；`parking_lot`、`tempfile`、`base64`/`rand`/`sha2`/`url` 六个依赖已引入但未回填基线，违反「新增依赖必须同步回基线」规则。
4. **`notify-debouncer-full` 归属名不副实**：基线把它记在 P1-8（文件索引），但 file-index 实际自实现去抖；真实使用者是 git-service 的缓存失效器。建议统一实现或修订基线描述。
5. **两个应尽快处理的风险**：Event Store 将整个事件信封（含 provider options）序列化落库，Secret 可能落库，触碰「Secret 不落库」红线；`trust_workspaces` 配置字段尚未被消费，但一旦接线即存在不可信仓库自我提权的攻击面。
6. **版本滞后（不急）**：rusqlite 0.32（上游最新 0.40.x）、toml 0.8（上游已到 1.x），建议立独立评估任务再升级。

### 2. P1 任务完成情况核对表

| 任务 | 交付 crate | 状态 | 关键证据 |
| --- | --- | --- | --- |
| P1-1 配置系统 | `config-service` | 🟢 | [schema.rs](../../crates/config-service/src/schema.rs)、[loader.rs](../../crates/config-service/src/loader.rs)；层级合并、profile overrides 均有测试 |
| P1-2 SQLite Actor | `app-database` | 🟢 | 串行 Actor、WAL、备份与只读恢复连接 |
| P1-3 schema 与迁移 | `session-store`（migration） | 🟢 | 与 P1-1 同期创建于提交 `63c776d` |
| P1-4 Event Store | `session-store` | 🟢 | [event_store.rs](../../crates/session-store/src/event_store.rs)：append / 按 sequence 重放 |
| P1-5 Projection | `session-store` | 🟢 | 可重建投影，27 项测试覆盖 session-store |
| P1-6 Blob Store | `artifact-store` | 🟢 | BLAKE3 寻址、引用计数、GC、磁盘预算 |
| P1-7 Workspace 服务 | `workspace-service` | 🟢 | 多 root、信任状态、Git 检测 |
| P1-8 文件索引 | `file-index` | 🟢 | ignore 遍历 + 自实现去抖监听（18 项测试） |
| P1-9 结构化日志 | `diagnostics`（logging） | 🟢 | tracing + 自实现 redaction、采样不丢 warn/error |
| P1-10 Metrics | `diagnostics`（metrics） | 🟢 | SQLite Actor 内自实现采集 |
| P1-11 诊断包导出 | `diagnostics`（bundle） | 🟢 | 脱敏导出、离线 JSON、import 往返测试 |
| P1-12 CLI Host | `cli-host` / `cli-command` / `cli-renderer` / `apps/pawork` | 🟢 | serve/run/shell/watch 骨架，`pawork` 为唯一正式宿主 |

**门禁证据（2026-08-08 复核）**：

- `cargo test`（上述 12 crate）：**80 passed / 0 failed**。
- `cargo clippy --all-targets -- -D warnings`：干净。
- `cargo fmt --check`：干净。
- 各任务 plan 文档（`plan/P1-*.md`）验收项均已勾选；`app-service` 仍为骨架属预期（P13-1 完整化）。

### 3. 包选型评估

#### 3.1 建议保留（自实现不值得）

| 包 | 版本 | 使用点 | 使用面评估 | 结论 |
| --- | --- | --- | --- | --- |
| `rusqlite` | 0.32（bundled+backup） | P1-2/3/4/5/6 | SQLite FFI 绑定 + backup API，全仓库数据层基座；sqlx 的异步池与「单连接 Actor」设计不匹配（基线已论证） | **保留**；版本升级见 §6 |
| `blake3` | 1 | P1-6 | Blob 内容寻址全量使用，SIMD 加速自实现不可企及 | **保留** |
| `ignore` | 0.4 | P1-8 | WalkBuilder + GitignoreBuilder，遍历与忽略规则核心；ripgrep 同源久经考验 | **保留** |
| `notify` | 7 | P1-8 | 跨平台文件监听抽象；自实现 ReadDirectoryChangesW/inotify/FSEvents 成本高且易错 | **保留** |
| `tracing` + `tracing-subscriber` | 0.1 / 0.3 | P1-9 | 结构化日志骨干；redaction 已按基线自实现，边界正确 | **保留** |
| `clap` | 4 | P1-12 | derive 宏最小胶水，命令树后续还会扩张 | **保留** |
| `toml` | 0.8 | P1-1 | 仅解析（serde 反序列化）；若未来需要「写回且保留注释」才换 `toml_edit` | **保留**；升级 1.x 见 §6 |
| `serde` / `serde_json` / `thiserror` / `tokio` | 基线版本 | 全局 | 基础设施，无争议 | **保留** |
| `directories` | 5 | P1-12、app-service | 三平台系统目录标准路径，自实现覆盖不全 | **保留** |
| `dunce` | 1 | config-service | Windows 路径简化，小而准；但覆盖面需扩大（见 V3） | **保留** |
| `regex` | 1 | P1-9/11 | redaction 规则引擎 | **保留** |

#### 3.2 需要重新评估的项

| 项 | 现状 | 选项 | 建议 |
| --- | --- | --- | --- |
| `uuid` | 基线声明（v4/v7/serde，[Cargo.toml:72](../../Cargo.toml)）但**全仓库零引用**；ID 均为 `string_id!` newtype，唯一性靠 DB 主键 | a) 移出基线，确立 ID 生成策略文档，未来需要时再引入；b) 立即启用 | **建议 a**。跨节点全局唯一 ID（分布式 session/event 场景）出现时，UUIDv7 仍是标准选择，届时引入不迟；当前声明属「基线虚置」 |
| `tracing-appender` | 基线声明但零引用；日志仅存内存 ring buffer，无文件/滚动输出 | a) 移出基线，待日志落盘任务立项再引入；b) 现在补文件输出 | **建议 a**，除非 P1-9 验收范围确认包含落盘 |
| `similar` | 基线记为 P7-3 使用，但 diff-service 实际解析 git 结构化输出，全仓库零引用（仅 [parser.rs](../../crates/diff-service/src/parser.rs) 注释中出现 similarity 字样） | a) 移出基线；b) 保留备用 | **建议 a**。未来若需进程内 diff（不经 git）再引入 |
| `notify-debouncer-full` 0.5 | 基线记 P1-8 使用；实际 file-index 自实现去抖（固定窗口，256 有界通道，[lib.rs:317](../../crates/file-index/src/lib.rs)、[lib.rs:384](../../crates/file-index/src/lib.rs)），真实使用者是 git-service 缓存失效器（[cache.rs:137-165](../../crates/git-service/src/cache.rs)） | a) file-index 改用 debouncer（免费获得同路径事件合并）；b) 保留自实现但修订基线归属并修复 blocking_send 风险（V7） | **倾向 a**：统一实现顺带解决 V7；若坚持自实现，需在基线中写明理由 |
| `rusqlite` 0.32 → 0.40.x | 落后上游多个 major（上游最新 0.40.1，bundled SQLite 版本与构建脚本均有改进） | 独立升级任务：API 差异评估 + 全量门禁回归 | **立项评估**，不与本阶段混改 |
| `toml` 0.8 → 1.x | 上游已发布 1.x（API 大体兼容） | 直接升级；写回需求出现时改评估 `toml_edit` | **顺手可做**，低风险 |

#### 3.3 「自实现替换包」总体判断

针对「引用面小 → 自实现换取可控性/性能/扩展性」的命题：**P1 范围内没有命中的包**。每个被引用的包使用面都覆盖了其核心价值区；真正「只用一小部分」的是三个零引用声明（uuid / tracing-appender / similar），正确动作是**移出基线**而不是自实现。相反，当前自实现的部分（metrics 采集、配置合并、redaction、去抖）中，去抖是唯一值得重新权衡的——因为它与现成 debouncer 功能重叠且带有 V7 风险。

### 4. 基线偏差清单

规则来源：ROADMAP「依赖选型基线」要求新增依赖同步回填基线表。

| 类型 | 项 | 位置 | 说明 |
| --- | --- | --- | --- |
| 声明未引用 | `uuid` | [Cargo.toml:72](../../Cargo.toml) | 见 §3.2 |
| 声明未引用 | `tracing-appender` | [Cargo.toml:86](../../Cargo.toml) | 见 §3.2 |
| 声明未引用 | `similar` | [Cargo.toml:127](../../Cargo.toml) | 见 §3.2 |
| 引入未登记 | `parking_lot = "0.12"` | [git-service/Cargo.toml:22](../../crates/git-service/Cargo.toml) | Phase 7 引入 |
| 引入未登记 | `tempfile = "3"` | [git-service/Cargo.toml:26](../../crates/git-service/Cargo.toml)、[diff-service/Cargo.toml:20](../../crates/diff-service/Cargo.toml)（dev） | 测试依赖也应登记 |
| 引入未登记 | `base64 = "0.22"`、`rand = "0.8"`、`sha2 = "0.10"`、`url = "2"` | [auth-service/Cargo.toml:14-22](../../crates/auth-service/Cargo.toml) | Phase 6 引入 |

**建议**：一次小型清理任务统一处理——删除 3 个零引用声明、回填 6 个未登记依赖（或说明豁免理由），并同步 ROADMAP 基线表。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V8）。

#### V1 [安全·高] `trust_workspaces` 自我提权攻击面

字段定义于 [schema.rs:43](../../crates/config-service/src/schema.rs)，合并逻辑会接受 workspace 层覆盖（[schema.rs:147-148](../../crates/config-service/src/schema.rs)），当前无消费者。一旦后续接线，恶意仓库只需在 `.pawork/config.toml` 写入 `trust_workspaces = true` 即可让工作区自授信任，绕过信任闸门。**建议**：趁未消费，将该字段限定为仅全局层可读（workspace 层覆盖直接忽略并告警），补回归测试；同时在 plan 中记录该约束。

#### V2 [安全·高] Event Store 持久化整个信封，Secret 可能落库

[event_store.rs:143-144](../../crates/session-store/src/event_store.rs) 将 `AgentEventEnvelope` 整体 `serde_json::to_string` 写入 `payload_json`（含 provider-specific options）。若任何事件携带 token/密钥字段，将直接违反「Secret 不落库」红线。**建议**：在序列化边界增加 redaction guard（对 options/headers 类字段白名单或掩码），并加契约测试：构造携带假 token 的事件，断言落库 JSON 中不含明文。此项建议列为 P0 后续任务。

#### V3 [健壮性·中] Windows verbatim 路径流入子进程

workspace-service 用 `fs::canonicalize` 归一化 root（[lib.rs:148](../../crates/workspace-service/src/lib.rs)、[lib.rs:234](../../crates/workspace-service/src/lib.rs)、[lib.rs:310](../../crates/workspace-service/src/lib.rs)），Windows 上产生 `\\?\` 前缀路径；git-service 直接把 cwd 传给子进程（[process.rs:77](../../crates/git-service/src/process.rs)），未做 `dunce::simplified`。部分工具对 verbatim 路径不兼容，属 P11-8 的潜在缺陷。**建议**：在 process_runtime 设置 cwd 处或 workspace-service 出口统一 `dunce::simplified`。

#### V4 [健壮性·中] artifact-store 崩溃残留 `.tmp-` 文件无人清理

`atomic_write` 先写 `.tmp-` 临时文件再 rename（[lib.rs:565-582](../../crates/artifact-store/src/lib.rs)）；进程崩溃会留下孤儿文件。GC 扫描刻意跳过 `.tmp-`（[lib.rs:593-609](../../crates/artifact-store/src/lib.rs)）但从不回收，长期磁盘泄漏。**建议**：GC 附带清理 mtime 超过阈值（如 24h）的 `.tmp-` 文件。

#### V5 [健壮性·低] 诊断包导出 TOCTOU

[bundle.rs:179-204](../../crates/diagnostics/src/bundle.rs)：先 `destination.exists()` 检查，再 `fs::rename` 落位；检查与 rename 之间的窗口内 rename 会覆盖已存在文件，与注释声称的「不覆盖已有文件」矛盾。**建议**：改用带序号/时间戳的命名策略，或 rename 前以 create-new 语义锁定目标。

#### V6 [安全·中] redaction 为 best-effort，残余风险未文档化

诊断包与日志的脱敏基于正则，无法穷尽形态（URL query 参数、嵌套 JSON 转义、自定义 header 等）。**建议**：在 `docs/features/` 与 `docs/quality/security-acceptance.md` 明确「诊断包视为可能含敏感数据，分享前需人工确认」，并把典型漏报形态列入测试样本持续回归。

#### V7 [性能/健壮性·中] file-index 去抖回调 `blocking_send` 有阻塞风险

notify 回调里对 256 容量通道做 `blocking_send`（[lib.rs:251](../../crates/file-index/src/lib.rs)、[lib.rs:317](../../crates/file-index/src/lib.rs)）。事件风暴（大仓库 checkout/切分支）且下游消费滞后时，watcher 线程被阻塞，OS 事件缓冲可能溢出丢事件。**建议**：`try_send` 失败时合并/丢弃并计数，或按 §3.2 统一改用 debouncer。

#### V8 [健壮性·低] file-index 错误列表无界

`errors: Arc<Mutex<Vec<String>>>` 只增不减（[lib.rs:312](../../crates/file-index/src/lib.rs)、[lib.rs:319](../../crates/file-index/src/lib.rs)），持续错误风暴下内存无界增长。**建议**：设置上限（如 1024）并环形淘汰，导出时标注截断。

### 6. 优化建议（按优先级）

#### P0（建议在下一阶段开工前处理）

1. **V2**：Event Store 序列化边界脱敏 + 契约测试（红线问题）。
2. **V1**：`trust_workspaces` 限定全局层（未消费时修改成本最低）。

#### P1（近期排期）

3. **artifact-store `put()` 性能**：当前在 DB Actor 闭包内完成 `blake3::hash` + 读盘比对 + `atomic_write`（[lib.rs:204-260](../../crates/artifact-store/src/lib.rs)）。大 blob 场景（ADR-018 大载荷）会阻塞**所有** DB 操作。建议：Actor 外先算 hash 与落盘（或 `spawn_blocking`），Actor 只处理元数据与引用计数。
4. **基线清理**：§4 清单一次性处理（改动面仅 Cargo.toml 与 ROADMAP 表）。
5. **去抖实现统一**：file-index 与 git-service 二选一（倾向 debouncer），顺带解决 V7。
6. **cli-host shell 阻塞**：`io::stdin().lock().lines()` 同步读取（[lib.rs:87](../../crates/cli-host/src/lib.rs)）阻塞 async worker 线程；改 `tokio::io::stdin` 或 `spawn_blocking`。

#### P2（顺手/评估项）

7. `rusqlite` 0.32 → 0.40.x、`toml` 0.8 → 1.x 升级评估（独立小任务，全量门禁回归）。
8. `is_ignored` 每文件重建 GitignoreBuilder（[lib.rs:487](../../crates/file-index/src/lib.rs)、[lib.rs:535-536](../../crates/file-index/src/lib.rs)）：按批/按 root 缓存 builder，大仓库初扫收益明显。
9. 初扫二进制探测每文件读 8KB（[lib.rs:53](../../crates/file-index/src/lib.rs)、[lib.rs:590](../../crates/file-index/src/lib.rs)）：可改惰性探测，或复用基线已声明的 `content-inspector`。
10. Metrics 仅有 count/sum/min/max（[metrics.rs:96-161](../../crates/diagnostics/src/metrics.rs)），无 p50/p95/p99，性能门禁判定力不足：补固定桶直方图或轻量分位数结构。
11. Config Loader 仅保留最后一个文件错误（`pending_error` 覆盖式，[loader.rs:57](../../crates/config-service/src/loader.rs)、[loader.rs:106](../../crates/config-service/src/loader.rs)）：多文件同时损坏时诊断信息丢失，建议聚合为错误列表。
12. `payload_json` 与独立列重复存储信封字段（[event_store.rs:205-206](../../crates/session-store/src/event_store.rs)）：评估瘦身（信封只存 payload，结构化字段全走列），权衡查询便利与存储开销。

### 7. 附录：「优先级 P1」标签任务现状（Phase 1 之外）

| 任务 | 状态 | 说明 |
| --- | --- | --- |
| P7-7 Hunk/Line stage | 🟢 已完成 | 随 Phase 7 交付 |
| P7-8 commit/branch/stash/log/show 等 | 🟢 已完成 | 随 Phase 7 交付 |
| P9-7 MCP OAuth | ⚪ 未开始 | 基线已选 `oauth2 = 5`；开工前复核授权码流与本地回调安全 |
| P11-5 Docker/Podman 沙箱 | ⚪ 未开始 | 容器 CLI 调用路径；与 ADR-031 的回退策略保持一致 |

### 8. 建议的后续动作（本次未执行，供研究）

1. 对 V1/V2 立项（安全红线优先）。
2. 基线清理小任务（§4），一次提交完成。
3. 去抖统一方案讨论（影响 file-index 与 git-service 两处）。
4. artifact-store 大载荷路径重构（P11 沙箱与 P12 worker 会放大该瓶颈）。
5. 版本升级评估窗口（rusqlite / toml）。

---

*评审方法：以 `de76839` 为基线，逐项核对 ROADMAP/plan 状态、源码与依赖清单，并复跑 12 个 Phase-1 crate 的测试与静态门禁；文中所有结论均给出文件与行号级证据。本文档仅为评审记录，不代表已批准的变更。*

---

## 修复记录（review-remediation）

> Phase 1 · 基础设施 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P1-1 ~ P1-12

**最终目的**：消除 [REVIEW.md](../../REVIEW.md) §1（Phase 1）评审发现的安全红线、健壮性缺陷与基线卫生问题——让「Secret 不落库」红线被 Event Store 序列化边界的脱敏与契约测试守护，关闭 `trust_workspaces` 的自我提权面，收敛 file-index/artifact-store 的阻塞与无界增长隐患，并使 workspace 基线声明与实际依赖一一对应。

**涉及范围**：`config-service`、`session-store`、`artifact-store`、`diagnostics`、`file-index`、`workspace-service`、根 `Cargo.toml`、ROADMAP「依赖选型基线」

### 细分步骤（分组）

#### A. 安全红线（V1 / V2）

1. **V2 Event Store 脱敏**：在 `session-store/src/event_store.rs` 序列化 `AgentEventEnvelope` 入 `payload_json` 的边界增加 redaction guard（对 options/headers/token 类字段白名单或掩码），断言落库 JSON 不含明文 secret。目的：守住「Secret 不写入数据库」红线（[ADR-014](../../docs/adr/ADR-014-secret-os-keychain.md)）。
2. **V1 trust_workspaces 收口**：在 `config-service` 仅保留 builtin 安全默认值与 global 用户配置的生效权，workspace 等更高层覆盖直接忽略并告警，补回归测试。目的：趁字段未消费时消除自我提权攻击面。

#### B. 健壮性与安全加固（V3 ~ V8）

3. **V3 verbatim 路径**：在 `workspace-service` 出口统一 `dunce::simplified`，消除 Windows `\\?\` 前缀流入子进程 cwd。目的：与 P7-9 V6 / P11-8 同根，本任务收口 workspace 出口。
4. **V4 临时文件残留**：`artifact-store` GC 扫描附带清理 mtime 超阈值（24h）的 `.tmp-` 孤儿文件。目的：修复崩溃残留的磁盘泄漏。
5. **V5 诊断包 TOCTOU**：`diagnostics` bundle 落位改用 create-new 语义或带序号/时间戳命名，关闭「先 exists 再 rename」的覆盖窗口。目的：兑现「不覆盖已有文件」注释承诺。
6. **V6 redaction 残余风险文档化**：在 `docs/features/` 与 `docs/quality/security-acceptance.md` 写明诊断包脱敏为 best-effort、分享前需人工确认，并把典型漏报形态（URL query、嵌套 JSON 转义、自定义 header）纳入回归样本。目的：让残余风险可见、可测。
7. **V7 file-index 阻塞回调**：将 notify 回调的 `blocking_send` 改为 `try_send`，满时合并/丢弃并计数（或按基线统一改用 debouncer）。目的：消除事件风暴下 watcher 线程阻塞与 OS 事件缓冲溢出。
8. **V8 file-index 错误无界**：给 `errors` 列表设上限（1024）并环形淘汰，导出标注截断。目的：修复错误风暴下内存无界增长。

#### C. 基线与包清理

9. **零引用声明**：从根 `Cargo.toml` 移除 `uuid`、`tracing-appender`（全仓库零引用；`similar`、`parking_lot`/`tempfile` 归 P7-9，`base64`/`rand`/`sha2`/`url` 归 P6-14，避免跨任务改同一行）。目的：基线声明与实际依赖一致。
10. **debouncer 归属订正**：在 ROADMAP 基线把 `notify-debouncer-full` 关联任务补 P7-6（真实首用为 git-service 缓存失效器），并在 plan 记录 file-index 与 git-service 去抖统一决策。目的：基线描述名副其实。

#### D. 文档同步

11. 同步 ROADMAP「依赖选型基线」本任务所辖行；按 V6 更新诊断/security 文档。目的：文档与代码一致。

### 主要产出物

- Event Store 脱敏 guard + 契约测试；`trust_workspaces` 全局层限制 + 回归测试
- artifact-store `.tmp-` 清理、diagnostics bundle create-new、file-index try_send + 有界 errors
- 根 `Cargo.toml` 基线清理（uuid/tracing-appender）；ROADMAP 基线同步

### 验收标准（保留 REVIEW 追踪编号）

- [x] **V2**：构造携带假 token 的事件写入 Event Store，断言 `payload_json` 不含明文 token（契约测试通过）
- [x] **V1**：workspace 层 `trust_workspaces = true` 被忽略并告警，仅全局层生效（回归测试）
- [x] **V3**：workspace-service 出口对 Windows verbatim 路径应用 `dunce::simplified`（路径测试覆盖）
- [x] **V4**：GC 清理 mtime > 24h 的 `.tmp-` 文件（构造孤儿文件验证回收）
- [x] **V5**：diagnostics bundle 落位不再覆盖既有文件（create-new/序号命名，TOCTOU 测试）
- [x] **V6**：`docs/features/*` 与 `security-acceptance.md` 写明诊断包 best-effort 脱敏与人工确认要求
- [x] **V7**：file-index watcher 回调改 `try_send`，风暴场景不阻塞（并发测试）
- [x] **V8**：file-index `errors` 上限 1024 环形淘汰并标注截断（测试）
- [x] **基线**：`uuid`、`tracing-appender` 从根 `Cargo.toml` 移除（或补豁免理由），ROADMAP 基线表同步
- [x] **归属**：`notify-debouncer-full` 基线关联 P7-6，去抖统一方案记录于 plan
- [x] **快速验证**：只运行本任务涉及 crate 的定向测试与必要 `cargo check -p <crate>`；Phase 1～7 remediation 全部收尾后统一执行 Core 主干 L2，不在本任务重复 workspace 全量门禁

**相关文档**：[REVIEW.md](../../REVIEW.md) §1 · [ADR-014 Secret 走 OS Keychain](../../docs/adr/ADR-014-secret-os-keychain.md) · [security-acceptance](../../docs/quality/security-acceptance.md) · [ROADMAP 依赖选型基线](../../ROADMAP.md#依赖选型基线)

> 基线去留决策（2026-08 review）：`uuid`/`tracing-appender` 暂无消费者，移出基线；未来需要全局唯一 ID 或日志落盘时再按基线流程重新引入。

> 去抖归属决策（P1-13）：`file-index` 保留基于 `notify`、有界通道与扫描级合并的轻量实现，回调只做非阻塞入队并对过载计数；`git-service` 的 P7-6 继续使用 `notify-debouncer-full` 做路径级缓存失效。两者共享「回调不阻塞、过载可观测、消费端合并」约束，但不为表面统一强制使用同一实现；ROADMAP 基线据此把 debouncer 的真实首用关联到 P7-6。

### 验证记录（2026-08-09）

- `cargo test -p config-service -p session-store -p artifact-store -p diagnostics -p file-index -p workspace-service`：78 passed，0 failed；对应 doc tests 通过。
- 同范围 `cargo check` 与 `cargo clippy --all-targets -- -D warnings` 通过；定向 `cargo fmt -- --check` 与 `git diff --check` 通过。
- 安全/并发模型审查发现复合敏感键漏报后，已补 `secret_key`、`secret_access_key`、`AWS_SECRET_ACCESS_KEY`、`password_hash` 的事实表 + Projection + replay 契约回归，并保留 TokenUsage 计数语义。
