# REVIEW — Pawork Phase 1–7 完成情况与包选型评审（整合版）

- **日期**：2026-08-08
- **评审基线**：Phase 1 以 `de76839` 为基线；Phase 2–7 以 `67d6c4d` 为基线（工作树除未跟踪的 REVIEW 文档外干净）
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置；后续再研究是否采纳）
- **范围**：ROADMAP.md Phase 1–7 各任务的完成情况、包选型、基线偏差、漏洞与优化点。本文由原 `REVIEW.md`（P1）与 `REVIEW-P2.md`…`REVIEW-P7.md` 合并而成，各阶段内容原样保留（仅调整标题层级），并在开篇增加「§0 跨阶段总览」提供整合视角。
- **V 编号约定**：各阶段内部独立使用 V1–Vn 编号（与原文一致）。跨阶段表格中以 `P<阶段>-V<n>` 前缀区分，避免歧义。

## 目录

- 0. 跨阶段总览
  - 0.1 门禁与完成度总览
  - 0.2 系统性问题：组件齐全、主干未通电
  - 0.3 跨阶段「安全/红线」问题索引
  - 0.4 跨阶段基线偏差总表
  - 0.5 plan 文档同步状态
  - 0.6 测试可信度提示
- 1. Phase 1（P1）
- 2. Phase 2（P2）
- 3. Phase 3（P3）
- 4. Phase 4（P4）
- 5. Phase 5（P5）
- 6. Phase 6（P6）
- 7. Phase 7（P7）
- 整合说明

## 0. 跨阶段总览

本节为整合新增，对七个阶段评审的共性结论做横向汇总；各阶段的逐任务证据与行号级证据见后文对应章节。

### 0.1 门禁与完成度总览

| Phase | 主要交付 crate | 测试（2026-08-08 复跑） | 静态门禁 | plan 同步 |
| --- | --- | --- | --- | --- |
| P1 配置/数据层 | config-service、app-database、session-store、artifact-store、workspace-service、file-index、diagnostics、cli-host(+命令/渲染)、apps/pawork | **80 passed / 0 failed** | clippy/fmt/schema-typegen 干净 | ✅ 全部勾选 |
| P2 Provider 运行时 | provider-runtime、provider-openai-compatible、auth-service、model-registry、test-support | **120 passed / 0 failed** | 干净 | ❌ 11 篇全 🟡未开始，19 框未勾 |
| P3 Agent Loop | agent-engine、context-engine、tool-runtime、agent-events | **89 passed / 0 failed** | 干净 | ❌ 10 篇全 🟡未开始，18 框未勾 |
| P4 工具/权限 | builtin-tools、policy-engine、checkpoint-service、process-runtime | **99 passed / 0 failed** | 干净 | ✅ 12 篇全勾（纠正 P2/P3 偏差） |
| P5 Session/Compaction | session-store（复用）、compaction-engine、context-engine（复用） | **63 passed / 0 failed**（3 crate） | 干净 | 多数已勾 |
| P6 三家 Provider | provider-openai、provider-anthropic、provider-google、provider-api、auth-service(+)、model-registry(+) | Phase-6 自有 94 / 含共享层 187 passed | 干净 | 各 plan 已勾 |
| P7 Git/Diff | git-service、diff-service | **72 passed / 0 failed** | 干净 | ✅ 已勾选 |

> 说明：P5/P6 与早期阶段共享 crate（如 session-store、context-engine、auth-service、model-registry），测试计数不可简单相加；各阶段复跑时仅统计其直接交付/复用 crate。

### 0.2 系统性问题：组件齐全、主干未通电

跨 P2–P6 反复出现同一模式：模块实现质量高、单测充分，但未接入主干循环，「测试绿」不等于「系统可用」。这是本轮评审最重要的系统性发现，建议作为 Phase 13 CLI Host 装配的前置/并行任务集中收口。

| 组件 / 能力 | 阶段 | 现状 | 详见 |
| --- | --- | --- | --- |
| `PolicyEngine::decide()` | P4 | 全仓库零生产调用，仅 13 处自测 | P4 §2 / V1 |
| `allowed_in_untrusted_workspace` | P4 | 全仓库零强制点 | P4 V1 |
| tool-runtime 调度器策略 | P4 | 仅用 `require_approval_for_writes` 布尔替代整套策略引擎 | P4 §2 |
| ToolScheduler ↔ ProviderLoop 桥接 | P3 | 不存在，两套独立实现从未组合 | P3 V9 |
| MessageQueue / RetryController / CancelHandle | P3 | ProviderLoop 零引用（用裸 CancellationToken） | P3 V3 / V7 |
| LoopSink 流式 delta 广播 | P3 | 整轮缓冲、从不广播 token 流 | P3 V2 |
| 多维预算（cost/duration/concurrency/artifact） | P3 | loop 中 4 维零记录，soft_warnings 不发事件 | P3 V5 / V6 |
| compaction-engine | P5 | 全 workspace 零消费者 | P5 §1-5 |
| context-engine / `trim_tool_result` | P5 | ContextBuilder 未调用 | P5 §1-5 |
| OAuth auto-refresh | P6 | `needs_refresh`/`refresh_access_token` 零消费者，轮换 token 不回写 | P6 V4 |
| `trust_workspaces` | P1 | 未消费（一旦接线存在自我提权面） | P1 V1 |

### 0.3 跨阶段「安全/红线」问题索引

下表汇总各阶段涉及安全与架构红线的项（Agent 红线「Secret 不落库」、「信任闸门」、「不可信输入执行」等），建议优先于「通电」之前处理。

| 编号 | 主题 | 阶段 | 类型 |
| --- | --- | --- | --- |
| P1-V1 | `trust_workspaces` 自我提权攻击面（未消费） | P1 | 安全·高 |
| P1-V2 | Event Store 持久化整个信封，Secret 可能落库（红线） | P1 | 安全·高 |
| P2-V6 | `provider_options` 无键保护，可覆盖 canonical 关键字段 | P2 | 安全·中 |
| P4-V1 | PolicyEngine 未接线，信任闸门运行时不存在 | P4 | 安全·高 |
| P4-V2 | 调度器硬编码上下文，checkpoint 跨 run 键碰撞 | P4 | 安全/正确性·高 |
| P4-V3 | apply_patch 回滚不完整，create 覆盖既有文件丢原内容 | P4 | 数据完整性·高 |
| P4-V4 | NeverAsk/OnFailure 无危险命令硬拒绝地板 | P4 | 安全·中 |
| P4-V5 | Windows env allowlist 缺 SYSTEMROOT/TEMP/TMP 等 | P4 | 安全/正确性·中 |
| P6-V1 | Google API key 写入 URL query 而非请求头 | P6 | 安全·中 |
| P6-V4 | OAuth refresh token 轮换不持久化 | P6 | 功能完整性·中 |
| P7-V1 | hunk stage 用可预测临时文件（符号链接竞争/源码外泄） | P7 | 安全·中 |
| P7-V2 | git 参数注入（位置参数未防前导 `-`） | P7 | 安全·中 |

### 0.4 跨阶段基线偏差总表

**声明未引用（基线声明但全仓库零引用，建议移出基线）**

| 依赖 | 声明位置 | 来源阶段 | 说明 |
| --- | --- | --- | --- |
| `uuid` | workspace 基线 | P1 | ID 均为 newtype，唯一性靠 DB 主键 |
| `tracing-appender` | workspace 基线 | P1 | 日志仅存内存 ring buffer，无落盘 |
| `similar` | workspace 基线 | P1 声明 / P7-3 未落地 | diff-service 实际解析 git 结构化输出，word-level diff 未实现 |
| `backon` | workspace + provider-runtime | P2 | 生产重试由 agent-engine 自实现，provider-runtime `ExponentialBackoff` 为死代码 |
| `arbitrary` | workspace 基线 | P2 | 无 `fuzz/` 目录，属性测试由 proptest 承担 |
| `content-inspector` | workspace 基线 | P4 | read_file 实际用 chardetng+encoding_rs |
| `oauth2` | workspace 基线 | P6 | OAuth 手写实现（PKCE/Device/refresh），零引用 |

**引入未登记（各 crate 引入但未回填 workspace 基线）**

| 依赖 | 位置 | 来源阶段 |
| --- | --- | --- |
| `futures` / `bytes` | workspace 根 Cargo.toml | P2 |
| `parking_lot` / `tempfile` | git-service / diff-service | P7（tempfile 亦用于 P2） |
| `base64` / `rand` / `sha2` / `url` | auth-service | P6（手写 OAuth） |

**crate 内死依赖（声明但该 crate 源码零引用）**

| 依赖 | crate | 阶段 |
| --- | --- | --- |
| `agent-domain` | policy-engine、checkpoint-service | P4 |
| `bytes` / `futures` | process-runtime | P4 |
| `serde_json` / `thiserror` | diff-service | P7 |

> 建议：一次小型基线清理任务统一处理以上三表，并在 CI 增加 `cargo machete`/`cargo udeps` 门禁。

### 0.5 plan 文档同步状态

| Phase | plan 同步状态 | 说明 |
| --- | --- | --- |
| P1 | ✅ 全部勾选 | 与 ROADMAP 一致 |
| P2 | ❌ 未同步 | 11 篇全 🟡未开始，19 个验收框未勾；提交未触碰 plan/ 与 docs/ |
| P3 | ❌ 未同步 | 10 篇全 🟡未开始，18 个验收框未勾；与 P2 同病 |
| P4 | ✅ 全部勾选 | 纠正了 P2/P3 的流程偏差，ROADMAP 同步 |
| P5 | ✅ 多数已勾 | 验收项大多已勾选 |
| P6 | ✅ 已勾选 | 各 plan 验收点有对应测试 |
| P7 | ✅ 已勾选 | P7-7/P7-8 已勾选；P7-1–6 均有对应测试 |

### 0.6 测试可信度提示

跨阶段反复出现「mock / 单测全绿，但真实端点或组合会暴露问题」：

- **P2**：reqwest 总超时、select! 守卫、未发 `include_usage`、`list_models` 未带认证（V1–V4）均被 wiremock 遮蔽。
- **P3**：89 项测试几乎全为单模块自测，零「ProviderLoop + ToolScheduler + MessageQueue + 预算 + 重试」真实组合覆盖。
- **P4**：PolicyEngine 13 处调用全在自测，零生产调用。
- **P5**：export/import 往返测试仅覆盖单分支，多分支正确性缺口未暴露。
- **P6**：Anthropic thinking budget 与 max_tokens 冲突被 mock 测试漏过（V2）。

建议：针对性地补充「跨模块端到端」与「触网 mock 语义」用例，并在「通电」后建立最小真实组合测试。


---

## 1. Phase 1（P1）— 配置系统、SQLite Actor、诊断与 CLI 骨架

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
| P1-1 配置系统 | `config-service` | 🟢 | [schema.rs](crates/config-service/src/schema.rs)、[loader.rs](crates/config-service/src/loader.rs)；层级合并、profile overrides 均有测试 |
| P1-2 SQLite Actor | `app-database` | 🟢 | 串行 Actor、WAL、备份与只读恢复连接 |
| P1-3 schema 与迁移 | `session-store`（migration） | 🟢 | 与 P1-1 同期创建于提交 `63c776d` |
| P1-4 Event Store | `session-store` | 🟢 | [event_store.rs](crates/session-store/src/event_store.rs)：append / 按 sequence 重放 |
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
| `uuid` | 基线声明（v4/v7/serde，[Cargo.toml:72](Cargo.toml)）但**全仓库零引用**；ID 均为 `string_id!` newtype，唯一性靠 DB 主键 | a) 移出基线，确立 ID 生成策略文档，未来需要时再引入；b) 立即启用 | **建议 a**。跨节点全局唯一 ID（分布式 session/event 场景）出现时，UUIDv7 仍是标准选择，届时引入不迟；当前声明属「基线虚置」 |
| `tracing-appender` | 基线声明但零引用；日志仅存内存 ring buffer，无文件/滚动输出 | a) 移出基线，待日志落盘任务立项再引入；b) 现在补文件输出 | **建议 a**，除非 P1-9 验收范围确认包含落盘 |
| `similar` | 基线记为 P7-3 使用，但 diff-service 实际解析 git 结构化输出，全仓库零引用（仅 [parser.rs](crates/diff-service/src/parser.rs) 注释中出现 similarity 字样） | a) 移出基线；b) 保留备用 | **建议 a**。未来若需进程内 diff（不经 git）再引入 |
| `notify-debouncer-full` 0.5 | 基线记 P1-8 使用；实际 file-index 自实现去抖（固定窗口，256 有界通道，[lib.rs:317](crates/file-index/src/lib.rs)、[lib.rs:384](crates/file-index/src/lib.rs)），真实使用者是 git-service 缓存失效器（[cache.rs:137-165](crates/git-service/src/cache.rs)） | a) file-index 改用 debouncer（免费获得同路径事件合并）；b) 保留自实现但修订基线归属并修复 blocking_send 风险（V7） | **倾向 a**：统一实现顺带解决 V7；若坚持自实现，需在基线中写明理由 |
| `rusqlite` 0.32 → 0.40.x | 落后上游多个 major（上游最新 0.40.1，bundled SQLite 版本与构建脚本均有改进） | 独立升级任务：API 差异评估 + 全量门禁回归 | **立项评估**，不与本阶段混改 |
| `toml` 0.8 → 1.x | 上游已发布 1.x（API 大体兼容） | 直接升级；写回需求出现时改评估 `toml_edit` | **顺手可做**，低风险 |

#### 3.3 「自实现替换包」总体判断

针对「引用面小 → 自实现换取可控性/性能/扩展性」的命题：**P1 范围内没有命中的包**。每个被引用的包使用面都覆盖了其核心价值区；真正「只用一小部分」的是三个零引用声明（uuid / tracing-appender / similar），正确动作是**移出基线**而不是自实现。相反，当前自实现的部分（metrics 采集、配置合并、redaction、去抖）中，去抖是唯一值得重新权衡的——因为它与现成 debouncer 功能重叠且带有 V7 风险。

### 4. 基线偏差清单

规则来源：ROADMAP「依赖选型基线」要求新增依赖同步回填基线表。

| 类型 | 项 | 位置 | 说明 |
| --- | --- | --- | --- |
| 声明未引用 | `uuid` | [Cargo.toml:72](Cargo.toml) | 见 §3.2 |
| 声明未引用 | `tracing-appender` | [Cargo.toml:86](Cargo.toml) | 见 §3.2 |
| 声明未引用 | `similar` | [Cargo.toml:127](Cargo.toml) | 见 §3.2 |
| 引入未登记 | `parking_lot = "0.12"` | [git-service/Cargo.toml:22](crates/git-service/Cargo.toml) | Phase 7 引入 |
| 引入未登记 | `tempfile = "3"` | [git-service/Cargo.toml:26](crates/git-service/Cargo.toml)、[diff-service/Cargo.toml:20](crates/diff-service/Cargo.toml)（dev） | 测试依赖也应登记 |
| 引入未登记 | `base64 = "0.22"`、`rand = "0.8"`、`sha2 = "0.10"`、`url = "2"` | [auth-service/Cargo.toml:14-22](crates/auth-service/Cargo.toml) | Phase 6 引入 |

**建议**：一次小型清理任务统一处理——删除 3 个零引用声明、回填 6 个未登记依赖（或说明豁免理由），并同步 ROADMAP 基线表。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V8）。

#### V1 [安全·高] `trust_workspaces` 自我提权攻击面

字段定义于 [schema.rs:43](crates/config-service/src/schema.rs)，合并逻辑会接受 workspace 层覆盖（[schema.rs:147-148](crates/config-service/src/schema.rs)），当前无消费者。一旦后续接线，恶意仓库只需在 `.pawork/config.toml` 写入 `trust_workspaces = true` 即可让工作区自授信任，绕过信任闸门。**建议**：趁未消费，将该字段限定为仅全局层可读（workspace 层覆盖直接忽略并告警），补回归测试；同时在 plan 中记录该约束。

#### V2 [安全·高] Event Store 持久化整个信封，Secret 可能落库

[event_store.rs:143-144](crates/session-store/src/event_store.rs) 将 `AgentEventEnvelope` 整体 `serde_json::to_string` 写入 `payload_json`（含 provider-specific options）。若任何事件携带 token/密钥字段，将直接违反「Secret 不落库」红线。**建议**：在序列化边界增加 redaction guard（对 options/headers 类字段白名单或掩码），并加契约测试：构造携带假 token 的事件，断言落库 JSON 中不含明文。此项建议列为 P0 后续任务。

#### V3 [健壮性·中] Windows verbatim 路径流入子进程

workspace-service 用 `fs::canonicalize` 归一化 root（[lib.rs:148](crates/workspace-service/src/lib.rs)、[lib.rs:234](crates/workspace-service/src/lib.rs)、[lib.rs:310](crates/workspace-service/src/lib.rs)），Windows 上产生 `\\?\` 前缀路径；git-service 直接把 cwd 传给子进程（[process.rs:77](crates/git-service/src/process.rs)），未做 `dunce::simplified`。部分工具对 verbatim 路径不兼容，属 P11-8 的潜在缺陷。**建议**：在 process_runtime 设置 cwd 处或 workspace-service 出口统一 `dunce::simplified`。

#### V4 [健壮性·中] artifact-store 崩溃残留 `.tmp-` 文件无人清理

`atomic_write` 先写 `.tmp-` 临时文件再 rename（[lib.rs:565-582](crates/artifact-store/src/lib.rs)）；进程崩溃会留下孤儿文件。GC 扫描刻意跳过 `.tmp-`（[lib.rs:593-609](crates/artifact-store/src/lib.rs)）但从不回收，长期磁盘泄漏。**建议**：GC 附带清理 mtime 超过阈值（如 24h）的 `.tmp-` 文件。

#### V5 [健壮性·低] 诊断包导出 TOCTOU

[bundle.rs:179-204](crates/diagnostics/src/bundle.rs)：先 `destination.exists()` 检查，再 `fs::rename` 落位；检查与 rename 之间的窗口内 rename 会覆盖已存在文件，与注释声称的「不覆盖已有文件」矛盾。**建议**：改用带序号/时间戳的命名策略，或 rename 前以 create-new 语义锁定目标。

#### V6 [安全·中] redaction 为 best-effort，残余风险未文档化

诊断包与日志的脱敏基于正则，无法穷尽形态（URL query 参数、嵌套 JSON 转义、自定义 header 等）。**建议**：在 `docs/features/` 与 `docs/quality/security-acceptance.md` 明确「诊断包视为可能含敏感数据，分享前需人工确认」，并把典型漏报形态列入测试样本持续回归。

#### V7 [性能/健壮性·中] file-index 去抖回调 `blocking_send` 有阻塞风险

notify 回调里对 256 容量通道做 `blocking_send`（[lib.rs:251](crates/file-index/src/lib.rs)、[lib.rs:317](crates/file-index/src/lib.rs)）。事件风暴（大仓库 checkout/切分支）且下游消费滞后时，watcher 线程被阻塞，OS 事件缓冲可能溢出丢事件。**建议**：`try_send` 失败时合并/丢弃并计数，或按 §3.2 统一改用 debouncer。

#### V8 [健壮性·低] file-index 错误列表无界

`errors: Arc<Mutex<Vec<String>>>` 只增不减（[lib.rs:312](crates/file-index/src/lib.rs)、[lib.rs:319](crates/file-index/src/lib.rs)），持续错误风暴下内存无界增长。**建议**：设置上限（如 1024）并环形淘汰，导出时标注截断。

### 6. 优化建议（按优先级）

#### P0（建议在下一阶段开工前处理）

1. **V2**：Event Store 序列化边界脱敏 + 契约测试（红线问题）。
2. **V1**：`trust_workspaces` 限定全局层（未消费时修改成本最低）。

#### P1（近期排期）

3. **artifact-store `put()` 性能**：当前在 DB Actor 闭包内完成 `blake3::hash` + 读盘比对 + `atomic_write`（[lib.rs:204-260](crates/artifact-store/src/lib.rs)）。大 blob 场景（ADR-018 大载荷）会阻塞**所有** DB 操作。建议：Actor 外先算 hash 与落盘（或 `spawn_blocking`），Actor 只处理元数据与引用计数。
4. **基线清理**：§4 清单一次性处理（改动面仅 Cargo.toml 与 ROADMAP 表）。
5. **去抖实现统一**：file-index 与 git-service 二选一（倾向 debouncer），顺带解决 V7。
6. **cli-host shell 阻塞**：`io::stdin().lock().lines()` 同步读取（[lib.rs:87](crates/cli-host/src/lib.rs)）阻塞 async worker 线程；改 `tokio::io::stdin` 或 `spawn_blocking`。

#### P2（顺手/评估项）

7. `rusqlite` 0.32 → 0.40.x、`toml` 0.8 → 1.x 升级评估（独立小任务，全量门禁回归）。
8. `is_ignored` 每文件重建 GitignoreBuilder（[lib.rs:487](crates/file-index/src/lib.rs)、[lib.rs:535-536](crates/file-index/src/lib.rs)）：按批/按 root 缓存 builder，大仓库初扫收益明显。
9. 初扫二进制探测每文件读 8KB（[lib.rs:53](crates/file-index/src/lib.rs)、[lib.rs:590](crates/file-index/src/lib.rs)）：可改惰性探测，或复用基线已声明的 `content-inspector`。
10. Metrics 仅有 count/sum/min/max（[metrics.rs:96-161](crates/diagnostics/src/metrics.rs)），无 p50/p95/p99，性能门禁判定力不足：补固定桶直方图或轻量分位数结构。
11. Config Loader 仅保留最后一个文件错误（`pending_error` 覆盖式，[loader.rs:57](crates/config-service/src/loader.rs)、[loader.rs:106](crates/config-service/src/loader.rs)）：多文件同时损坏时诊断信息丢失，建议聚合为错误列表。
12. `payload_json` 与独立列重复存储信封字段（[event_store.rs:205-206](crates/session-store/src/event_store.rs)）：评估瘦身（信封只存 payload，结构化字段全走列），权衡查询便利与存储开销。

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

## 2. Phase 2（P2）— Provider 运行时、OpenAI-compatible 适配、认证与模型目录

- **日期**：2026-08-08
- **评审基线**：`main` @ `67d6c4d`（工作树干净）；Phase 2 由单一提交 `a8cd17d` 交付（31 文件，+6273 行）
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置；后续再研究是否采纳）
- **范围**：ROADMAP.md Phase 2 的 11 个任务（P2-1 ~ P2-11）的完成情况、所引入包是否合适、基线偏差；漏洞与优化点一并列出。Phase 3/6 构建在 Phase 2 地基之上（如 provider-openai 直接委托 openai-compatible 引擎），受影响处标注「传播面」。

### 1. 结论摘要

1. **完成度基本可信，但「测试绿」与「真实可用」的差距比 Phase 1 大**：P2-1 ~ P2-11 全部 🟢，5 个交付面（provider-runtime / provider-openai-compatible / auth-service / model-registry / test-support）复跑共 **120 passed / 0 failed**；`clippy -D warnings`、`fmt --check`、`schema-typegen --check` 均干净。
2. **四个高风险问题全部是「mock 过得去、真实端点会翻车」型**：reqwest 总超时 60s 覆盖流式全程，长生成必被掐断（V1）；select! 守卫使预取消失效、请求照发（V2）；OpenAI 流式从不请求 `include_usage`，真实 API 下 usage 恒为 0（V3）；`list_models` 不带认证头，任何受保护的 `/models` 端点 401（V4）。
3. **包选型总体合理**：reqwest / keyring / wiremock / proptest / futures / bytes 使用面都足够大；SSE、JSONL、Partial JSON 按基线「参考 + 自实现」落地且各带 no-panic 属性测试，方向正确。**没有「引用面小、应自实现替换」的包**。
4. **主要问题在基线管理**：`futures`、`bytes` 引入未登记；`backon`、`arbitrary` 声明未引用。生产重试实际是 agent-engine 自实现的 `RetryPolicy`，provider-runtime 的 `ExponentialBackoff` 是带两个 bug 的死代码，与基线声明的 backon 三方并存、无一生效（V8）。
5. **流程偏差**：11 篇 `plan/P2-*.md` 全部停留 🟡未开始、19 个验收勾选全部未勾，提交 `a8cd17d` 未触碰任何 plan/ 与 docs/ 文件，违反 AGENTS.md §4「任何任务完成后，对应模块文档与 ROADMAP 状态须同步更新」（ROADMAP 本身已更新）。
6. **契约套件对 ADR-015 与 P2-11 自身验收均有缺口**：timeout、reconnect 用例缺失（P2-11 验收原文含 timeout，[plan/P2-11-contract-tests.md:22](plan/P2-11-contract-tests.md)）；`assert_error_kind` 对空事件流 vacuous 通过；名为 cancel-mid-stream 的测试实际是预取消。

### 2. P2 任务完成情况核对表

| 任务 | 交付 crate | 状态 | 关键证据 |
| --- | --- | --- | --- |
| P2-1 HTTP 运行时 | `provider-runtime` | 🟢（有 V1/V2） | [http.rs](crates/provider-runtime/src/http.rs)：超时/代理/自定义 header/x-trace-id/取消竞争；默认 60s 见 [http.rs:34](crates/provider-runtime/src/http.rs) |
| P2-2 SSE 解析器 | `provider-runtime` | 🟢（fuzz 口径见 §3.2） | [sse.rs](crates/provider-runtime/src/sse.rs)：data/event/id/retry、BOM、跨 chunk UTF-8；proptest 随机字节不 panic（[sse.rs:304-306](crates/provider-runtime/src/sse.rs)） |
| P2-3 JSON Lines 解析器 | `provider-runtime` | 🟢 | [jsonl.rs](crates/provider-runtime/src/jsonl.rs)：提前断开、错误事件、proptest |
| P2-4 Partial JSON 拼接 | `provider-runtime` | 🟢（见 §6-15） | [partial_json.rs](crates/provider-runtime/src/partial_json.rs)：修复语义 + proptest |
| P2-5 OpenAI-compatible 适配 | `provider-openai-compatible` | 🟢（有 V3/V4/V5） | [provider.rs](crates/provider-openai-compatible/src/provider.rs)、[request.rs](crates/provider-openai-compatible/src/request.rs)、[stream.rs](crates/provider-openai-compatible/src/stream.rs) |
| P2-6 API Key 认证 | `auth-service` | 🟢 | Keychain/Memory 双后端（[backend.rs:17](crates/auth-service/src/backend.rs)、[backend.rs:81](crates/auth-service/src/backend.rs)）；明文不入 StoredCredential 有测试（[credential.rs:269-273](crates/auth-service/src/credential.rs)） |
| P2-7 Model Registry | `model-registry` | 🟢（见 V9、§6-12） | [registry.rs](crates/model-registry/src/registry.rs)：目录/别名/能力/定价 |
| P2-8 流式组装 | `provider-runtime` | 🟢 | [stream_assembly.rs](crates/provider-runtime/src/stream_assembly.rs)：事件→领域消息 |
| P2-9 Usage 与 stop reason | `provider-runtime` | 🟢（有 V3/V5） | [usage.rs](crates/provider-runtime/src/usage.rs)：多 Provider 字段归一、整数 micro 计价 |
| P2-10 重试与错误归一化 | `provider-runtime`（+ Phase 3 `agent-engine`） | 🟢（有 V8） | [retry.rs:14](crates/provider-runtime/src/retry.rs) classify_status / parse_retry_after；生产退避在 [agent-engine/src/retry.rs:109-124](crates/agent-engine/src/retry.rs)，正确尊重 retry_after（[agent-engine/src/retry.rs:114-116](crates/agent-engine/src/retry.rs)） |
| P2-11 Provider Contract Tests | `test-support` + `provider-openai-compatible` | 🟡部分（见 §7.1） | [test-support/src/contract.rs](crates/test-support/src/contract.rs) 断言库 + [tests/contract.rs](crates/provider-openai-compatible/tests/contract.rs) 10 用例 |

**门禁证据（2026-08-08 复核）**：

- `cargo test -p provider-runtime -p provider-openai-compatible -p auth-service -p model-registry -p test-support`：**120 passed / 0 failed**（provider-runtime 54；provider-openai-compatible 12 单元 + 10 契约；auth-service 27，含 Phase 6 追加的 oauth 模块；model-registry 10；test-support 7）。
- `cargo clippy --workspace --all-targets -- -D warnings`：干净。
- `cargo fmt --all -- --check`：干净。
- `cargo run -p schema-typegen -- --check`：TypeScript declarations up to date。
- 各任务 plan 文档（`plan/P2-*.md`）状态与验收勾选**均未同步**（§4、§7.2）。

### 3. 包选型评估

#### 3.1 建议保留（自实现不值得）

| 包 | 版本（Cargo.lock） | 使用点 | 使用面评估 | 结论 |
| --- | --- | --- | --- | --- |
| `reqwest`（rustls+stream+json） | 0.12.28 | P2-1；P9-2 将复用 | 唯一 HTTP 客户端，全部 Provider 流量经此；基线已论证（[ROADMAP.md:81](ROADMAP.md)） | **保留**；用法需修（V1：改 `read_timeout`） |
| `keyring` | 3.6.3 | P2-6 | OS Keychain 唯一入口，Secret 不落库红线的承载者（[backend.rs:26-40](crates/auth-service/src/backend.rs)） | **保留** |
| `futures` | 0.3 | P2-1/P2-5 | `Stream`/`StreamExt` 是字节流消费的核心抽象（[http.rs:10](crates/provider-runtime/src/http.rs)、[provider.rs:145](crates/provider-openai-compatible/src/provider.rs)） | **保留**；需回填基线（§4） |
| `bytes` | 1 | P2-1/P2-5 | 流式字节载体 `Bytes`（[http.rs:9](crates/provider-runtime/src/http.rs)） | **保留**；需回填基线（§4） |
| `wiremock` | 0.6.5 | P2-11；Phase 6 全部 provider | 契约套件 HTTP mock 基座，6 个 crate dev 依赖 | **保留**；注意 mock 遮蔽真实行为的风险（V3/V4） |
| `proptest` | 1 | P2-2/P2-3/P2-4 | 三个解析器的 no-panic 属性测试（[sse.rs:304](crates/provider-runtime/src/sse.rs)、[jsonl.rs:126](crates/provider-runtime/src/jsonl.rs)、[partial_json.rs:526](crates/provider-runtime/src/partial_json.rs)） | **保留** |

#### 3.2 需要重新评估的项

| 项 | 现状 | 选项 | 建议 |
| --- | --- | --- | --- |
| `backon` | 基线声明用于 P2-10（[ROADMAP.md:98](ROADMAP.md)）；workspace 与 provider-runtime 均声明（[Cargo.toml:129](Cargo.toml)、[provider-runtime/Cargo.toml:21](crates/provider-runtime/Cargo.toml)）但**全仓库零引用**（唯一命中是注释 [agent-engine/src/retry.rs:13](crates/agent-engine/src/retry.rs)）。生产重试 = agent-engine 自实现 `RetryPolicy`；provider-runtime 的 `ExponentialBackoff` 是死代码（V8） | a) 移出基线并删依赖，承认「退避自实现」；b) 用 backon 替换自实现退避 | **倾向 a**：agent-engine 的退避已满足需求且尊重 Retry-After，继续引入 backon 收益有限；同时删除 `ExponentialBackoff` 死代码。若未来需要更复杂策略（按错误类别差异化退避）再评估 b |
| `cargo-fuzz` + `arbitrary` | 基线测试工具行（[ROADMAP.md:100](ROADMAP.md)）；`arbitrary` 已声明（[Cargo.toml:135](Cargo.toml)）但无 crate 引用，仓库无 `fuzz/` 目录。P2-2/P2-3 验收「fuzz 不 panic」（[plan/P2-2-sse-parser.md:23](plan/P2-2-sse-parser.md)）实际由 proptest 承担 | a) 建 `fuzz/` 目标（SSE/JSONL/partial-json 三个解析器是理想靶子）；b) 修订基线与 plan，明确「属性测试代替 cargo-fuzz」 | **建议 a**：解析器是外部输入第一入口，libFuzzer 级覆盖值得；至少应先把 `arbitrary` 的声明处置掉 |
| `reqwest` 超时语义 | 0.12.28 已提供 `read_timeout`（按读操作重置），当前只用了总 `timeout`（V1） | 无需换包，改用 API | 见 V1 |

#### 3.3 「自实现替换包」总体判断

针对「引用面小 → 自实现换取可控性」的命题：**P2 范围内没有命中的包**。真正需要收敛的是反向问题——**自实现与已声明的包并存且自实现是死代码**：退避策略一处有 backon（声明未用）、一处有 agent-engine `RetryPolicy`（生产中用）、一处有 provider-runtime `ExponentialBackoff`（死代码且带 bug）。三方并存是最差状态，按 §3.2 收敛为一处。基线「参考 + 自实现」的三个解析器（SSE/JSONL/Partial JSON）实现质量良好，不需要回退为引包。

### 4. 基线偏差清单

规则来源：ROADMAP「依赖选型基线」要求新增依赖同步回填基线表（[ROADMAP.md:14](ROADMAP.md)、[ROADMAP.md:58](ROADMAP.md)）。

| 类型 | 项 | 位置 | 说明 |
| --- | --- | --- | --- |
| 引入未登记 | `futures = "0.3"` | [Cargo.toml:68](Cargo.toml) | `a8cd17d` 引入；ROADMAP 基线表无此行。Cargo.toml 注释自称「依赖选型基线（ROADMAP『依赖选型基线·直接采用』）」，镜像关系已失真 |
| 引入未登记 | `bytes = "1"` | [Cargo.toml:69](Cargo.toml) | 同上 |
| 声明未引用 | `backon = "1"` | [Cargo.toml:129](Cargo.toml)、[provider-runtime/Cargo.toml:21](crates/provider-runtime/Cargo.toml) | 见 §3.2；零代码引用 |
| 声明未引用 | `arbitrary = "1"` | [Cargo.toml:135](Cargo.toml) | 无 `fuzz/` 目录、无 crate 引用；见 §3.2 |
| 流程偏差 | `plan/P2-*.md` 全部未同步 | 11 篇均 `🟡未开始`，19 个验收框未勾 | 提交 `a8cd17d` 只改 Cargo/ROADMAP/源码，未触碰 plan/ 与 docs/，违反 AGENTS.md §4。ROADMAP 状态列本身已更新为 🟢，属「半同步」 |

**建议**：一次小型清理任务统一处理——回填 futures/bytes 两行、删除 backon/arbitrary 两处声明（或说明豁免理由）、同步 11 篇 plan 文档。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V10）。

#### V1 [正确性·高] reqwest 总超时 60s 覆盖流式全程，长生成必被掐断

[http.rs:34](crates/provider-runtime/src/http.rs) 默认 `timeout = 60s`，经 [http.rs:101-102](crates/provider-runtime/src/http.rs) 设为 reqwest 的 `timeout()`。reqwest 文档明确该超时是「from when the request starts connecting until the response body has finished…a total deadline」——对流式响应，**整个 body 读取都计入**。任何总时长超过 60s 的 LLM 生成（长 reasoning、慢本地模型）都会在中途被超时打断并归一为错误。reqwest 0.12.28 已提供 `read_timeout`（按每次读操作重置，专为「未知大小的长流」设计）。**传播面**：provider-openai-compatible 与 Phase 6 的 provider-openai/anthropic/google 全部经此客户端（provider-openai 的 `request_timeout` 覆盖的也是同一总超时语义，[provider-openai/src/provider.rs:72-76](crates/provider-openai/src/provider.rs)）。**建议**：流式路径改用 `read_timeout`（如 60s 无新字节才判定停滞）并取消/大幅放宽总超时；补「慢速长流不被掐断」的契约用例（与 §7.1 的 timeout 用例合并）。

#### V2 [正确性·高] select! 取消分支守卫使预取消失效，请求照发

[http.rs:173-175](crates/provider-runtime/src/http.rs) 与 [http.rs:216-218](crates/provider-runtime/src/http.rs) 的取消分支写作 `_ = cancel.cancelled(), if !cancel.is_cancelled() => …`。agent-domain 的 `CancellationFuture` 在 token 已取消时首次 poll 即 Ready（[cancel.rs:65-68](crates/agent-domain/src/cancel.rs)），**不加守卫时预取消本可被正确处理**；加了守卫后，预取消的 token 反而使取消分支丧失候选资格，select 只等 `send_fut`——请求照样发出、照样等待响应。契约测试 `contract_cancel_mid_stream`（[tests/contract.rs:185-199](crates/provider-openai-compatible/tests/contract.rs)）恰好是预取消：`cancel.cancel()` 在 `stream()` 之前调用，请求实际仍打到 mock，Cancelled 错误来自 [provider.rs:145-148](crates/provider-openai-compatible/src/provider.rs) 循环内的 `is_cancelled` 检查——测试通过但验证的完全不是 select 路径。**建议**：删除两处守卫；把该测试改为真正的 mid-stream 取消（读到一个 delta 后取消），并保留一个预取消用例断言「请求不应发出」（可用 wiremock 的命中计数验证）。

#### V3 [正确性·高] OpenAI 流式 usage 永远拿不到：未发送 `stream_options.include_usage`

[request.rs](crates/provider-openai-compatible/src/request.rs) 构造请求体时没有任何 `stream_options` / `include_usage` 字段（全仓库亦无命中）。OpenAI 及兼容 API 的流式模式**默认不返回 usage**，必须显式请求 `stream_options: {"include_usage": true}`。契约测试 `contract_usage_and_stop_reason`（[tests/contract.rs:164-181](crates/provider-openai-compatible/tests/contract.rs)）由 mock 主动推送 usage chunk，把该缺口完全遮蔽。后果：OpenAI 系（含 Phase 6 provider-openai 委托路径）真实流量下 usage 恒为 0，P2-9 的归一化没有输入源，下游费用估算与 Phase 14 额度监控全部失真。**建议**：请求体固定附加 `stream_options.include_usage = true`，正确处理尾部 usage-only chunk（`choices` 为空）；契约测试改为断言请求体包含该字段。

#### V4 [正确性·中] `list_models` 不携带认证头

[provider.rs:210-216](crates/provider-openai-compatible/src/provider.rs)：参数名为 `_credential`（未使用），`get_json` 不带任何 Authorization。OpenAI 官方与绝大多数云端兼容端点的 `/v1/models` 要求认证，此路径必然 401。契约测试 `contract_list_models`（[tests/contract.rs:320-337](crates/provider-openai-compatible/tests/contract.rs)）用无认证 mock + `None` 凭据，无法暴露。旁证：Phase 6 的 provider-openai 选择完全不调远端 `/models`、直接返回内置目录（[provider-openai/src/provider.rs:97-102](crates/provider-openai/src/provider.rs)）来绕开它。**建议**：复用 `auth_header()`（[provider.rs:87-93](crates/provider-openai-compatible/src/provider.rs)）给 `list_models`；契约测试增加「请求头含 Authorization」断言。

#### V5 [正确性·中] `[DONE]` 无 finish_reason 时 stop_reason 被记为 Error

[provider.rs:137](crates/provider-openai-compatible/src/provider.rs) 将 summary 初始 `stop_reason` 置为 `StopReason::Error`；`[DONE]` 分支（[provider.rs:152-155](crates/provider-openai-compatible/src/provider.rs)）只置 `saw_completion`、不更新 stop_reason。部分本地服务（Ollama/vLLM 的某些版本）最后一个 chunk `finish_reason` 为 null 或缺失、直接以 `[DONE]` 收尾——此时流成功完成，但 summary 记为 Error，误导 P3-7 重试判定与 GUI 展示。同源问题：`map_stop_reason(None, false) → StopReason::Error`（[usage.rs:51](crates/provider-runtime/src/usage.rs)）。**建议**：流正常走到 `[DONE]` 而从未见到 finish_reason 时归一为 `Completed`（或 `Other("done")`）；`map_stop_reason(None)` 的语义在 docs/features/providers.md 中写明。

#### V6 [安全·中] `provider_options` 透传无键保护，可覆盖 canonical 关键字段

[request.rs:89-93](crates/provider-openai-compatible/src/request.rs)：provider_options 以「覆盖」语义合并进请求体顶层，无任何保留键限制——调用方可覆盖 `model`、`messages`、`stream`、`tools`。P2 阶段入口只有测试，但 Phase 6（P6-9）已把该透传作为正式能力，后续 GUI/配置一旦直通，即可绕过 canonical 层约束（例如把 `stream` 改为 false 破坏整个流式管线）。**建议**：定义保留键集合（model/messages/stream/tools 及认证相关字段），透传命中保留键时忽略并告警；或在 provider_options 入口做 schema 白名单，并在 [docs/features/providers.md](docs/features/providers.md) 记录边界。

#### V7 [健壮性·中] 解析器缓冲无上限，且非法字节逐个移除是 O(n²)

SSE 与 JSONL 解析器的内部 `buf` 只增不减、无容量上限（[sse.rs:61](crates/provider-runtime/src/sse.rs)、[jsonl.rs:20](crates/provider-runtime/src/jsonl.rs)）：一条永不出现行终止符的流（恶意或故障端点）会让内存无限增长。非法 UTF-8 的处理用 `Vec::remove` 逐字节移除（[sse.rs:137](crates/provider-runtime/src/sse.rs)、[jsonl.rs:84](crates/provider-runtime/src/jsonl.rs)），每次 remove 是 O(n) memmove，持续坏字节流下退化为 O(n²)。P2-2/P2-3 验收只要求「不 panic」，性能健壮性缺了一半。**建议**：buf 设上限（如 1 MiB，超限发解析错误事件并重置）；非法字节改用游标分段 `drain` 批量移除。

#### V8 [质量·中] `ExponentialBackoff` 死代码且带两个 bug，与 P2-10 验收字面矛盾

[retry.rs:159-208](crates/provider-runtime/src/retry.rs) 的 `ExponentialBackoff` 仅被自身测试引用（[retry.rs:254](crates/provider-runtime/src/retry.rs)、[retry.rs:266](crates/provider-runtime/src/retry.rs)），生产重试走 agent-engine 的 `RetryPolicy`（正确尊重 Retry-After，[agent-engine/src/retry.rs:114-116](crates/agent-engine/src/retry.rs)）。死代码本身有两个 bug：① [retry.rs:206](crates/provider-runtime/src/retry.rs) 结尾 `Some(delay.min(self.cap))` 把 Retry-After 也钳进 cap，直接违反其文档注释（[retry.rs:157](crates/provider-runtime/src/retry.rs)「遵守 Retry-After」）与 P2-10 验收项「退避遵守 Retry-After」（[plan/P2-10-retry-error.md:23](plan/P2-10-retry-error.md)）；② jitter 用固定种子 LCG（[retry.rs:173](crates/provider-runtime/src/retry.rs)），所有实例共享同一序列（削弱雷群缓解），且 `(rng_state >> 33) / u32::MAX`（[retry.rs:190](crates/provider-runtime/src/retry.rs)）的采样值域只有 [0, ≈0.5]，抖动区间减半。**建议**：随 §3.2 一并处置——删除该结构（首选）或修复后接线生产。

#### V9 [正确性·低] `resolve()` 精确匹配与「不区分大小写」注释矛盾

[registry.rs:101-109](crates/model-registry/src/registry.rs)：两次 HashMap 精确查找，无任何大小写归一；注释（[registry.rs:107](crates/model-registry/src/registry.rs)）却声称「不区分大小写」。`resolve("GPT-4o")` 返回 None，用户以不同大小写书写模型 id 时静默落入「目录外模型」路径（能力/定价/上下文校验全部失效）。**建议**：入口 `to_ascii_lowercase` 归一（别名表构建时同样归一），或修正注释并在校验层给出明确错误。

#### V10 [质量·低] 契约测试遗留调试输出

[tests/contract.rs:205](crates/provider-openai-compatible/tests/contract.rs)：`println!("XXXURI_START{}XXXURI_END", uri);` 调试残留，随 `cargo test` 输出。**建议**：删除（顺手项）。

### 6. 优化建议（按优先级）

#### P0（建议在 Provider 面向真实用户接线前处理）

1. **V3**：请求体附加 `stream_options.include_usage` 并处理 usage-only 尾块——usage 是费用与 Phase 14 额度的数据源，当前恒为 0。
2. **V1**：流式路径改 `read_timeout`；与 timeout 契约用例（§7.1）一起落地。
3. **V2 + V4**：删 select 守卫、`list_models` 加认证头；两处都是小改动，可一提交完成，并各自补针对性断言。

#### P1（近期排期）

4. **V5**：`[DONE]` 无 finish_reason 归一为 Completed；同步修订 `map_stop_reason(None)` 语义说明。
5. **退避收敛**：删 `ExponentialBackoff`（V8）+ 移除 backon 声明（§3.2）+ 回填 futures/bytes（§4），一次基线清理提交。
6. **V7**：解析器有界缓冲 + 批量移除非法字节。
7. **契约套件补齐**：新增 timeout、reconnect 用例（P2-11 验收原文要求，[plan/P2-11-contract-tests.md:22](plan/P2-11-contract-tests.md)）；修复 `assert_error_kind` 的 vacuous 通过——当前 `found || events.is_empty()`（[test-support/src/contract.rs:93-96](crates/test-support/src/contract.rs)）让空事件流永远通过，应改为同时接收 `stream()` 的返回错误并强制断言其一。
8. **文档同步**：11 篇 `plan/P2-*.md` 状态与验收勾选回填（AGENTS.md §4）；删 [tests/contract.rs:205](crates/provider-openai-compatible/tests/contract.rs) 调试输出（V10）。
9. **V6**：provider_options 保留键保护（赶在更多入口接入前）。

#### P2（顺手/评估项）

10. [request.rs:49-50](crates/provider-openai-compatible/src/request.rs) 发送 `max_tokens`：OpenAI o 系列只接受 `max_completion_tokens`，建议按模型族切换或双发兼容。
11. [provider.rs:236-237](crates/provider-openai-compatible/src/provider.rs) 对发现的模型硬编码 128k/16k 能力：改为「未知 → 留空/可配置覆盖」，避免 `validate_context` 与真实窗口错位。
12. model-registry 内置目录陈旧（gpt-4o / gpt-4o-mini / claude-3-5-sonnet / gemini-1.5-pro / gpt-3.5-turbo，定价为硬编码近似值，[registry.rs:183-261](crates/model-registry/src/registry.rs)）：标注数据日期、建立目录更新任务；另注意 Phase 6 provider-openai 自带一份内置目录（[provider-openai/src/provider.rs:114](crates/provider-openai/src/provider.rs) 起），双目录并存有漂移风险。
13. 计价逻辑双份：[pricing.rs:83-105](crates/model-registry/src/pricing.rs) 与 [usage.rs:76-97](crates/provider-runtime/src/usage.rs)（`ModelPricingRef` 注释自称「与 model-registry 的 ModelPricing 字段对齐，避免循环依赖」）。字段对齐靠注释维持，建议把计价纯函数收敛到单一 crate 导出，另一侧复用。
14. `response_id` 用 trace_id 顶替（[provider.rs:130-131](crates/provider-openai-compatible/src/provider.rs)、[provider.rs:199](crates/provider-openai-compatible/src/provider.rs)）：应取响应体 `id`，trace 关联与 provider 侧 id 目前混为一个值。
15. partial_json 两处毛刺：`parse_repaired` 对已完整 JSON 双重解析（[partial_json.rs:46-51](crates/provider-runtime/src/partial_json.rs) 先 `from_str`，`repair_json` 内 [partial_json.rs:372](crates/provider-runtime/src/partial_json.rs) 再查一遍）；`scan_number`（[partial_json.rs:382-404](crates/provider-runtime/src/partial_json.rs)）把 `1.2.3` 这类畸形数字按「EOF 截断」原样保留，最终 `parse_repaired` 返回 None，组装侧回退 `Value::Null` 丢失整个 arguments（[stream_assembly.rs:183-185](crates/provider-runtime/src/stream_assembly.rs)）。实际流中罕见，记录备查。
16. auth-service 的 `MemoryBackend`（[backend.rs:81](crates/auth-service/src/backend.rs)）中 secret 不做 zeroize：当前是测试/回退后端，若升级为生产可用，需评估 `Zeroizing` 包装；在 [docs/features/auth.md](docs/features/auth.md) 记录该残余风险。
17. SSE `finish()` 在流尾残留非法 UTF-8 时静默丢弃（[sse.rs:92-104](crates/provider-runtime/src/sse.rs) 注释「尽力而为」）：可接受，但建议在诊断日志中计数，便于排查「最后一个事件消失」类问题。

### 7. 附录

#### 7.1 ADR-015 契约用例覆盖对照

[ADR-015:12](docs/adr/ADR-015-provider-contract-tests.md) 要求 14 类用例：text、tool call、multiple tool calls、image、thinking、usage、stop reason、cancel、timeout、rate limit、malformed stream、partial JSON、reconnect、context overflow。

| 用例 | provider-openai-compatible（P2-11） | 说明 |
| --- | --- | --- |
| text / tool call / multiple tool calls | ✅ 3 用例 | [tests/contract.rs:95-163](crates/provider-openai-compatible/tests/contract.rs) |
| usage / stop reason | ✅（但被 mock 遮蔽，见 V3） | [tests/contract.rs:164](crates/provider-openai-compatible/tests/contract.rs) |
| cancel | ⚠️ 名为 mid-stream 实为预取消（V2） | [tests/contract.rs:185](crates/provider-openai-compatible/tests/contract.rs) |
| rate limit / context overflow / malformed / partial JSON | ✅ 4 用例 | [tests/contract.rs:202-318](crates/provider-openai-compatible/tests/contract.rs) |
| **timeout** | ❌ 缺失 | P2-11 验收原文包含（[plan/P2-11-contract-tests.md:22](plan/P2-11-contract-tests.md)）；Phase 6 三个 provider 套件同样没有 |
| **reconnect** | ❌ 缺失 | ADR-015 与 P2-11 步骤 1 均列出；全仓库无对应用例 |
| image / thinking | ➖ 不在 P2 范围 | 已由 Phase 6 各自套件覆盖（provider-openai/anthropic/google 的 tests/contract.rs） |
| list_models（超出 ADR 清单） | ✅ | [tests/contract.rs:320](crates/provider-openai-compatible/tests/contract.rs) |

另：`tests/contract.rs` 自 `a8cd17d` 后无任何改动（git log 确认），Phase 6 未回补 P2 套件。

#### 7.2 plan 文档漂移清单

| 文件 | 状态字段 | 未勾验收框 |
| --- | --- | --- |
| plan/P2-1-http-runtime.md ~ plan/P2-11-contract-tests.md（共 11 篇） | 全部 `🟡未开始`（应为 🟢） | 合计 19 个 `- [ ]`，如 [plan/P2-10-retry-error.md:22-23](plan/P2-10-retry-error.md)、[plan/P2-11-contract-tests.md:22](plan/P2-11-contract-tests.md) |

对照 REVIEW.md §2 的做法（Phase 1 的 plan 文档均已勾选），Phase 2 是唯一「ROADMAP 已 🟢、plan 全未动」的阶段；提交 `a8cd17d` 的文件清单（31 个文件）确认未触碰 plan/ 与 docs/。

### 8. 建议的后续动作（本次未执行，供研究）

1. 对 V1~V4 立项（真实端点可用性红线；V3 是 Phase 14 额度监控的前置）。
2. 基线清理小任务（§4 + §3.2）：回填 futures/bytes、移除 backon/arbitrary、删 ExponentialBackoff，一次提交。
3. 契约套件补齐：timeout/reconnect 用例 + `assert_error_kind` 语义修复 + cancel 用例改造（§6-7）。
4. plan/docs 同步任务：11 篇 plan 文档回填状态与勾选，providers.md 补 `include_usage` / stop reason 语义说明。
5. 目录与计价治理（§6-12/13）：模型目录更新机制、计价单一来源，可与 Phase 14 一并评估。

---

*评审方法：以 `67d6c4d` 为基线，逐项核对 ROADMAP/plan 状态、源码与依赖清单，并复跑 Phase 2 相关 5 个 crate 的测试与静态门禁（test/clippy/fmt/schema-typegen）；对 reqwest 超时语义、CancellationFuture 行为等关键断言直接核对了依赖源码与 agent-domain 实现；文中所有结论均给出文件与行号级证据。本文档仅为评审记录，不代表已批准的变更。*


---

## 3. Phase 3（P3）— Agent Loop 主干

- **日期**：2026-08-08
- **评审基线**：`main` @ `67d6c4d`（工作树除 `REVIEW-P2.md` 未跟踪外干净）
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置；后续再研究是否采纳）
- **范围**：ROADMAP.md Phase 3「Agent Loop」的 10 个任务（P3-1 ~ P3-10）的完成情况、所引入包是否合适、基线偏差；漏洞与优化点一并列出。Phase 3 是「关键路径」中 Agent Loop 主干，Phase 4（工具/权限）、Phase 5（Session/压缩）、Phase 7（Git）均建在其上，受影响处在文中标注「传播面」。

### 1. 结论摘要

1. **测试全绿，但「绿」的含金量低于 Phase 1/2**：4 个交付 crate（`agent-engine` / `context-engine` / `tool-runtime` / `agent-events`）复跑共 **89 passed / 0 failed**（agent-engine 50 单元 + 1 集成；context-engine 26；tool-runtime 9；agent-events 3）；`clippy -D warnings`、`fmt --check`、`schema-typegen --check` 均干净。但 89 项测试几乎全是**单模块自测**，没有任何一项覆盖「ProviderLoop + ToolScheduler + MessageQueue + 预算 + 重试」的真实组合。
2. **核心问题：组件齐全，主干未接线**。P3-1~P3-10 各自实现良好，但作为「主干」的 `ProviderLoop` 只真正组合了 3 个兄弟模块：状态机（P3-1）、部分预算（P3-6）、部分事件广播（P3-9）。`MessageQueue`（P3-5）、`RetryController`（P3-7）、`CancelHandle`+进程树清理（P3-8）在 `provider_loop.rs` 中**零引用**——它们被 `pub use` 导出却从未进入循环。模块头注释自称「组合状态机、预算控制、消息队列、事件广播」（[provider_loop.rs:4-5](crates/agent-engine/src/provider_loop.rs)），与实现不符。
3. **四项「mock 过得去、真实运行会暴露」的高危缺口**：①取消/预算耗尽两条终止路径只转状态机、不发 `RunCancelled`/`RunFailed` 事件（V1）；②Provider 流式增量被 `LoopSink` 全量缓冲、从不广播，GUI/CLI 拿不到实时 token 流（V2）；③`provider.stream()` 调用没有任何重试包裹，P3-7 重试逻辑在 loop 层完全不生效（V3）；④崩溃恢复重放对「工具轮次」的状态机重建失真，产生虚假 `IllegalTransition`（V4）。
4. **包选型合理，tiktoken-rs 落地正确**：`tiktoken-rs` 0.6.0（P3-2）按基线「仅 OpenAI 系精确、其它启发式」正确实现；`tokio` 的 `broadcast`/`Semaphore`/`Mutex`/`Notify` 是标准原语。**没有「引用面小、应自实现替换」的包**，也没有「自实现却应引包」的反例（状态机/重试/预算/队列按基线属「完全自实现」）。Phase 3 crate 未引入任何新依赖。
5. **基线偏差小但流程偏差与 Phase 2 同病**：无新增「引入未登记」依赖；唯一的「声明未引用」是 `futures`（P2 遗留，agent-engine 成为第三个消费者，登记必要性更强）。10 篇 `plan/P3-*.md` **全部停留 `🟡未开始`、验收框全未勾**，违反 AGENTS.md §4（与 REVIEW-P2 §4 同一问题，提交未触碰 plan/）。
6. **P3-6 多维预算名不副实**：9 个预算维度中，Cost / Duration / Concurrency / ArtifactBytes 四个维度在 loop 中**零记录**（`record_cost`/`set_elapsed`/`set_concurrency`/`record_artifact` 从不调用），相关硬上限永远不可能触发；`soft_warnings` 被计算但从不翻译为事件，P3-6 验收「达预算产生事件、不静默停」对软阈值未满足。

### 2. P3 任务完成情况核对表

| 任务 | 交付 crate/模块 | 状态 | 关键证据 |
| --- | --- | --- | --- |
| P3-1 Run 状态机 | `agent-engine/state.rs` | 🟢 | 12 态全转换 + 非法转换防御 + 事件 hint 映射（[state.rs:155-200](crates/agent-engine/src/state.rs)）；8 项测试 |
| P3-2 上下文构建与预算 | `context-engine` | 🟢 | 14 来源确定性排序、tiktoken 精确 + 启发式回退、output/thinking reserve、超限触发 `CompactionTrigger`（[builder.rs](crates/context-engine/src/builder.rs)、[token.rs](crates/context-engine/src/token.rs)）；26 项测试 |
| P3-3 Provider Loop | `agent-engine/provider_loop.rs` | 🟢（有 V1/V2/V3） | 多轮工具循环、混合审批保序、状态机驱动（[provider_loop.rs](crates/agent-engine/src/provider_loop.rs)）；但重试/消息队列/进程清理未接入（见 §6） |
| P3-4 Tool Scheduler | `tool-runtime/scheduler.rs` | 🟢（有 V8/V9/V10） | 只读并发/写串行/同文件串行/Git index 串行/审批暂停/取消传播（[scheduler.rs](crates/tool-runtime/src/scheduler.rs)）；9 项测试；但上下文注入假值（V8）且未与 ProviderLoop 桥接（V9） |
| P3-5 消息队列 | `agent-engine/queue.rs` | 🟢（未接线，见 V7） | enqueue/replace_queued/drain、快照恢复、并发不丢（[queue.rs](crates/agent-engine/src/queue.rs)）；5 项测试；**ProviderLoop 未使用** |
| P3-6 预算控制 | `agent-engine/budget.rs` | 🟡部分（见 V5/V6） | 9 维预算 + 软/硬阈值；但 loop 仅记录 5 维、soft_warnings 不发事件（[budget.rs](crates/agent-engine/src/budget.rs)） |
| P3-7 重试 | `agent-engine/retry.rs` | 🟡部分（见 V3） | `RetryPolicy`/`RetryController` 实现 + 尊重 Retry-After；6 项测试；**ProviderLoop 未调用** |
| P3-8 取消 | `agent-engine/cancel.rs` | 🟡部分（见 V7） | `CancelHandle` + `ProcessTreeCleaner` trait + 原子门控；3 项测试；**ProviderLoop 用裸 CancellationToken，进程清理不触发** |
| P3-9 事件流式分发 | `agent-engine/broadcast.rs` | 🟢（有 V2） | `tokio::broadcast` 有界多订阅 + Lagged 背压 + <2ms 延迟基准（[broadcast.rs](crates/agent-engine/src/broadcast.rs)）；但流式增量不进入广播 |
| P3-10 Interrupted Run 恢复 | `agent-engine/recovery.rs` | 🟡部分（见 V4） | `scan_interrupted`/`replay_run`/`group_by_run`，<1s 重放基准（[recovery.rs](crates/agent-engine/src/recovery.rs)）；但工具轮次状态重建失真 |

**门禁证据（2026-08-08 复核）**：

- `cargo test -p agent-engine -p context-engine -p tool-runtime -p agent-events`：**89 passed / 0 failed**。
- `cargo clippy -p agent-engine -p context-engine -p tool-runtime -p agent-events --all-targets -- -D warnings`：干净。
- `cargo fmt --all -- --check`：干净（`FMT_EXIT=0`）。
- `cargo run -p schema-typegen -- --check`：TypeScript declarations up to date。
- 各任务 plan 文档（`plan/P3-*.md`）状态与验收勾选**均未同步**（§5、§8.2）。

### 3. 包选型评估

#### 3.1 建议保留（自实现不值得）

| 包 | 版本（Cargo.lock） | 使用点 | 使用面评估 | 结论 |
| --- | --- | --- | --- | --- |
| `tiktoken-rs` | 0.6.0 | P3-2（`token.rs`） | OpenAI 系精确 BPE 计数（`get_bpe_from_model`/`encode_ordinary`/`CoreBPE`），非 OpenAI 自动回退启发式（[token.rs:170-176](crates/context-engine/src/token.rs)、[token.rs:208-218](crates/context-engine/src/token.rs)），与基线约定完全一致 | **保留**；会拉入 `bstr`/`fancy-regex`/`ndarray` 等较重依赖，但 BPE 分词本就无法轻量自实现 |
| `tokio`（`broadcast`） | 1 | P3-9 | 多订阅者有界广播 + Lagged 背压，是「慢消费者不拖垮核心」的标准答案（[broadcast.rs:62-70](crates/agent-engine/src/broadcast.rs)） | **保留** |
| `tokio`（`Semaphore`） | 1 | P3-4 | 全局并发上限，`OwnedSemaphorePermit` drop 即释放 | **保留** |
| `tokio`（`Mutex`/`Notify`） | 1 | P3-5 | 消息队列的异步互斥与唤醒；`Notify` 的 permit 语义正确覆盖「释放锁后再 await」的唤醒竞态 | **保留** |
| `async-trait` / `serde` / `serde_json` / `thiserror` / `tracing` | 基线版本 | 全局 | 基础设施，无争议 | **保留** |

#### 3.2 需要重新评估的项

| 项 | 现状 | 建议 |
| --- | --- | --- |
| `futures` | workspace 声明（[Cargo.toml:68](Cargo.toml)），agent-engine 成为继 provider-runtime、provider-openai-compatible 之后**第三个**消费者（[agent-engine/Cargo.toml:14](crates/agent-engine/Cargo.toml)），但 ROADMAP「直接采用」基线表仍无此行（REVIEW-P2 §4 已记录） | **回填基线**（P2 遗留，agent-engine 强化其必要性）。零代码级问题，纯文档同步 |

#### 3.3 「自实现替换包」总体判断

针对「引用面小 → 自实现换取可控性」的命题：**P3 范围内没有命中的包**。真正需要关注的不是选型，而是**自实现的模块是否被正确接线**——状态机、重试、预算、消息队列、广播都是按基线「完全自实现（P3-*）」正确落地的，但其中重试/消息队列/取消句柄三块自实现产物在主干循环里是「建成但未通电」状态（见 §6 V3/V7）。这与 Phase 2 的「backon 声明未用 + ExponentialBackoff 死代码」是同一类问题在 Phase 3 的放大：组件质量高，集成度低。

### 4. 基线偏差清单

规则来源：ROADMAP「依赖选型基线」要求新增依赖同步回填基线表（[ROADMAP.md:14](ROADMAP.md)、[ROADMAP.md:58](ROADMAP.md)）。

| 类型 | 项 | 位置 | 说明 |
| --- | --- | --- | --- |
| 声明未引用（P2 遗留，强化） | `futures = "0.3"` | [Cargo.toml:68](Cargo.toml) | agent-engine 新增引用，消费者增至 3 个；ROADMAP 基线表仍缺此行 |
| 新增引入未登记 | — | — | Phase 3 四个 crate 的所有依赖均映射到既有 workspace 条目，**无新增偏差** |
| 流程偏差 | `plan/P3-*.md` 全部未同步 | 10 篇均 `🟡未开始`，验收框全未勾 | 与 REVIEW-P2 §4 同一问题；ROADMAP 状态列已 🟢，属「半同步」 |

**建议**：与 REVIEW-P2 §4 的基线清理任务合并执行——回填 `futures`、同步 10 篇 P3 plan 文档。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V11）。

#### V1 [正确性·高] 取消/预算耗尽两条终止路径不发终态事件

[provider_loop.rs:186-217](crates/agent-engine/src/provider_loop.rs) 的 `run()` 有四条终止路径，但只有「通用错误」与「成功」两条发终态事件：

- 取消（预检，L187-190）：`transition(Cancel)` 后直接 `return Err(Cancelled)`，**无 `RunCancelled` 事件**。
- 预算耗尽（L192-196）：`transition(Fail)` 后直接 `return Err(BudgetExceeded)`，**无 `RunFailed` 事件**。
- 流中取消（L201-204、L205-210）：同样 `transition(Cancel)` 后 `return`，**无 `RunCancelled` 事件**。
- 通用错误（L211-217）：正确调用 `emit_terminal_payload(RunFailed)`。

后果：被取消或因预算停止的 Run，其持久化事件流**以非终态事件结尾**，违反「每次转换都有事件」契约（P3-1 验收）与 ADR-016「状态可由事件序列重建」——重建出的状态会停留在 `ExecutingTools`/`StreamingResponse` 等活跃态而非 `Cancelled`/`Failed`。**传播面**：Phase 5 session 投影、Phase 13 GUI 的 Run 状态展示均依赖终态事件。测试 `cancelled_run_emits_cancelled_and_returns_error`（[provider_loop.rs](crates/agent-engine/src/provider_loop.rs)）名字声称「emits_cancelled」却只断言 `state==Cancelled`、不断言事件被广播，与该缺口正好叠加（V11）。**建议**：四条终止路径统一经 `emit_terminal_payload` 补发对应事件，并改测试断言订阅者收到 `RunCancelled`。

#### V2 [正确性·高] Provider 流式增量被全量缓冲、从不广播

`LoopSink`（[provider_loop.rs:575-595](crates/agent-engine/src/provider_loop.rs)）实现 `ProviderEventSink::emit` 时只把每个 `ProviderStreamEvent` push 进 `Mutex<Vec>`，整轮流结束后由 `AssembledTurn::apply` 一次性消费成一条助手消息，再以单条 `MessageCommitted` 广播。`AgentEvent` 枚举里定义了 `AssistantTextDelta`/`AssistantThinkingDelta`/`ToolCallArgumentsDelta`/`ToolOutputDelta`/`ToolCallStarted`（[agent-events/lib.rs](crates/agent-events/src/lib.rs)），**但 ProviderLoop 从不 emit 这些变体**——它们对订阅者是不可见的。后果：GUI/CLI 无法做「逐 token 流式显示」，P3-9「事件流式分发」实际只分发生命周期事件（RunStarted/MessageCommitted/...），<2ms 延迟基准（[broadcast.rs](crates/agent-engine/src/broadcast.rs)）测的也不是 token 流。**建议**：让 `LoopSink` 在缓冲的同时把 delta 事件 fan-out 到 `EventBroadcaster`（或引入双写 sink），`AgentEvent` 的 delta 变体即为此设计，缺的只是接线。

#### V3 [正确性·高] 重试逻辑完全未接入 Provider Loop

`retry.rs` 实现了完整的 `RetryPolicy`/`RetryController`/`RetryDecision`（含尊重 `Retry-After`、指数退避、6 项测试），但 `provider.stream(...)` 调用（[provider_loop.rs:259](crates/agent-engine/src/provider_loop.rs)）**没有任何重试包裹**——全仓库 `RetryController`/`RetryPolicy` 仅在 `retry.rs` 自身与 `lib.rs` 的 `pub use` 出现，`provider_loop.rs` 零引用。P3-7 验收「断流可重试、上下文不丢」在 loop 层完全不成立：一次 `StreamInterrupted`/`Network`/`Timeout` 错误直接走 L211-217 的通用错误路径，Run 标记 `Failed` 终止，无重试。**建议**：在 `run_turn` 的 `provider.stream()` 外层包 `RetryController`，断流时保持 `messages` 不变重发（断流重试语义），并把每次 `RetryAttempt` 翻译为 `AgentEvent::Diagnostic` 以满足「重试与事件一致性」。

#### V4 [正确性·高] 崩溃恢复重放对「工具轮次」状态机重建失真

[recovery.rs:60-90](crates/agent-engine/src/recovery.rs) 的 `replay_run` 用事件流重建状态机，但 `StreamFinished` 转换的 `EventHint` 为 `None`（[state.rs](crates/agent-engine/src/state.rs) `event_hint`），循环**不为它持久化任何事件**。因此重放一个含工具的轮次时：

1. `ProviderRequestStarted` → 状态到 `StreamingResponse`（L66）；
2. 助手 `MessageCommitted` 到达时状态仍是 `StreamingResponse`，L79-80 的 `if state==CollectingToolCalls` 判定为假 → 不推进；
3. `ToolApprovalRequested`（L67 → `ApprovalRequested`）在 `StreamingResponse` 上是**非法转换**（`ApprovalRequested` 仅合法于 `CollectingToolCalls`）→ 产生 `IllegalTransition` issue；
4. 后续 `ToolExecutionStarted`/下一轮 `ProviderRequestStarted` 依次全部非法，状态机**永久卡在 `StreamingResponse`**。

后果：任何含工具调用的 Run 重放后会堆积虚假 `IllegalTransition`（误导运维），`recovered_state` 路径错误（仅因「非终态」被归为 `Interrupted`），直接违反 ADR-016「状态可由事件序列完全重建」。当前 6 项 recovery 测试（[recovery.rs](crates/agent-engine/src/recovery.rs)）**无一包含工具轮次**（事件夹具里没有 `ToolApprovalRequested`/`ToolExecutionStarted`），缺口被完全遮蔽。**建议**：要么为 `StreamFinished{has_tool_calls:true}` 增加可持久化事件标记，要么让重放从助手 `MessageCommitted` 的消息内容（是否含 `ToolCall` part）推断 `CollectingToolCalls`；并补一条「工具轮次重放无 issue」的回归测试。

#### V5 [正确性·中] 软预算警告从不产生事件

[budget.rs:171-216](crates/agent-engine/src/budget.rs) 的 `check()` 会计算 `soft_warnings`（达 80% 默认软阈值的维度），但 `provider_loop.rs` 只在 L193 检查 `report.must_stop()`（硬上限），`soft_warnings` 计算后被丢弃，从不翻译为 `AgentEvent::Diagnostic`。P3-6 验收「达预算产生事件、不静默停」对**软阈值**未满足——用户永远收不到「已用 80% 预算」的预警，只能等到硬上限直接 Failed。**建议**：每轮 `tick_iteration` 后若 `!report.soft_warnings.is_empty()` 则 emit `Diagnostic`，并在首次触发某维度软阈值时记录避免重复刷屏。

#### V6 [正确性·中] 9 维预算中 4 维恒不触发

[provider_loop.rs](crates/agent-engine/src/provider_loop.rs) 实际只调用了 5 个预算记录方法：`tick_iteration`（L192，Iterations）、`record_tokens`（L262，Input/OutputTokens）、`record_tool_call`（L339，ToolCalls）、`record_output`（L368，OutputBytes）。其余 4 个——`record_cost`（Cost）、`set_elapsed`（Duration）、`set_concurrency`（Concurrency）、`record_artifact`（ArtifactBytes）——**在 loop 中零调用**。后果：这四个维度的硬上限（如 `max_duration_ms`/`max_cost_micros`）配置了也永远不会触发，`BudgetController` 的多维承诺名不副实。**传播面**：Phase 14 额度监控依赖 cost 维度，当前无法从 loop 获得费用累计。**建议**：loop 入口记录起始 `Instant` 每轮 `set_elapsed`；token 记录处用 model-registry 定价 `record_cost`；artifact 工具结果处 `record_artifact`。

#### V7 [集成·中] MessageQueue 与 CancelHandle 未接入 Provider Loop

P3-5 的 `MessageQueue`（replace queued / 快照恢复 / 并发不丢，5 项测试）与 P3-8 的 `CancelHandle`（根令牌 + `ProcessTreeCleaner` + 原子门控，3 项测试）都是高质量自实现，但 `provider_loop.rs` 对二者**零引用**：loop 用裸 `CancellationToken`（[provider_loop.rs:179](crates/agent-engine/src/provider_loop.rs)），不持有 `CancelHandle`；循环是单消息驱动的 `run()`，不消费 `MessageQueue`。后果：①运行中用户新消息无处入队（P3-5「运行中可发送」未落地）；②取消不触发进程树清理，P3-8 验收「Cancel 不留下运行进程」在 loop 层无法成立。**建议**：`ProviderLoop::run` 改为消费 `MessageQueue`（每轮 `drain_one` 决定是否续跑），并接收 `CancelHandle` 取代裸 token，使 `cancel()` 联动 `ProcessTreeCleaner`。

#### V8 [正确性·中] ToolScheduler 向工具注入假 workspace/run 上下文

[scheduler.rs:259-265](crates/tool-runtime/src/scheduler.rs) 构造 `ToolExecutionContext` 时硬编码 `WorkspaceId::from("default")`、`RunId::from("default")`、`working_directory: None`。文件类工具（Phase 4 的 read_file/write_file 等）依赖真实 `workspace_id` 解析相对路径、依赖 `working_directory` 确定 cwd，拿到 `"default"` 会解析到错误位置或失败。调度器签名也未暴露注入入口（`execute_named` 无 context 参数）。**建议**：`ToolScheduler::new` 或 `execute_named` 增加 `ToolExecutionContext` 来源（由 Run 携带真实 workspace/run），避免 Phase 4 工具接入时再返工。

#### V9 [集成·中] ProviderLoop 与 ToolScheduler 双轨、从未组合

`ProviderLoop` 通过自定义 `LoopContext` trait 注入工具执行与审批（[provider_loop.rs:36-54](crates/agent-engine/src/provider_loop.rs)），而 P3-4 的 `ToolScheduler` 是另一套独立的并发/串行/审批实现（[scheduler.rs](crates/tool-runtime/src/scheduler.rs)）。两者**从未组合**：没有「`LoopContext` 适配到 `ToolScheduler`」的桥接，调度器的 capability 串行、同文件串行、Git index 串行策略从未被真实 loop 走过。模块头注释（[provider_loop.rs:7-8](crates/agent-engine/src/provider_loop.rs)）自称「既可接 ToolScheduler 也可 Mock 注入」，但该适配器不存在。**建议**：在 app-service 或 agent-engine 内提供 `SchedulerLoopContext` 桥接，并加一条端到端测试（loop + scheduler + 真 capability 冲突场景）。

#### V10 [正确性·低] ToolScheduler.execute() 从 input.name 取工具名，语义错误

[scheduler.rs:233-245](crates/tool-runtime/src/scheduler.rs) 的 `execute()` 从 `request.input.get("name")` 反查工具，而工具名本应来自模型 tool_call 的 `.name`（`PendingToolInvocation.name`），不应藏在 `input` JSON 里。这既与 `execute_named` 的语义重复，又会与「工具自身 input schema 合法含 `name` 字段」冲突。**建议**：废弃 `execute()` 或改为只接受显式工具名；统一走 `execute_named`。

#### V11 [健壮性·低] LoopSink 缓冲整轮流 + 测试名过实

- [provider_loop.rs:575-595](crates/agent-engine/src/provider_loop.rs)：`LoopSink` 把整轮 `ProviderStreamEvent` 全量缓存进 `Vec`，超长生成（长 reasoning/大 tool arguments）的内存随 token 线性增长至轮结束。与 V2 的「不广播」同源——缓冲是为了事后组装，但组装完成后该 Vec 即可丢弃，当前确实在 `events()` clone 后释放，问题可控，仅记录。
- 测试 `cancelled_run_emits_cancelled_and_returns_error` 名字声称验证「emits_cancelled」，实际只断言 `state==Cancelled` 与 `Err(Cancelled)`，不断言 `RunCancelled` 事件被广播，给 V1 的漏发提供了虚假信心。

### 6. 优化建议（按优先级）

#### P0（建议在 Provider Loop 接入真实 Provider/工具前处理）

1. **V1**：四条终止路径统一补发 `RunCancelled`/`RunFailed` 事件（红线：终态可观察 + 可重建），并修测试断言。
2. **V3**：`provider.stream()` 外层包 `RetryController`，断流重试保持上下文——P3-7 的核心承诺，当前完全悬空。
3. **V4**：重放状态机对工具轮次的重建修复（增加 StreamFinished 事件标记或从消息内容推断），补工具轮次回归测试——ADR-016 重建承诺。

#### P1（近期排期）

4. **V2**：`LoopSink` 双写到 `EventBroadcaster`，让 delta 变体对订阅者可见——P3-9 流式分发的真正落地。
5. **V5 + V6**：soft_warnings 翻译为 `Diagnostic` 事件；补齐 cost/elapsed/concurrency/artifact 四维记录——P3-6 多维预算名副其实。
6. **V7**：`ProviderLoop::run` 消费 `MessageQueue` 并接收 `CancelHandle`——P3-5/P3-8 接入主干。
7. **V8**：`ToolScheduler` 注入真实 workspace/run 上下文——Phase 4 工具接入的前置。
8. **V9**：提供 `LoopContext` → `ToolScheduler` 桥接 + 端到端测试——打通 Phase 3 内部双轨。
9. **文档同步**：10 篇 `plan/P3-*.md` 状态与验收勾选回填（AGENTS.md §4）；修订 `provider_loop.rs` 模块头注释使其与实际组合范围一致（V2/V3/V7 的注释失真）。

#### P2（顺手/评估项）

10. **V10**：废弃 `ToolScheduler::execute()` 的 input.name 取名路径。
11. **V11**：`cancelled_run_emits_cancelled...` 测试补事件断言；评估 `LoopSink` 是否可边组装边丢弃已消费 delta 以降峰值内存。
12. 预算耗尽被记为 `RunFailed`（[provider_loop.rs:194](crates/agent-engine/src/provider_loop.rs)）：与「真正的失败」混淆，建议引入独立终态或 `RunStopped{reason: budget}` 语义，便于 GUI 区分「正常预算停止」与「错误失败」。
13. `recovery.rs` 的 `group_by_run` 对每个 envelope `clone`（[recovery.rs:140-148](crates/agent-engine/src/recovery.rs)）：大事件流下有内存放大，可改为按 run 分组借用。
14. `tiktoken-rs` 0.6.0 会拉入 `fancy-regex`/`bstr`/`ndarray`：评估是否可惰性加载 tokenizer（仅 OpenAI 模型才需要），减少非 OpenAI 路径的编译/二进制体积。

### 7. 附录

#### 7.1 Phase 3 模块集成矩阵

| 子任务模块 | 被 ProviderLoop 使用？ | 说明 |
| --- | --- | --- |
| P3-1 状态机（`state.rs`） | ✅ | 循环驱动转换（[provider_loop.rs:182-184](crates/agent-engine/src/provider_loop.rs) 等） |
| P3-6 预算（`budget.rs`） | ⚠️ 部分 | 仅 5/9 维记录；soft_warnings 不发事件（V5/V6） |
| P3-9 广播（`broadcast.rs`） | ⚠️ 部分 | 仅生命周期事件；流式增量不广播（V2） |
| P3-5 消息队列（`queue.rs`） | ❌ | loop 零引用（V7） |
| P3-7 重试（`retry.rs`） | ❌ | loop 零引用（V3） |
| P3-8 取消（`cancel.rs`） | ❌ | loop 用裸 token（V7） |
| P3-4 调度器（`tool-runtime`） | ❌ | 经 `LoopContext` trait 隔离，无桥接（V9） |
| P3-10 恢复（`recovery.rs`） | ➖ | 独立重放路径，工具轮次失真（V4） |
| P3-2 上下文（`context-engine`） | ➖ | 独立 crate，未被 ProviderLoop 调用（Phase 8/13 接线） |

对照 Phase 1/2（各 crate 都有真实消费者），Phase 3 的四个 crate **全部是叶子 crate**：`rg` 全仓库无 app-service/cli-host/core-runtime 依赖 agent-engine / context-engine / tool-runtime（agent-events 仅被 agent-engine、context-engine 引用）。这意味着「测试绿」≠「系统可用」——Phase 13 CLI Host 装配前，主干循环从未被任何宿主真实驱动。

#### 7.2 plan 文档漂移清单

| 文件 | 状态字段 | 未勾验收框 |
| --- | --- | --- |
| plan/P3-1-run-state-machine.md ~ plan/P3-10-interrupted-run-recovery.md（共 10 篇） | 全部 `🟡未开始`（应为 🟢） | 合计 18 个 `- [ ]`，如 [plan/P3-1-run-state-machine.md:24-25](plan/P3-1-run-state-machine.md)、[plan/P3-10-interrupted-run-recovery.md:22-23](plan/P3-10-interrupted-run-recovery.md) |

与 REVIEW-P2 §7.2 同一问题：ROADMAP 状态列已更新为 🟢，但 plan/ 未跟进。Phase 3 的提交未触碰任何 `plan/P3-*.md`。

### 8. 建议的后续动作（本次未执行，供研究）

1. 对 V1/V3/V4 立项（主干可观察性 + 重试落地 + 重建正确性，均属 Phase 3 自身验收范围）。
2. Provider Loop 接线任务（V2/V5/V6/V7/V9）：把已建成但未通电的模块接入主干，建议作为 Phase 13 CLI Host 装配的前置或并行任务。
3. ToolScheduler 上下文注入（V8）：Phase 4 工具实现前完成，避免返工。
4. 基线 + 文档同步小任务（§4 + §7.2）：与 REVIEW-P2 的清理合并一次提交。
5. 端到端集成测试：建立一条「ProviderLoop + ToolScheduler + MessageQueue + 预算 + 重试 + 恢复」的最小真实组合测试，弥补当前「全模块自测、零组合」的覆盖盲区。

---

*评审方法：以 `67d6c4d` 为基线，逐项核对 ROADMAP/plan 状态、源码与依赖清单，并复跑 Phase 3 相关 4 个 crate 的测试与静态门禁（test/clippy/fmt/schema-typegen）；对终止事件缺失、LoopSink 广播、重试接线、replay 状态重建等关键断言直接核对了 `provider_loop.rs`/`recovery.rs`/`state.rs` 的控制流；文中所有结论均给出文件与行号级证据。本文档仅为评审记录，不代表已批准的变更。*


---

## 4. Phase 4（P4）— 核心工具与权限

- **日期**：2026-08-08
- **评审基线**：`main` @ `67d6c4d`（工作树除 `REVIEW-P2.md` / `REVIEW-P3.md` 未跟踪外干净）
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置；后续再研究是否采纳）
- **范围**：ROADMAP.md Phase 4「核心工具与权限」的 12 个任务（P4-1 ~ P4-12）的完成情况、所引入包是否合适、基线偏差；漏洞与优化点一并列出。Phase 4 是关键路径中「Built-in Tools → Policy」一环，上游承接 Phase 3 Agent Loop，下游被 Phase 5（Compaction 引用 checkpoint）、Phase 11（Sandbox 复用 ProcessRuntime / ExecutionConstraints）、Phase 12（Worker 写隔离）依赖，受影响处在文中标注「传播面」。

### 1. 结论摘要

1. **测试全绿，但「绿」的含金量与 Phase 3 同档——单元自测充分、端到端接线缺失**：4 个交付 crate（`builtin-tools` / `policy-engine` / `checkpoint-service` / `process-runtime`）复跑共 **99 passed / 0 failed**（builtin-tools 31、checkpoint-service 13、policy-engine 50、process-runtime 5）；`clippy -D warnings`、`fmt --check` 干净。但 99 项测试全部是「单工具/单模块自测」，没有任何一项覆盖「Scheduler → PolicyEngine → Tool → Checkpoint 回滚」的真实链路。
2. **核心问题与 REVIEW-P3 §2 同源：组件齐全，主干未接线**。`PolicyEngine::decide()`（P4-9 的全部决策逻辑：6 种 ApprovalMode、Shell 风险分类、信任闸门、ExecutionConstraints）**全仓库零生产调用**——唯一调用方是 policy-engine 自己的 13 个单测（[engine.rs:212-376](crates/policy-engine/src/engine.rs)）。执行路径上的 `tool-runtime` 调度器只用一个 `require_approval_for_writes: bool` 布尔（[scheduler.rs:274-283](crates/tool-runtime/src/scheduler.rs)）替代了整套策略引擎；工具描述符里的 `allowed_in_untrusted_workspace` 字段**全仓库无任何强制点**。结果是：P4-10 的「未信任工作区默认限制写/命令」在运行时并不存在闸门。
3. **执行上下文注入假值，checkpoint 上下文断链**：调度器把 `workspace_id` / `run_id` 硬编码为 `"default"`（[scheduler.rs:261-262](crates/tool-runtime/src/scheduler.rs)），导致所有写工具的 checkpoint 都挂在 `"default"` run 下、与真实 Agent run 无关，回滚键全局碰撞。这与 REVIEW-P3 V8/V9（上下文注入假值）同根。
4. **一项数据完整性缺陷**：apply_patch 部分失败回滚不完整——对 `create` 覆盖既有文件、`update`、`delete` 三类操作，错误路径只回滚 `create`（新建）与 `rename`（反向），内容型操作依赖 checkpoint 但**从不调用 `rollback_tool_call`**；尤其 `create` 覆盖既有文件时 `rollback_done` 直接 `remove_file`，原内容丢失。验收「部分失败回滚」仅覆盖 create-new 单一情形（V3）。
5. **包选型总体合理，无「应自实现替换」命中**：regex（线性时间、ReDoS 安全）、ignore+globset（ripgrep 同源）、chardetng+encoding_rs（Mozilla 编码检测）、libc（Unix 进程组）、blake3（checkpoint 内容寻址）使用面都覆盖核心价值区。按基线「参考+自实现」落地的 edit_file / apply_patch 匹配器方向正确，但**违反基线自定的「需完整 fuzz 与审计」标准——零属性/fuzz 测试**（基线原文见 ROADMAP「完全自实现」与 §3.3）。
6. **基线管理优于 Phase 1/2/6**：无「引入未登记」依赖；唯一的「声明未引用」是 `content-inspector`（基线记 P4-1，但 read_file 实际只用 chardetng+encoding_rs，全仓库零引用）。另有 4 个 crate 内死依赖（agent-domain×2、bytes、futures）与 `atomic_write` 四处重复实现，属清理项。
7. **流程合规**：12 篇 `plan/P4-*.md` 状态均为 `🟢已完成`、验收框全部已勾，提交 `da1c260` 同步更新了 ROADMAP——**纠正了 Phase 2/3 的 plan 停留 🟡、验收未勾的流程偏差**（见 REVIEW-P2 §1-5、REVIEW-P3 §1-5）。docs/features（tools/policy/checkpoint/process）与 ADR-009/010 均已就位。
8. **四个「mock 过得去、真实运行会暴露」的中危项**：① run_command 非真流式（缓冲全集后一次性 emit 单 delta，长构建用户全程无输出，V8）；② Windows env allowlist 缺 SYSTEMROOT/TEMP 等，env_clear 后复杂工具链行为异常（V5）；③ edit_file 模糊匹配吞文件尾换行（V6）；④ list_directory 单个 dangling symlink 致整目录列出失败（V7）。
9. **checkpoint 崩溃恢复缺口**：checkpoint 元数据（run→change→blob 映射）纯内存（[lib.rs:151/155](crates/checkpoint-service/src/lib.rs)），进程崩溃后索引丢失、无法回滚——与 P3-10「崩溃后 <1s 恢复」目标及 ADR-010「所有改动可撤销」矛盾（V9）。blob 本身持久，但映射不持久。
10. **路径安全是全 Phase 4 最扎实的一环**：`resolve_workspace_path`（[path.rs](crates/policy-engine/src/path.rs)）防穿越/绝对路径/`.git`/symlink 跳出/设备文件/TOCTOU，逻辑完备、15 项测试，且被 builtin-tools 实际复用——是少数真正接线的策略能力。

### 2. P4 任务完成情况核对表

| 任务 | 交付 crate/模块 | 状态 | 关键证据 |
| --- | --- | --- | --- |
| P4-1 read_file | `builtin-tools/read_file.rs` | 🟢（有 V11） | 行号/offset/limit、二进制+chardetng 编码检测、路径经 policy-engine；但整文件 `std::fs::read` 后再切片（[read_file.rs:93](crates/builtin-tools/src/read_file.rs)） |
| P4-2 write_file | `builtin-tools/write_file.rs` | 🟢 | 原子写 tmp+sync+rename、建父目录、保留 unix mode、写前 checkpoint（[write_file.rs](crates/builtin-tools/src/write_file.rs)） |
| P4-3 edit_file | `builtin-tools/edit_file.rs` | 🟢（有 V6/V13） | 精确替换/多段预演原子/uniqueness 冲突/模糊匹配；但模糊模式吞尾换行（[edit_file.rs:317](crates/builtin-tools/src/edit_file.rs)） |
| P4-4 apply_patch | `builtin-tools/apply_patch.rs` | 🟡部分（有 V3） | create/update/delete/rename、dry run、原子提交；但部分失败回滚不完整（[apply_patch.rs:313-329](crates/builtin-tools/src/apply_patch.rs)），验收仅覆盖 create-new |
| P4-5 run_command | `builtin-tools/run_command.rs` | 🟡部分（有 V5/V8） | exit code/timeout/cancel/进程树终止正确；但「流式」名不副实（[run_command.rs:151-160](crates/builtin-tools/src/run_command.rs)），Windows env 缺关键变量（[run_command.rs:31](crates/builtin-tools/src/run_command.rs)） |
| P4-6 search_text | `builtin-tools/search_text.rs` | 🟢（有 V10/V11） | 固定串/正则（regex 线性时间无 ReDoS）、glob+ignore、上下文行、字节预算；但入口单点 cancel、整文件入内存 |
| P4-7 find_files | `builtin-tools/find_files.rs` | 🟢（有 V10） | glob/类型/深度/ignore/稳定排序、结果受限；但入口单点 cancel |
| P4-8 list_directory | `builtin-tools/list_directory.rs` | 🟡部分（有 V7/V12） | 类型/大小/mtime/symlink/分页；但 dangling symlink 致整目录失败（[list_directory.rs:113](crates/builtin-tools/src/list_directory.rs)）、全收集后分页 |
| P4-9 Policy Engine | `policy-engine` | 🟡部分（有 V1/V4） | 6 种 ApprovalMode、路径安全、Shell 分类、信任闸门、50 测试；但 `decide()` **零生产调用**，执行路径未接线 |
| P4-10 Workspace Trust | `policy-engine`+`workspace-service` | 🟡部分（见 V1/V2） | `PolicyInput.trusted` + `requires_trust` 模型在；但信任来源未接线、`allowed_in_untrusted_workspace` 不强制、调度器上下文为假值 |
| P4-11 Checkpoint 与回滚 | `checkpoint-service` | 🟢（有 V9/V11） | 写前 snapshot、按 tool_call/run 逆序回滚、冲突检测（BLAKE3）、不 `git reset --hard`；但元数据纯内存（崩溃不可回滚） |
| P4-12 Process Runtime | `process-runtime` | 🟢（有 V14） | Unix 进程组 `setpgid`+`killpg`、Windows `taskkill /T`、无死锁并发读、max_output 截断、timeout+cancel；但 `spawn_stream` 返回死句柄（[lib.rs:258](crates/process-runtime/src/lib.rs)） |

**门禁证据（2026-08-08 复核）**：

- `cargo test -p builtin-tools -p policy-engine -p checkpoint-service -p process-runtime`：**99 passed / 0 failed**（builtin-tools 31、checkpoint-service 13、policy-engine 50、process-runtime 5）。
- `cargo clippy -p <同上> --all-targets -- -D warnings`：干净。
- `cargo fmt -p <同上> -- --check`：干净（退出码 0）。
- 各 `plan/P4-*.md` 验收项均已勾选；ROADMAP Phase 4 计数 12/12 🟢。

### 3. 包选型评估

#### 3.1 建议保留（自实现不值得）

| 包 | 版本 | 使用点 | 使用面评估 | 结论 |
| --- | --- | --- | --- | --- |
| `regex` | 1 | P4-6 search_text | 线性时间引擎，从结构上消除 ReDoS，满足 P4-6「无 ReDoS」验收；`RegexBuilder` 控制大小写 | **保留** |
| `ignore` + `globset` | 0.4 / 0.4 | P4-6、P4-7 | WalkBuilder+GitignoreBuilder、GlobSet 多模式匹配，ripgrep 同源 | **保留** |
| `chardetng` + `encoding_rs` | 0.1 / 0.8 | P4-1 read_file | Mozilla 编码检测 + 解码，`decode` 损失式兜底并标注 | **保留** |
| `libc` | 0.2 | P4-12 process-runtime | Unix `setpgid`/`killpg`，`unsafe` 边界最小（仅进程组信号） | **保留** |
| `blake3` | 1 | P4-11 checkpoint | 冲突检测重算哈希、blob 内容寻址，SIMD 加速自实现不可企及 | **保留** |
| `tokio`（process/io-util/sync/time） | 1 | P4-5、P4-12 | 子进程/管道/超时/cancel 标准原语 | **保留** |
| `serde`/`serde_json`/`thiserror`/`async-trait`/`tracing` | 基线版本 | 全局 | 基础设施，无争议 | **保留** |

#### 3.2 需要重新评估的项

| 项 | 现状 | 建议 |
| --- | --- | --- |
| `content-inspector = "0.2"` | 基线记 P4-1 使用，但 read_file 实际只用 `chardetng`，**全仓库零引用**（`rg content_inspector` 无命中） | **移出基线**（声明虚置，与 REVIEW.md 的 uuid/tracing-appender/similar 同类） |
| `agent-domain`（policy-engine） | [policy-engine/Cargo.toml](crates/policy-engine/Cargo.toml) 声明，源码零引用（policy-engine 只用 tool_api） | 删除该依赖；不影响接口 |
| `agent-domain`（checkpoint-service） | [checkpoint-service/Cargo.toml](crates/checkpoint-service/Cargo.toml) 声明，源码零引用 | 删除该依赖 |
| `bytes`、`futures`（process-runtime） | [process-runtime/Cargo.toml](crates/process-runtime/Cargo.toml) 声明，源码零引用（实际只用 tokio 原语） | 删除；REVIEW-P2 曾把 futures/bytes 列为「引入未登记」，现已回填基线但在此 crate 仍是死引用 |

#### 3.3 「自实现替换包」总体判断

针对「引用面小 → 自实现」命题：**P4 范围内没有命中**。每个被引用包使用面都覆盖核心价值区。反向看，按基线「参考+自实现」落地的 edit_file / apply_patch 精确匹配与 fuzzy 匹配器方向正确（安全关键路径需可控语义），但**违反基线自定的验收标准**——ROADMAP「完全自实现」表对 apply_patch/edit_file 明确要求「需完整 fuzz 与审计」，而 builtin-tools 与 checkpoint-service **零 proptest/arbitrary/cargo-fuzz 目标**（`rg arbitrary|proptest|fuzz` 仅误命中 fuzzy 特性名）。建议补属性测试：随机 `old_string`/`new_string`/文件内容组合，断言不 panic、`occurrences` 计数与最终内容一致、回滚后与原文逐字节相等。

### 4. 基线偏差清单

规则来源：ROADMAP「依赖选型基线」要求新增依赖同步回填基线表。

| 类型 | 项 | 位置 | 说明 |
| --- | --- | --- | --- |
| 声明未引用 | `content-inspector` | [Cargo.toml:103](Cargo.toml) | 基线记 P4-1，实际零引用，见 §3.2 |
| crate 内死依赖 | `agent-domain` | [policy-engine/Cargo.toml](crates/policy-engine/Cargo.toml)、[checkpoint-service/Cargo.toml](crates/checkpoint-service/Cargo.toml) | 两 crate 源码均零引用 |
| crate 内死依赖 | `bytes`、`futures` | [process-runtime/Cargo.toml](crates/process-runtime/Cargo.toml) | 源码零引用 |

**对比**：Phase 4 **无「引入未登记」**（所有外部依赖均在 workspace 基线内），基线卫生优于 Phase 1（6 个未登记）/Phase 2（futures/bytes）/Phase 6（base64 等）。`cargo build`/`clippy` 不会报死依赖（路径依赖会被链接），需 `cargo machete`/`cargo udeps` 才能检出——建议在 CI 增加一道。

**建议**：一次小型清理——移出 `content-inspector` 基线声明、删 3 个 crate 的 4 个死依赖、把四处重复的 `atomic_write` 下沉到 `builtin-tools/common`（或 checkpoint-service 导出复用），CI 增加 machete/udep 门禁。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V14）。

#### V1 [安全·高] PolicyEngine 主干未接线，`allowed_in_untrusted_workspace` 完全不强制

`PolicyEngine::decide()` 的全部 13 处调用都在 policy-engine 自测内（[engine.rs:212-376](crates/policy-engine/src/engine.rs)），**全仓库无任何生产调用方**（`rg "\.decide\("` 仅命中 policy-engine 测试与 agent-engine 的 `RetryPolicy.decide`，后者是重试策略、无关）。执行路径上 `tool-runtime` 调度器自带的 `requires_approval` 只看 `config.require_approval_for_writes` 布尔 + capability 类型（[scheduler.rs:274-283](crates/tool-runtime/src/scheduler.rs)），不查信任、不分类 Shell、不用 ApprovalMode。工具描述符里的 `allowed_in_untrusted_workspace`（read 工具 `true`、写工具 `false`）**全仓库零强制点**（`rg allowed_in_untrusted_workspace` 排除赋值后无命中）。

后果：P4-9 的审批模式、P4-10 的「未信任工作区默认限制写/命令」（ADR-009）在运行时**不存在闸门**——与 REVIEW.md V1（`trust_workspaces` 未消费）同一攻击面的延续：一旦接线就有自我提权风险，但当前根本未接线，所以是「安全能力未生效」而非「漏洞被利用」。传播面：Phase 11 Sandbox 引用了 `policy_engine::ExecutionConstraints`（[sandbox-runtime/src/lib.rs:56-57](crates/sandbox-runtime/src/lib.rs)），但只取约束类型、不走 decide。

#### V2 [正确性/安全·高] 调度器硬编码上下文，checkpoint 上下文断链

[scheduler.rs:261-262](crates/tool-runtime/src/scheduler.rs) 构造 `ToolExecutionContext` 时 `workspace_id` / `run_id` 均写死 `"default"`、`working_directory: None`。后果：① write_file/edit_file/apply_patch 调 checkpoint 时传入 `run_id="default"`，所有 run 的改动挂在同一 key 下，回滚键全局碰撞、跨 run 互相污染；② 真实 Agent run 的 run_id 永远到不了工具。与 REVIEW-P3 V8/V9（上下文注入假值、Scheduler 未与 ProviderLoop 桥接）同根。传播面：Phase 5 Compaction、Phase 12 Worker 写隔离都依赖 run 级 checkpoint 正确归属。

#### V3 [数据完整性·高] apply_patch 部分失败回滚不完整，create 覆盖既有文件会丢原内容

部分失败路径调用 `rollback_done`（[apply_patch.rs:313-329](crates/builtin-tools/src/apply_patch.rs)），其语义为：`Create` → `remove_file`；`Delete`/`Update` → 空操作（注释「由 checkpoint rollback 恢复」）；`Rename` → 反向 rename。问题：① 错误路径**从不调用 `checkpoint_service::rollback_tool_call`**，注释承诺的 checkpoint 恢复无人触发，Update/Delete/Create-over-existing 的已应用改动**留在半应用状态**；② 特别地，`create` 覆盖既有文件时（[apply_patch.rs:182-188](crates/builtin-tools/src/apply_patch.rs) 已为其拍了 snapshot），`rollback_done` 仍走 `Create => remove_file`（[apply_patch.rs:319-321](crates/builtin-tools/src/apply_patch.rs)），**直接删除文件、原内容丢失**。验收「部分失败回滚」的测试 `partial_failure_rolls_back` 只覆盖 create-new + rename-fail，未覆盖 update/delete/create-over-existing。**建议**：rollback_done 对已 snapshot 的路径改用 checkpoint 内容恢复；或在错误路径追加一次 `rollback_tool_call` 调用，并补三类回归测试。

#### V4 [安全·中] NeverAsk/OnFailure 完全跳过 Shell 风险分类，无硬拒绝地板

[engine.rs:63](crates/policy-engine/src/engine.rs) 对 `NeverAsk | OnFailure` 直接 `allow_or_constrained(cap)`，不调用 `effective_risk`/`classify_command`——即 trusted + NeverAsk 下 `rm -rf /`、`dd of=/dev/sda` 被 `AllowWithConstraints`（仅附 timeout/输出上限）放行。Shell 分类器（[shell.rs](crates/policy-engine/src/shell.rs)）只在 AlwaysAsk/AskForWrites/AskForDangerous 三种模式下才生效。当前因 V1 引擎未接线而仅具理论风险，但一旦接线，最宽松模式对最具破坏性命令也无「硬拒绝地板」。**建议**：增加一个无视 ApprovalMode 的 denylist 地板（如 `rm -rf /`、`mkfs`、`dd of=/dev/` 恒 Deny 或恒 AskUser），把 Shell 分类从「装饰」提升为「底线」。

#### V5 [安全/正确性·中] Windows env allowlist 缺失关键变量

[run_command.rs:31](crates/builtin-tools/src/run_command.rs) `ENV_ALLOWLIST = ["PATH","HOME","LANG","LC_ALL","TERM"]`，配合 `spec.env_clear = true`（[run_command.rs:131](crates/builtin-tools/src/run_command.rs)）。Windows 上：① 缺 `SYSTEMROOT`（cmd.exe / 多数程序加载 ntdll 等系统 DLL 依赖它）、`TEMP`/`TMP`（写临时文件的工具失败）、`USERPROFILE`、`COMSPEC`、`PATHEXT`；② `HOME` 在 Windows 通常不存在（用 `USERPROFILE`），`LANG`/`LC_ALL`/`TERM` 多为空，实际只透传了 PATH。本机 smoke test（`echo hello`）能过是因为 cmd 内建命令不触系统 DLL 加载，但 `cargo build`、`git`、PowerShell 脚本等真实工具链会异常或行为偏差。allowlist 还硬编码、Unix 中心、无工作区配置透传。**建议**：按平台分桶（Windows 额外含 SYSTEMROOT/TEMP/TMP/USERPROFILE/COMSPEC/PATHEXT），并允许配置层追加透传变量。

#### V6 [正确性·中] edit_file 模糊匹配吞文件尾换行

`replace_fuzzy` 用 `content.lines()`（剥终止换行）收集后 `out.join("\n")` 重建（[edit_file.rs:298-317](crates/builtin-tools/src/edit_file.rs)）。`str::lines()` 不保留结尾 `\n`，`join("\n")` 不补回——因此当模糊匹配窗口含末行时，源文件结尾的 `\n` 被静默吞掉。非模糊 `replacen` 路径操作原始字符串、不受影响。后果：对源文件做一次末行模糊编辑即丢失 POSIX 文本文件的结尾换行（可能触发 lint/格式门禁）。现有测试 `fuzzy_match_normalizes_whitespace` 用单行无尾换行样本，未覆盖此情形。**建议**：重建时根据原文是否以 `\n` 结尾补回；或记录并保留尾换行。

#### V7 [健壮性·中] list_directory 单个 dangling symlink 致整目录列出失败

[list_directory.rs:111-115](crates/builtin-tools/src/list_directory.rs) 对每个 entry 调 `entry.metadata()?`——`DirEntry::metadata` **跟随符号链接**，dangling symlink 返回 `NotFound` 并经 `?` 传播，**整目录列出失败**。验收「symlink 信息正确」的测试只造了有效 symlink。后果：工作区里一个失效链接就让 list_directory 整体报错，Agent 无法浏览。**建议**：改用 `symlink_metadata`（不跟随）判类型/大小，对跟随失败的目标降级为「broken symlink」而非整体失败。

#### V8 [可用性·中] run_command 非真流式

[run_command.rs:148-160](crates/builtin-tools/src/run_command.rs) 先 `runtime.run(spec, cancel).await`（缓冲全集 stdout/stderr），完成后一次性 `sink.emit(OutputDelta{...})` 各发一个 delta——注释自承「结果以事件回放保证流式可见」。对长构建/测试，用户全程看不到增量输出，直到进程结束才一次性涌入。process-runtime **已有**真流式 `spawn_stream`（[process-runtime/src/lib.rs:219](crates/process-runtime/src/lib.rs)）但未被 run_command 采用。与 REVIEW-P3 V2（流式增量被 LoopSink 全量缓冲、从不广播）同类。**建议**：run_command 改用 `spawn_stream`，边读边 emit。

#### V9 [健壮性·中] checkpoint 元数据纯内存，崩溃后不可回滚

checkpoint 的 run→change→blob 映射存于 `Arc<Mutex<BTreeMap<...>>>`（[lib.rs:151](crates/checkpoint-service/src/lib.rs)）与 `paths` 映射（[lib.rs:155](crates/checkpoint-service/src/lib.rs)），**不持久化**。blob 本身经 ArtifactStore 落盘，但索引纯内存。进程崩溃后（正是 P3-10「Interrupted Run 恢复」要处理的场景）映射丢失，rollback_run/rollback_tool_call 找不到记录，ADR-010「所有 Agent 改动可审查与撤销」在崩溃路径上不成立。传播面：Phase 5 Compaction、Phase 12 Worker 回滚均依赖 checkpoint 可恢复。**建议**：把 RunCheckpoint（或其投影）写入 session-store / Event Store，崩溃恢复时重建映射。

#### V10 [健壮性·低] search_text / find_files 仅入口单点 cancel

二者各只在 `execute` 入口检查一次 `cancel.is_cancelled()`（[search_text.rs:81](crates/builtin-tools/src/search_text.rs)、[find_files.rs:81](crates/builtin-tools/src/find_files.rs)），随后进入同步 `WalkBuilder` 遍历 + 逐文件读取，全程不再查 cancel。大仓库（数十万文件）长扫描无法中途取消。**建议**：遍历循环内每 N 个 entry 或每文件检查一次 cancel。

#### V11 [性能·中] 多工具在 async 中做阻塞 std::fs 且整文件入内存

- read_file：`std::fs::read(&absolute)` 整文件入内存后再 offset/limit 切片（[read_file.rs:93](crates/builtin-tools/src/read_file.rs)）；`MAX_OUTPUT_BYTES` 只限渲染输出、不限读取，多 GB 日志会撑爆内存。
- search_text：`std::fs::read_to_string(path)` 逐文件全量读（search_text.rs `scan_file` 前），大文件内存与阻塞风险。
- checkpoint：`std::fs::read(&absolute)` 全量读后存 blob（[lib.rs:193](crates/checkpoint-service/src/lib.rs)）。
- 三者均在 `async fn execute` 内同步阻塞调用（无 `spawn_blocking`），慢盘/大文件会卡住 tokio worker 线程（与 REVIEW.md P1-6 cli-host shell 阻塞同型）。

**建议**：读路径改 `tokio::fs` + 流式/分块（read_file 按行流读至 limit）；重 IO 包 `spawn_blocking`。

#### V12 [性能·低] list_directory 全收集后分页，分页名不副实

[list_directory.rs:111-157](crates/builtin-tools/src/list_directory.rs) 先把 `read_dir` 全部 entry 收进 `Vec<Entry>`（含每项两次 metadata 系统调用），排序后 `skip(offset).take(limit)`。超大目录（如 node_modules、构建产物）O(N) 内存与时间，offset/limit 不省成本。**建议**：对稳定排序需求可接受现状，否则记录总数后单次扫描取页。

#### V13 [性能·低] edit_file 模糊匹配 O(L·n) 行拼接

`count_fuzzy`/`replace_fuzzy` 对 `content.lines().windows(n)` 每个窗口做 `join("\n")` + `normalize_ws`（[edit_file.rs:277-317](crates/builtin-tools/src/edit_file.rs)），每窗口 O(n) 拼接，整体 O(L·n)。大文件 + 大 `old_string` 块时偏慢。**建议**：滚动哈希或规范化后单次扫描匹配。

#### V14 [健壮性·低] process-runtime `spawn_stream` 返回死句柄且不限输出

[lib.rs:258](crates/process-runtime/src/lib.rs) `let handle = ProcessHandle { child: None }`——child 已 move 进 spawned task，句柄内无 child，故 `handle.kill()`/`handle.id()` 恒空操作；唯一停止路径是 `cancel` token。同时 `spawn_stream` 的 `stream_lines` **不执行 `max_output_bytes`**（见 [process-runtime/src/lib.rs stream_lines](crates/process-runtime/src/lib.rs)），`Exit` 事件恒 `truncated: false`（[lib.rs:253](crates/process-runtime/src/lib.rs)）。缓冲 `run()` 路径有截断、流式路径无——语义不一致。**建议**：句柄持有发送端或 child id 以支持外部 kill；流式路径补输出上限与截断标记。

### 6. 优化建议（按优先级）

#### P0（建议在下一阶段开工前处理）

1. **V1**：把 `PolicyEngine::decide()` 接入 `tool-runtime` 调度器，用 `PolicyDecision` 替换 `require_approval_for_writes` 布尔；同时让调度器强制 `allowed_in_untrusted_workspace`（未信任工作区 + 该字段 false → Deny）。这是 Phase 4 安全边界的「通电」动作，成本最低时点就是现在（未接线阶段）。
2. **V2**：调度器从真实 `ToolExecutionContext`（workspace_id / run_id / working_directory）注入，消除 `"default"` 假值；与 REVIEW-P3 V8/V9 合并处理。
3. **V3**：apply_patch 部分失败回滚补全（内容型操作走 checkpoint 恢复，create-over-existing 不删除原文件）+ 三类回归测试。

#### P1（近期排期）

4. **V9**：checkpoint 元数据持久化（session-store / Event Store 投影），支撑崩溃后回滚——ADR-010 与 P3-10 的前置。
5. **V8**：run_command 改用 `spawn_stream` 实现真流式。
6. **V5**：env allowlist 按平台分桶 + 配置可透传。
7. **V6**：edit_file 模糊匹配保留尾换行 + 回归测试。
8. **V7**：list_directory 改 `symlink_metadata`、dangling 降级。
9. **V11**：read_file/search_text/checkpoint 改流式或 `spawn_blocking`，避免 worker 阻塞与整文件入内存。
10. **V4**：增加无视 ApprovalMode 的危险命令硬拒绝地板。
11. **§3.3**：为 edit_file/apply_patch 匹配器补 proptest/arbitrary 属性测试（满足基线「需完整 fuzz」标准），断言不 panic、计数一致、回滚逐字节相等。

#### P2（顺手/评估项）

12. **基线清理**（§4）：移 `content-inspector` 基线声明、删 4 个 crate 内死依赖、`atomic_write` 四处下沉复用、CI 增 `cargo machete`/`cargo udeps` 门禁。
13. **V10**：search/find 遍历内周期性查 cancel。
14. **V12**：list_directory 大目录分页优化。
15. **V13**：edit_file 模糊匹配滚动匹配。
16. **V14**：spawn_stream 句柄可 kill + 流式输出上限。

### 7. 附录

#### 7.1 Phase 4 与「优先级 P1」标签任务

ROADMAP 中 Phase 4 的 12 个任务**均无 P1 标签**（全部为 P0），故无跨 Phase 的 P1 任务需在此追踪。涉及 P4 产物的 P1 任务：P9-7（MCP OAuth，⚪）会复用 auth 能力，与 P4 无直接耦合。

#### 7.2 文档与 ADR 现状

- `docs/features/`：tools.md / policy.md / checkpoint.md / process.md 均已存在（Phase 4 模块文档齐全）。
- ADR：ADR-009（默认工作区信任）、ADR-010（全写 checkpoint）均已就位并被 plan 引用。
- 注：本文档未逐字审阅 docs 内容，仅确认存在性；如需文档级评审另开任务。

### 8. 建议的后续动作（本次未执行，供研究）

1. 对 V1/V2 立项（安全边界通电 + 上下文接线）——与 REVIEW-P3 §8 的「主干接线」合并为一个跨 Phase 任务。
2. 对 V3 立项（apply_patch 回滚补全 + 回归测试）。
3. checkpoint 持久化（V9）作为 P3-10/P5 的前置评估。
4. 基线清理小任务（§4 + §3.2），一次提交完成。
5. 匹配器属性测试补全（§3.3 / P1-11），满足基线自定 fuzz 标准。

---

*评审方法：以 `67d6c4d` 为基线，逐项核对 ROADMAP/plan 状态、源码与依赖清单，并复跑 4 个 Phase-4 crate 的测试与静态门禁；文中所有结论均给出文件与行号级证据。本文档仅为评审记录，不代表已批准的变更。*


---

## 5. Phase 5（P5）— Session 树、Compaction 与上下文裁剪

- **日期**：2026-08-08
- **评审基线**：`main` @ `67d6c4d`（工作树仅含未跟踪的 REVIEW-P*.md，无源码改动）
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置；后续再研究是否采纳）
- **范围**：ROADMAP.md Phase 5 的 9 个任务（P5-1 ~ P5-9）的完成情况、所引入包是否合适、是否存在更优替代或自实现替换的必要；基线偏差（声明未引用 / 引入未登记）；漏洞与优化点一并列出。格式对齐 [REVIEW.md](REVIEW.md)。

### 1. 结论摘要

1. **完成度基本可信**：P5-1 ~ P5-9 全部 🟢。三个交付 crate（`session-store`、`compaction-engine`、`context-engine`）于 2026-08-08 复跑 `cargo test` 共 **63 项测试全部通过**（10 + 26 + 27）；`clippy -D warnings` 与 `fmt --check` 干净。各 plan 的验收项大多已勾选并有对应测试。
2. **包选型无问题**：三个 crate 实际引用的包（`serde` / `serde_json` / `thiserror` / `rusqlite` / `tokio` / `tiktoken-rs`）全部是基线内依赖，**Phase 5 没有引入任何新依赖**，因此不存在「声明未引用 / 引入未登记」的 Phase 5 新增偏差。基线已把 Compaction 引擎、JSONL 流式解析划为完全自实现，落地与此一致。
3. **两个正确性中风险值得在多分支上线前处理**：（V1）压缩引擎 `compact()` 用 `replay_events` 跨分支读取事件，导致 `replaced_range` 与 recovery fork 引用的 `branch_id` 和实际折叠的事件集合不对应；（V2）`import_session` 把导出里**所有分支**的事件都经 `append_event(active_branch)` 回灌，多分支会话往返后事件→分支归属丢失（现有往返测试只覆盖单分支，故未暴露）。
4. **Pi 导入器两处实现缺陷**：（V3）未知字段收集循环用 `unknown_entries.insert(0, …)` 固定写 key 0，多条未知记录互相覆盖，与「保存未知字段」验收项相悖，且测试未断言该报告内容；（V4）`ModelSwitch` 只计数不落事件，模型切换信息实际未还原；（V5）`import_pi_jsonl` 用同步 `std::fs::read_to_string` 读整个文件，阻塞 async 线程且无大小上限。
5. **能力已建但尚未接线**：`compaction-engine` 与 `context-engine` 在整个 workspace 中**没有任何消费者**（仅各自 Cargo.toml 出现），`trim_tool_result` 也未进入 `ContextBuilder`。与 Phase 1 的 `app-service` 骨架同理，属预期（接线在 `agent-engine`，后续阶段完成），但意味着这些路径尚无集成验证。
6. **中文场景的 token 估算偏差**（V6）：`HeuristicEstimator` 默认 `chars/4`、`estimate_text_tokens` 同样 `chars().count()/4`，对 CJK 文本会把 token 数低估约 4–6 倍；tiktoken（OpenAI 系）不受影响，问题集中在 Anthropic/Gemini 的启发式路径，当前 Provider 未接线为潜伏风险。

### 2. P5 任务完成情况核对表

| 任务 | 交付 crate / 模块 | 状态 | 关键证据 |
| --- | --- | --- | --- |
| P5-1 Session Tree / Fork | `session-store::session_tree` | 🟢 | [fork_from_event](crates/session-store/src/session_tree.rs:35) 从任意事件分叉并校验事件存在/分支重名；[events_by_branch](crates/session-store/src/session_tree.rs:134) 按 branch+sequence 分页读取，大 session 不全量加载 |
| P5-2 Branch 切换 | `session-store::event_store` | 🟢 | [switch_branch](crates/session-store/src/event_store.rs:80) 切换 active branch；[append_event](crates/session-store/src/event_store.rs:129) 内 `active_branch != branch_id` 即拒写，并发写受保护 |
| P5-3 Resume/归档/删除/重命名 | `session-store::lifecycle` + `lib` | 🟢 | rename/archive/unarchive/resume/delete 齐全；[acquire_lease](crates/session-store/src/lifecycle.rs:169)/renew/release 带过期抢占；[integrity_check](crates/session-store/src/lifecycle.rs:321) 只读检测 sequence 间隙与 parent 缺失；[open_read_only](crates/session-store/src/lib.rs:62) 提供损坏后只读恢复入口 |
| P5-4 搜索 / 标签 | `session-store::search` | 🟢 | add/set/remove/list 标签（小写归一、去重）；search_sessions 命中标题/标签/内容并按维度去重 |
| P5-5 Compaction 引擎 | `compaction-engine::engine` | 🟢（决策态） | [compact](crates/compaction-engine/src/engine.rs:85) 读事件、Fork recovery branch（[create_branch](crates/compaction-engine/src/engine.rs:112)）、应用保留策略、产出版本化快照；快照版本化见 [snapshot.rs](crates/compaction-engine/src/snapshot.rs) |
| P5-6 压缩保留策略 | `compaction-engine::retention` | 🟢 | [apply](crates/compaction-engine/src/retention.rs:115) 纯函数：system 永留、最近 N 轮、未解决任务、用户约束、修改文件、pending/failed tool call；golden session 场景见 engine 测试 |
| P5-7 Tool Result 裁剪 | `context-engine::tool_result_trim` | 🟢（逻辑）/ ⚠️（未接线） | [classify](crates/context-engine/src/tool_result_trim.rs:54) 四级；[trim_tool_result_with](crates/context-engine/src/tool_result_trim.rs:151) 大/超大转 ArtifactReference 并暂存 `retained_full`；但 `ContextBuilder` 未调用，全 workspace 无消费者 |
| P5-8 Export / Import | `session-store::export_import` | 🟢（单分支）/ ⚠️（多分支） | [SessionExport](crates/session-store/src/export_import.rs:48) 带 `schema_version`；[import_session](crates/session-store/src/export_import.rs:168) 重建；往返测试仅单分支 |
| P5-9 Pi JSONL Importer | `session-store::pi_import` | 🟢 / ⚠️ | header/message/tool/model/compaction/branch 解析齐全、保留未知字段、不改原文件（测试断言 before==after）；但 V3/V4/V5 三处缺陷（见 §5） |

**门禁证据（2026-08-08 复核，基线 `67d6c4d`）**：

- `cargo test -p session-store -p compaction-engine -p context-engine`：**63 passed / 0 failed**（compaction-engine 10、context-engine 26、session-store 27）。
- `cargo clippy -p session-store -p compaction-engine -p context-engine --all-targets -- -D warnings`：干净。
- `cargo fmt --all -- --check`：干净。

### 3. 包选型评估

#### 3.1 建议保留（自实现不值得）

| 包 | 版本 | 使用点 | 使用面评估 | 结论 |
| --- | --- | --- | --- | --- |
| `rusqlite` | 0.32（workspace） | P5-1~4/8/9（session-store 全模块） | 分支树、租约、搜索、导入导出全走 SQLite Actor 绑定层 | **保留** |
| `tiktoken-rs` | 0.6（workspace） | P5-7 所在的 `context-engine::token` | OpenAI 系精确计数（[TiktokenEstimator](crates/context-engine/src/token.rs:120)），与基线「仅对 OpenAI 系精确」一致 | **保留** |
| `serde` / `serde_json` / `thiserror` / `tokio` | 基线版本 | 全局 | 基础设施 | **保留** |

#### 3.2 自实现判断

基线把 **Compaction 引擎（P5-5/6）** 与 **JSONL 流式解析（P5-9，serde_json 逐行）** 列为完全自实现，落地完全吻合：

- `retention::apply` 是纯函数、确定性（BTreeSet 按 `EventId` 排序输出）、无 IO，保留语义完全可控——正确选择。
- `pi_import` 逐行 `serde_json::from_str` + 宽松字段识别，未引入额外 JSONL 框架——符合基线「serde_json 逐行即可」。
- `tool_result_trim` 的分级裁剪是 Pawork 特定语义（小/中/大/超大 + ArtifactReference 占位），无对应现成包，自实现正确。

**结论：Phase 5 范围内没有任何「引用面小、自实现更划算」的第三方包，不需要自实现替换，也不需要新增依赖。** 唯一可商榷的是 §5 V6 的中文 token 启发式（属参数调优，不涉及换包）。

### 4. 基线偏差清单

**Phase 5 三个 crate 引入的偏差：零。** 所有依赖均为基线已登记项。

REVIEW.md §4 记录的 workspace 级历史偏差在本基线仍存在（属 Phase 1/6/7 范畴，不在本次修复目标内，仅同步现状）：

| 类型 | 项 | 现状 | 备注 |
| --- | --- | --- | --- |
| 声明未引用 | `uuid`、`tracing-appender`、`similar` | 仍零引用（`similar` 唯一命中是 [parser.rs:8](crates/diff-service/src/parser.rs:8) 注释里的单词 "similarity"，非 crate 使用） | 与 REVIEW.md 一致，未恶化 |
| 引入未登记 | `parking_lot`、`tempfile`、`base64`、`rand`、`sha2`、`url` | 仍仅在各 crate Cargo.toml，未回填 workspace 基线 | 与 REVIEW.md 一致 |

**建议**：沿用 REVIEW.md §6 的「一次性基线清理小任务」处理，不与 Phase 5 混改。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V10）。

#### V1 [正确性·中] 压缩引擎跨分支读取事件

[engine.rs:93](crates/compaction-engine/src/engine.rs:93) 调用 `replay_events(session_id, 1, usize::MAX)`，而 `replay_events` 的查询不带 `branch_id`（[event_store.rs:229](crates/session-store/src/event_store.rs:229)，仅按 `session_id + sequence`），读出的是**全 session 所有分支**的事件。但 `compact` 的入参 `branch_id` 同时用作 recovery branch 的 parent（[engine.rs:112](crates/compaction-engine/src/engine.rs:112)）与命名，`replaced_range` 也据此计算。多分支会话下被折叠的事件集合与 `branch_id` 不对应，recovery branch 的 fork 点也未必在目标分支上。当前因无消费者未触发，但这是多分支压缩的正确性隐患。**建议**：压缩读取改用 `events_by_branch(session_id, branch_id, …)`，或为 `replay_events` 增加 branch 过滤重载。

#### V2 [正确性·中] Export/Import 多分支往返丢失事件→分支归属

[import_session](crates/session-store/src/export_import.rs:168) 在重建分支树后，对导出的**全部事件**统一执行 `append_event(export.active_branch.clone(), event.clone())`（[export_import.rs:201](crates/session-store/src/export_import.rs:201)）。导出侧 `export_session` 是跨分支读取事件（按 sequence 升序），因此非 active 分支的事件在导入后全部被写入 active branch，`session_events.branch_id` 与导出前不一致。「往返等价」验收在多分支下不成立；现有 `export_round_trips_through_json_and_import` 仅构造单分支会话，故未暴露。**建议**：导入时按事件原始 `branch_id`（需在导出 schema 中携带每事件的 branch，或按分支分组重建）分派；并补一个多分支往返测试。

#### V3 [正确性·中] Pi 导入器未知字段收集互相覆盖

[pi_import.rs:412](crates/session-store/src/pi_import.rs:412) 的未知字段收集循环执行 `report.unknown_entries.insert(0, format!("{}={}", k, v))`，key 恒为 `0`（BTreeMap），多条未知记录互相覆盖，最终只保留最后一条。与 P5-9 验收项「保存未知字段」相悖；测试 `parse_recognizes_known_kinds_and_preserves_unknown_fields` 只检查单条 `unknown_fields`，且 `import_pi_*` 测试从未断言 `report.unknown_entries` 内容，故未捕获。**建议**：key 改用行号或递增序号；补多条未知记录的导入断言。

#### V4 [正确性·低] ModelSwitch 只计数不持久化

[pi_import.rs:369-370](crates/session-store/src/pi_import.rs:369) 对 `PiPayload::ModelSwitch` 仅 `report.imported_model_switches += 1`，不追加任何事件，模型切换信息未真正还原进会话（plan「还原会话结构」目标部分落空）。根因是 `AgentEvent` 无对应变体。**建议**：若需保留，扩一个 `ModelSwitched` 事件或在 message metadata 中标注；否则在报告里明确「未持久化」。

#### V5 [阻塞异步·中] Pi 导入同步读取整个文件

[import_pi_jsonl](crates/session-store/src/pi_import.rs:270) 用 `std::fs::read_to_string` 一次性读入全部内容，再走 `import_pi_jsonl_lines`。该调用位于 async 方法内、且其后所有 DB 写入都经 Actor 异步化，唯独文件读取是同步阻塞；大 Pi 文件（历史长会话）会阻塞 runtime 工作线程，且无大小上限（内存压力）。**建议**：改 `tokio::fs::read_to_string` 或 `spawn_blocking`，并对超大文件改逐行流式读取（与基线「JSONL 流式解析」语义一致）。

#### V6 [估算偏差·中] 启发式 token 估算对 CJK 严重低估

[HeuristicEstimator](crates/context-engine/src/token.rs:167) 默认 `chars_per_token = 4`（[token.rs:188](crates/context-engine/src/token.rs:188) `chars.div_ceil(chars_per_token)`），压缩引擎的 [estimate_text_tokens](crates/compaction-engine/src/engine.rs:160) 同样是 `chars().count() / 4`。中文字符约 1–2 token/字，按 4 字/token 估算会把 token 数低估约 4–6 倍 → 预算/压缩触发判定偏乐观，非 OpenAI 模型有上下文溢出风险。tiktoken 路径（OpenAI 系）BPE 正确，不受影响。**建议**：对启发式路径按脚本（CJK/拉丁）分流设 ratio，或保守取 `chars_per_token ≈ 1.5`；压缩统计的 `estimate_text_tokens` 复用 `TokenEstimator` 而非硬编码 /4。

#### V7 [搜索精度·低] 内容搜索命中原始 JSON

[search.rs:205](crates/session-store/src/search.rs:205) 内容匹配用 `m.message_json LIKE ?1`，会对 `message_json` 的字段名/`role`/`metadata` 等结构噪声误命中（如搜 "content"/"role" 命中所有消息），且 snippet 是 `substr(m.message_json,1,120)`（[search.rs:203](crates/session-store/src/search.rs:203)）即原始 JSON 片段，可读性差。模块注释已说明暂未用 FTS5 的理由（sessions 主键为 TEXT、无整数 rowid）。**建议**：内容匹配改为抽取 `Text` 部分后再 LIKE（或在 projection 里冗余一份纯文本列），snippet 同源；迁移到整数 rowid 后再上 FTS5。

#### V8 [正确性·低] replay/tail 不感知分支

[replay_events](crates/session-store/src/event_store.rs:229) 与 [tail_events](crates/session-store/src/event_store.rs:257) 查询均不带 `branch_id`，会混排多分支事件。P5-1 已新增分支感知的 `events_by_branch`，但旧的 session 级重放 API 未收敛，调用方易误用（V1 即为其下游表现）。**建议**：明确 replay/tail 的「整 session」语义并文档化，或提供分支感知重载，避免上下文重建误混分支。

#### V9 [健壮性·低] 死错误变体

[SessionStoreError::EventSessionMismatch](crates/session-store/src/lib.rs:130) 在全仓库无任何构造点（rg 仅命中声明本身），属死代码。**建议**：移除或补上 append 路径的 session 一致性校验并配测试。

#### V10 [正确性·低] Tool Result 裁剪不计二进制内容

[byte_len_of_tool_result](crates/context-engine/src/tool_result_trim.rs:111) 对 `Image` 等非文本 part 以 `0` 计（[tool_result_trim.rs:118](crates/context-engine/src/tool_result_trim.rs:118)），故「图片为主、文本很少」的结果会被判为 `Small` 而原样进入上下文，与「超大输出不无限进入上下文」验收项在二进制场景下存在缺口。注释说明二进制由调用方在写 Blob 时另行管理，但裁剪入口本身未设防。**建议**：在分类时对二进制 part 也给一个估算权重（如按 base64 长度或固定成本），或由调用方在传入前先把二进制转 Artifact。

### 6. 优化建议（按优先级）

#### P0（多分支能力正式上线前处理）

1. **V1 + V8**：压缩读取改 `events_by_branch`，并收敛 replay/tail 的分支语义——这是「分支即一等公民」能否成立的关键，当前是潜伏正确性 bug。
2. **V2**：导入按原始 branch 分派事件并补多分支往返测试，否则 export/import 不能作为可信迁移/备份通道。

#### P1（近期排期）

3. **V3**：Pi 未知字段收集 key 改行号（一行改动）+ 断言。
4. **V5**：Pi 导入改异步/流式读取，与基线「JSONL 流式解析」对齐。
5. **接线补齐**：把 `trim_tool_result` 接入 `ContextBuilder`（或 agent-engine 调用点）、把 `CompactionEngine` 接入 agent loop 的超限处理路径——当前两个 crate 零消费者，属「已实现未集成」，需在对应阶段补端到端验证。
6. **V6**：启发式 token 估算针对 CJK 调参，压缩统计复用 `TokenEstimator`。

#### P2（顺手/评估项）

7. **V4**：明确 ModelSwitch 的持久化策略（新事件 or metadata）。
8. **V7**：内容搜索改按抽取文本匹配，snippet 同源；评估 FTS5 迁移窗口。
9. **V9**：移除死变体 `EventSessionMismatch`，或补 session 一致性校验。
10. **V10**：Tool Result 裁剪对二进制 part 给估算权重。
11. **文档同步**：[context-engine/compaction.rs](crates/context-engine/src/compaction.rs) 注释仍写「压缩引擎位于 compaction-engine（尚未实现）」，已过时；`context-engine::CompactionReason` 与 `compaction-engine::CompactionReason` 同名异构（后者多一个 `Manual`），建议统一或明确映射，避免调用方混淆。

### 7. 建议的后续动作（本次未执行，供研究）

1. 对 V1/V2 立项（多分支正确性，影响压缩与迁移两条主线）。
2. V3/V5 作为 Pi 导入器的小修复合并提交。
3. 评估 `compaction-engine` / `context-engine` 接入 `agent-engine` 的时机与端到端测试方案。
4. 中文 token 启发式调参（V6）作为 Provider 接线（Phase 6）的前置项。

---

*评审方法：以 `67d6c4d` 为基线，逐项核对 ROADMAP/plan 状态、源码与依赖清单，并复跑 3 个 Phase-5 crate 的测试与静态门禁；文中所有结论均给出文件与行号级证据。本文档仅为评审记录，不代表已批准的变更，未修改任何代码/配置。*


---

## 6. Phase 6（P6）— OpenAI / Anthropic / Google 三家 Provider 适配

- **日期**：2026-08-08
- **评审基线**：`main` @ `67d6c4d`（工作树仅含未跟踪的 REVIEW 文档，无代码改动）
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置）
- **范围**：ROADMAP.md Phase 6 的 9 个任务（P6-1 ~ P6-9）的完成情况、所引入包是否合适、是否存在更优替代或自实现替换的必要；附基线偏差、漏洞与优化点。

### 1. 结论摘要

1. **三大 Provider 适配质量可信**：provider-openai / provider-anthropic / provider-google 各自通过统一 Contract Tests（10 / 13 / 14 项，全程 wiremock 不触网），覆盖文本流、单/并行 tool call、usage+stop、cancel、429 限流、流中断归一。P6-1/2/3 🟢 属实。
2. **跨切能力（P6-5/6/7/9）落实良好**：Thinking / Image / Prompt Cache / provider_options 的 canonical 表达落在 provider-api，三家适配器各自正确映射；cache token（Anthropic `cache_read_input_tokens` / `cache_creation_input_tokens`、Gemini `cachedContentTokenCount`）已归一到 usage。**ADR-002 解耦红线成立**：`rg` 全量扫描 agent-engine / context-engine / agent-domain 无 provider 名特例分支（仅 context-engine 按 model 名选 tiktoken/启发式估算器，与基线一致）。
3. **两个完成度存疑项**：
   - **P6-8 结构化输出对 Anthropic 是空操作**：[request.rs:109-114](crates/provider-anthropic/src/request.rs) 的 `ResponseFormat::Json | JsonSchema` 分支只有注释、无任何指令注入或 schema 透传，schema 被静默丢弃；与 OpenAI（`json_schema`）/ Google（`responseSchema`）形成行为不对称。
   - **P6-4 OAuth 为「库已完成、零接线」**：PKCE / Device Flow / refresh / callback primitives 与脱敏红线都到位，但 `needs_refresh` / `refresh_access_token` / `store_oauth_token` / `resolve_oauth_credential` 在 auth-service 之外**无任何消费者**，「auto refresh」未进入请求路径，刷新后轮换的 refresh token 也无处回写。任务标 🟢 与实际集成状态有偏差。
4. **基线偏差集中在 OAuth**：workspace 基线 `oauth2 = "5"`（[Cargo.toml:96](Cargo.toml)）**全仓库零引用**——实现选择了手写而非基线声明的 `oauth2` crate；同时手写引入的 `base64` / `rand` / `sha2` / `url`（[auth-service/Cargo.toml:14-22](crates/auth-service/Cargo.toml)）未回填基线。需对「oauth2 基线去留」做一次明确决策。
5. **三个应处理的风险**：(V1) Google 把 API key 放进 URL query 而非 `x-goog-api-key` 头；(V2) Anthropic thinking 默认 budget（High=8192）大于默认 max_tokens（4096），真实 API 会 400 拒绝，且被 mock 测试漏过；(V4) OAuth 刷新令牌轮换未持久化。

### 2. P6 任务完成情况核对表

| 任务 | 交付 crate | 状态 | 关键证据 |
| --- | --- | --- | --- |
| P6-1 OpenAI 适配 | `provider-openai`（复用 `provider-openai-compatible`） | 🟢 | [provider.rs](crates/provider-openai/src/provider.rs)：OpenAI 协议即 Chat Completions，复用兼容引擎；contract 10 项全过 |
| P6-2 Anthropic 适配 | `provider-anthropic` | 🟢 | [request.rs](crates/provider-anthropic/src/request.rs) + [stream.rs](crates/provider-anthropic/src/stream.rs)；contract 13 项全过 |
| P6-3 Google Gemini 适配 | `provider-google` | 🟢 | [request.rs](crates/provider-google/src/request.rs) + [stream.rs](crates/provider-google/src/stream.rs)；contract 14 项全过 |
| P6-4 OAuth | `auth-service`（oauth.rs） | 🟡 | PKCE/Device/refresh/callback 均实现且测试通过，但**无外部消费者**，auto-refresh 未接线 |
| P6-5 Thinking / Reasoning | `provider-api` + 三家 | 🟢 | canonical `ThinkingConfig`/`ThinkingLevel`（[provider-api/lib.rs](crates/provider-api/src/lib.rs)）；Anthropic budget / OpenAI `reasoning_effort` / Gemini `thinkingBudget` 各自映射 |
| P6-6 图片输入 | `agent-domain` + 三家 | 🟢 | `ImageContent`/`ImageSource`（[message.rs:43-57](crates/agent-domain/src/message.rs)）；三家 image block 映射均有 contract 覆盖 |
| P6-7 Prompt Cache | `provider-api` + Anthropic/OpenAI | 🟢 | `PromptCachePreference`；Anthropic `cache_control` 标记（[request.rs:44](crates/provider-anthropic/src/request.rs)、[request.rs:192-195](crates/provider-anthropic/src/request.rs)）；cache token 归一到 usage（[stream.rs:175-185](crates/provider-anthropic/src/stream.rs)） |
| P6-8 结构化输出 | `provider-api` + 三家 | 🟡 | OpenAI `json_schema`（[request.rs:75-85](crates/provider-openai-compatible/src/request.rs)）、Google `responseSchema`（[request.rs:96-100](crates/provider-google/src/request.rs)）OK；**Anthropic 静默丢弃**（[request.rs:109-114](crates/provider-anthropic/src/request.rs)） |
| P6-9 Provider-specific options | `provider-api` + 三家 | 🟢 | `provider_options: BTreeMap` 透传；agent core 无 provider 名分支（见 §3.3） |

**门禁证据（2026-08-08 复核）**：

- `cargo fmt --all -- --check`：干净（exit 0）。
- `cargo clippy -p provider-api -p provider-runtime -p provider-openai-compatible -p provider-openai -p provider-anthropic -p provider-google -p auth-service -p model-registry -p agent-domain --all-targets -- -D warnings`：**Finished，无告警**。
- `cargo test`（上述 9 crate）：**187 passed / 0 failed**。Phase-6 自有 crate 合计 94 项（provider-openai 2+10、provider-anthropic 20+13、provider-google 8+14、auth-service 27）；共享层 provider-runtime 54、provider-openai-compatible 12+10、provider-api 4、model-registry 10、agent-domain 3。

### 3. 包选型评估

#### 3.1 建议保留（自实现不值得）

| 包 | 版本 | 使用点 | 评估 | 结论 |
| --- | --- | --- | --- | --- |
| `reqwest`（rustls+stream） | 0.12 | 三家 provider HTTP 底座 | Provider 流式与 list_models 的唯一网络层，feature 子集精确 | **保留** |
| `serde` / `serde_json` | 1 | 全部请求/响应编解码 | 基础设施 | **保留** |
| `tokio` / `futures` / `bytes` | 1 / 0.3 / 1 | 流式组装、回调服务器、取消竞争 | 异步与字节流核心 | **保留** |
| `thiserror` | 2 | `ProviderError` / `AuthError` | 库错误类型分工 | **保留** |
| `async-trait` | 0.1 | `ModelProvider` / `ProviderEventSink` | 稳定 Rust 对象安全异步接口 | **保留** |
| `keyring` | 3 | `KeychainBackend`（[backend.rs](crates/auth-service/src/backend.rs)） | OS Keychain 绑定，Secret 不落库红线依赖 | **保留** |
| `backon` | 1 | provider-runtime 重试退避 | 退避策略完整 | **保留** |
| `wiremock` / `proptest` | 0.6 / 1 | contract 与 fuzz 测试 | 三家 provider 契约套件基座 | **保留** |

#### 3.2 需要重新评估的项

| 项 | 现状 | 选项 | 建议 |
| --- | --- | --- | --- |
| `oauth2 = "5"` | 基线声明（[Cargo.toml:96](Cargo.toml)）且 plan P6-4 写明「基于 oauth2 crate 实现 PKCE / refresh」，但实现**手写**（[oauth.rs:6](crates/auth-service/src/oauth.rs) 注释「不引入整套 oauth2 SDK」），全仓库零引用 | a) 采纳 oauth2 crate 重写 token 交换/PKCE 原语，手写层只留 Device Flow 编排；b) 维持手写，**更新基线与 plan 说明自实现理由并移除 oauth2** | **建议 b**。手写质量合格（PKCE S256 经 RFC 7636 测试向量验证、state CSRF 校验、错误归一不含 token、Secret 红线到位）；oauth2 crate 在「PKCE + refresh」子集上的增量价值不足以抵消重写+回归成本。但必须补齐 §3.3 所列缺口（refresh 轮换回写、auto-refresh 接线）并同步基线文档 |
| `base64` 0.22 / `rand` 0.8 / `sha2` 0.10 / `url` 2 | auth-service 手写 OAuth 引入（[auth-service/Cargo.toml:14-22](crates/auth-service/Cargo.toml)），**均未登记基线** | 回填基线或改用已有等价物 | **回填**。`base64`/`sha2`/`rand` 是加密/编码自实现高风险区，采用成熟 crate 正确；`url` 已是 `reqwest` 间接依赖，直接引用合理。一并写入基线「直接采用」表并标注 P6-4 |
| Anthropic `response_format` 处理 | 无原生 response_format，当前空实现（V3） | a) 注入 system 指令 + 工具约束 schema；b) 显式返回不支持错误；c) 透传到 provider_options 让上层决策 | **建议 a 或 c**，至少不能静默丢弃（见 V3） |

#### 3.3 「自实现替换包」总体判断

Phase 6 范围内**没有发现应被自实现替换的已引包**——reqwest/serde/keyring 等使用面都覆盖核心价值区。唯一需要决策的是反向问题：**oauth2 crate 基线虚置**。手写 OAuth 在「PKCE + token 交换 + Device Flow」子集上是基线「参考 + 自实现」表的合理延伸（与 SSE 自实现同源），保留手写可行，但需补三个缺口：

1. **refresh token 轮换回写**：`refresh_access_token`（[oauth.rs:307](crates/auth-service/src/oauth.rs)）返回的 `TokenSet` 可能携带**新** refresh_token（部分 Provider 每次刷新轮换），但无 `update_oauth_token` 之类函数把它写回 backend；旧 refresh token 失效后用户被迫重新授权。
2. **auto-refresh 编排缺失**：`needs_refresh` + `refresh_access_token` 只是原语，没有任何调用方在发请求前检查并刷新（见 §2 P6-4、§5 V4）。
3. **PKCE verifier 取模偏差**（[oauth.rs:86](crates/auth-service/src/oauth.rs)）：`UNRESERVED[(*b % 66)]` 对 66 字符表有轻微偏差（256 非 66 整数倍）。PKCE verifier 只需高熵不可猜，64 字符 ≈ 390 bit，偏差不构成可利用风险，但若决定长期手写，建议改用拒绝采样或直接 base64url(random 48 bytes)。

### 4. 基线偏差清单

规则来源：ROADMAP「依赖选型基线」要求新增依赖同步回填、声明须被引用。

| 类型 | 项 | 位置 | 说明 |
| --- | --- | --- | --- |
| 声明未引用 | `oauth2 = "5"` | [Cargo.toml:96](Cargo.toml) | P6-4 改手写，零引用（见 §3.2） |
| 引入未登记 | `base64 = "0.22"` | [auth-service/Cargo.toml:14](crates/auth-service/Cargo.toml) | OAuth 手写引入 |
| 引入未登记 | `rand = "0.8"` | [auth-service/Cargo.toml:15](crates/auth-service/Cargo.toml) | 同上 |
| 引入未登记 | `sha2 = "0.10"` | [auth-service/Cargo.toml:18](crates/auth-service/Cargo.toml) | PKCE S256 |
| 引入未登记 | `url = "2"` | [auth-service/Cargo.toml:22](crates/auth-service/Cargo.toml) | 授权 URL 构造 |

> 附注（非本阶段新增，已在 REVIEW.md 记录）：`uuid`、`tracing-appender`、`anyhow`、`similar` 仍为声明未引用；`rmcp`/`wasmtime`/`wit-bindgen`/`landlock`/`windows`/`windows-service`/`portable-pty`/`ed25519-dalek` 属未来 Phase（9/10/11）的预声明，可在对应阶段开工时再评估是否提前引用。

**建议**：一次小型清理——按 §3.2 决策 oauth2 去留（倾向移除并补文档），回填 base64/rand/sha2/url 四项，同步 ROADMAP 基线表。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V8）。

#### V1 [安全·中] Google API key 写入 URL query

[provider.rs:92-97](crates/provider-google/src/provider.rs) 把 secret 拼成 `?alt=sse&key=<secret>`，且该请求不附任何认证头（[provider.rs:112](crates/provider-google/src/provider.rs) 传 `&[]`）。query 参数会进入：代理访问日志（`HttpClientConfig.proxy` 启用时）、Google 服务端日志、潜在的重定向目标、以及任何诊断/抓包。HTTP 运行时本身不记录 URL（[http.rs](crates/provider-runtime/src/http.rs) 仅把 url 作参数、无 tracing 宏输出），但「key 在 URL」是 Google 已明确不推荐的旧式做法。**建议**：改用 `x-goog-api-key: <secret>` 请求头（Google 现行推荐），key 从 URL 移除；这是少量改动且与 Anthropic/OpenAI 的「头携带 secret」模式一致。

#### V2 [正确性·中] Anthropic thinking budget 与 max_tokens 默认冲突

[request.rs:16](crates/provider-anthropic/src/request.rs) `max_tokens = max_output_tokens.unwrap_or(4096)`，而 [request.rs:216-225](crates/provider-anthropic/src/request.rs) 的 `thinking_budget` 默认 Low=1024 / Medium=4096 / High=8192。Anthropic 扩展思考要求 `thinking.budget_tokens < max_tokens`，因此默认 max（4096）+ High（8192）或 Medium（4096，等于非小于）真实请求会被 API 以 400 拒绝。现有 mock 测试不触网故未暴露（`thinking_maps_to_budget` 用例 budget=8192、max=128 仍断言通过）。**建议**：构造请求体时将 `budget_tokens` 钳制为 `< max_tokens`（留余量），并对「未显式设 max_output_tokens 但开 thinking」补一条默认提升或告警。

#### V3 [正确性/功能·中] Anthropic 结构化输出静默丢弃

[request.rs:109-114](crates/provider-anthropic/src/request.rs) 的 `ResponseFormat::Json | JsonSchema` 分支仅有注释（「退化为 system 指令」），实际**无任何动作**——既不注入 schema 指令，也不透传到 body，更不报错。用户请求 JsonSchema 时 schema 与 name 被完全丢弃，与 OpenAI（`response_format: json_schema`）/Google（`responseSchema`）行为不对称，且 P6-8 验收「可要求并校验 JSON 结构化输出」对 Anthropic 实际未达成。**建议**：至少注入一条 system/tool 约束把 schema 喂给模型，或在 `ModelCapabilities` 标注 Anthropic 此模型不支持后由上层回退；不应静默。

#### V4 [功能完整性·中] OAuth auto-refresh 未接线 + refresh 轮换不回写

`needs_refresh` / `refresh_access_token` / `store_oauth_token` / `resolve_oauth_credential` 在 auth-service 之外**零消费者**（`rg` 全仓确认）。P6-4 步骤 2「auto refresh — token 自动续期」只交付了原语，没有在任何请求路径前置刷新检查；`refresh_access_token`（[oauth.rs:307](crates/auth-service/src/oauth.rs)）可能返回轮换的新 refresh token，但无函数将其回写 backend，轮换型 Provider 会在下一次刷新失败。**建议**：在 provider 构造/请求前置处接入「检查 `needs_refresh` → 刷新 → 回写 access/refresh → 更新 `expires_at`」的编排（可放 app-service，Phase 13 顺手完成），并补 `update_oauth_token` 写回函数与对应测试。

#### V5 [健壮性·低] Anthropic cache_control 标注每条 user 消息

`cache_enabled` 默认为真（`PromptCachePreference::Automatic`），[request.rs:192-195](crates/provider-anthropic/src/request.rs) 在 `message_to_anthropic`（逐条消息调用）内对**每条** role=user 消息的末 block 加 `cache_control`。多轮长对话会累积远超 Anthropic 缓存断点上限的标记 → 触发 400。**建议**：仅在「可缓存前缀」的稳定边界（system、首个稳定 user turn、工具定义末尾）标记，或受断点计数约束；参考 Anthropic 当前断点上限动态钳制。

#### V6 [健壮性·低] OAuth 回调服务器单次读取 + redirect_uri 未绑定监听

[oauth.rs](crates/auth-service/src/oauth.rs) `CallbackServer`：`handle_callback_connection` 只做一次 `read(&mut [0u8;4096])`，浏览器回调若分片或携带大 cookie 可能解析不全；`PkceFlowConfig.redirect_uri` 是独立字符串，未与 `local_addr()` 校验一致，配置错误时仍生成指向错误地址的授权 URL。**建议**：循环读到请求头结束或限长后解析；`start()` 用实际绑定端口回填 redirect_uri 或校验一致。

#### V7 [安全·低] PKCE verifier 取模偏差

[oauth.rs:86](crates/auth-service/src/oauth.rs) `UNRESERVED[(*b % 66)]`，66 非 256 因数，存在轻微偏差。不降低实际熵（64×log2(66)≈390 bit），无可利用性，但若长期手写建议改拒绝采样或 `base64url(rand 48B)`，并加一条均匀性属性测试。

#### V8 [健壮性·低] Gemini 工具调用 id 为合成序号

[stream.rs](crates/provider-google/src/stream.rs) `chunk_to_events` 用 `call-{tool_counter}`（`call-0`/`call-1`…）作为 ToolCallId，因 Gemini 不在响应中返回调用 id。后续 tool result 回填只能靠顺序/名称匹配，多工具并发或重放场景下 id 不稳定。**建议**：在 `ModelResponseSummary.provider_metadata` 或 ToolCall 元数据中保留 Gemini 原始顺序，由上层在回写 functionResponse 时按 name 对齐，避免依赖合成 id 跨轮稳定。

### 6. 优化建议（按优先级）

#### P0（建议尽快处理）

1. **V2**：Anthropic thinking budget 钳制到 `< max_tokens` + 默认值重算；补一条触网 mock 校验（断言 `budget_tokens < max_tokens`）。
2. **V3**：Anthropic 结构化输出至少注入 schema 指令或显式不支持，消除静默丢弃。
3. **V1**：Google key 改 `x-goog-api-key` 头，移出 URL query。

#### P1（近期排期）

4. **V4**：补 OAuth auto-refresh 编排 + `update_oauth_token` 回写，并加「刷新后轮换 token 被持久化」的契约测试；明确 P6-4 验收口径（库完成 vs 端到端）。
5. **基线清理**（§4）：决策 oauth2 去留（建议移除并补自实现说明）、回填 base64/rand/sha2/url、同步 ROADMAP 基线表。
6. **V5**：收敛 Anthropic `cache_control` 标注点，受断点上限约束。

#### P2（顺手/评估项）

7. **V6/V7/V8**：回调服务器读取与 redirect_uri 绑定；PKCE 均匀性；Gemini 工具 id 稳定性。
8. **内置模型目录新鲜度**：三家 `builtin_models()` 硬编码（OpenAI 含 o1/gpt-4o、Anthropic 仅 claude-3.5/3、Gemini 至 2.5）。模型迭代快，建议把目录外置为可更新数据或补一个远端 `/models`（带能力探测）的渐进路径，避免目录与线上脱节。
9. **list_models 全静态**：三家均返回内置目录、`models_url()` 标 `#[allow(dead_code)]`（Anthropic）。评估是否提供「远端目录 + 能力推断」开关，至少用于发现新模型。
10. **provider_options 语义统一**：Anthropic 与 OpenAI 把 `provider_options` 合并到顶层、Google 合并到 `generationConfig`（各自正确），但「同名覆盖 canonical」的语义只在 OpenAI 注释中写明（[request.rs](crates/provider-openai-compatible/src/request.rs)）。建议在 provider-api 文档统一声明该「覆盖」语义，避免上游误用。

### 7. 附录：相关「优先级 P1」与遗留项

| 事项 | 状态 | 说明 |
| --- | --- | --- |
| P9-7 MCP OAuth | ⚪ 未开始 | 复用本阶段 `auth-service` OAuth primitives；开工前确认 callback 服务器复用与 redirect_uri 一致性（V6） |
| agent-api 职责边界 | 遗留 | ROADMAP 遗留项；Phase 6 不涉及，Phase 13 前评估 |
| provider-bedrock / provider-mistral | 遗留 | workspace-layout 已登记但无任务；与本阶段三家原生适配同构，启动时补任务 |

### 8. 建议的后续动作（本次未执行，供研究）

1. 对 V2/V3/V1 立项（正确性 + 安全优先，改动面集中在三个 provider crate）。
2. V4 的 OAuth 接线方案讨论（落点在 app-service 还是 provider 构造），并据此最终判定 P6-4 验收。
3. 基线清理小任务（§4），一次提交完成。
4. 决定 oauth2 crate 去留（建议移除 + 文档化自实现理由 + 补 §3.3 三缺口）。
5. 内置模型目录的更新机制评估（§6 P2.8）。

---

*评审方法：以 `67d6c4d` 为基线，逐项核对 ROADMAP/plan 状态、源码与依赖清单，并复跑 9 个 Phase-6 相关 crate 的测试与静态门禁；ADR-002 解耦红线经全仓 `rg` 验证。文中所有结论均给出文件与行号级证据。本文档仅为评审记录，不代表已批准的变更。*


---

## 7. Phase 7（P7）— Git、Diff 与 Worktree

- **日期**：2026-08-08
- **评审基线**：`main` @ `67d6c4d`（HEAD）；工作树含用户未提交的 docs/ROADMAP/plan 改动与本评审产物，均不影响 Phase 7 代码结论
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置；后续再研究是否采纳）
- **范围**：ROADMAP.md Phase 7 的 8 个任务（P7-1 ~ P7-8，主题「Git、Diff 与 Worktree」）的完成情况、所引入包是否合适、是否存在更优替代或自实现替换的必要；另含「优先级 P1」标签任务（P7-7/P7-8）现状。安全漏洞与优化点一并列出。

### 1. 结论摘要

1. **完成度可信**：P7-1 ~ P7-8 全部 🟢。2026-08-08 复跑 `git-service`（51 项）+ `diff-service`（21 项）共 **72 项测试全部通过**（均为真实 git 仓库集成测试）；`cargo clippy --all-targets -- -D warnings` 与 `cargo fmt --check`（两 crate）干净。
2. **包选型总体合理**：实际引用面落在 `notify` / `notify-debouncer-full`（P7-6 watcher）、`parking_lot`（缓存锁）、`tempfile`（测试 + stage patch 临时文件）上，使用面都覆盖其核心价值，**不建议自实现替换**。但「直接采用」表把 `notify-debouncer-full` 归到 P1-8，真实使用者是 P7-6 的 git 缓存失效器，归属应补 P7-6。
3. **`similar` 仍为「声明未引用」**：P7-3 与基线原计划用 `similar` 做 word-level diff，但 `diff-service` 实际解析 git 结构化输出（`--raw`/`--numstat`/unified patch），全仓库零真实引用（仅 [parser.rs:8](crates/diff-service/src/parser.rs) 注释出现 `similarity` 字样）。`docs/features/git-diff.md:44` 却把「word-level diff / Ignore whitespace / Hunk discard / 内容指纹」列为能力——文档承诺与实现存在缺口。
4. **基线偏差**：`similar`（声明未引用）、`parking_lot`/`tempfile`（Phase 7 引入但未回填 workspace 基线，REVIEW.md §4 已点名，仍未处理）持续存在；**新增** `diff-service` 把 `serde_json`、`thiserror` 声明为直接依赖却零使用。
5. **两个应优先处理的安全点**：(a) `apply_patch_to_index` 用可预测路径（`pawork-hunk-stage-{pid}-{counter}.patch`）在系统 temp 目录写 patch，多用户/共享主机下存在符号链接竞争与源码外泄面；(b) `history`/`branch` 的 `rev`/`range`/`name`/`start_point` 作为位置参数直传 git，未防「以 `-` 开头」的选项注入。
6. **一个语义缺口**：`CacheScope::Staged` 未实现——`refresh` 用 `let _ = scope;` 忽略 scope，`Staged` 实际返回与 `Worktree` 完全相同的全量视图，API 具误导性。

### 2. P7 任务完成情况核对表

| 任务 | 交付模块 | 状态 | 关键证据 |
| --- | --- | --- | --- |
| P7-1 Repo 检测 / branch / HEAD | `git-service::repo` + `process` | 🟢 | [repo.rs](crates/git-service/src/repo.rs)：`open`/`current_head`/`repo_info`；错误归一 [error.rs:45-58](crates/git-service/src/error.rs)；非仓库 → `NotARepository` |
| P7-2 status / changed files | `git-service::status` | 🟢 | [status.rs:84-96](crates/git-service/src/status.rs)：`--porcelain=v1 -z`，解析 rename `previous_path`；`changed_files` 剔除未跟踪 |
| P7-3 结构化 Diff | `diff-service` | 🟢 | [service.rs](crates/diff-service/src/service.rs) `diff_summary`/`diff`；rename/binary/无末尾换行测试；100k 行解析基准 < 500ms（[parser.rs:214-231](crates/diff-service/src/parser.rs)） |
| P7-4 stage / unstage / discard | `git-service::stage` | 🟢 | [stage.rs:65-108](crates/git-service/src/stage.rs)；discard 标 `Dangerous` 供审批 |
| P7-5 Worktree | `git-service::worktree` | 🟢 | [worktree.rs](crates/git-service/src/worktree.rs)；`remove` 先 `list()` 校验受管理，删除只交 `git worktree remove`，**绝不** `std::fs` 递归删——红线遵守 |
| P7-6 Git 缓存 / watcher | `git-service::cache` | 🟢 | [cache.rs](crates/git-service/src/cache.rs)：`StatusCache`(parking_lot RwLock) + `CachedStatusService` + notify-debouncer 失效；缓存命中 1000 次 < 50ms 测试 |
| P7-7 Hunk / Line stage（P1） | `diff-service::hunk_stage` | 🟢 | [hunk_stage.rs](crates/diff-service/src/hunk_stage.rs)：`build_hunk_patch`/`build_line_patch` + `git apply --cached [--reverse]`，hunk/line 级 stage/unstage 真实 git 测试 |
| P7-8 commit/branch/stash/log/show（P1） | `git-service::{commit,branch,stash,history,conflict}` | 🟢 | 51 项真实 git 测试含 conflict/merge-base/未合并检测；plan 验收勾选 |

**门禁证据（2026-08-08 复核）**：

- `cargo test -p git-service -p diff-service`：**git-service 51 passed / diff-service 21 passed / 0 failed**（含真实 git 仓库集成与 100k 行 diff 基准）。
- `cargo clippy -p git-service -p diff-service --all-targets -- -D warnings`：干净。
- `cargo fmt -p git-service -p diff-service -- --check`：干净。
- 各 plan 验收项：P7-7/P7-8 已勾选；P7-1~P7-6 验收点（解析稳定、不删用户数据、切换 < 50ms、100k 行 < 500ms）均有对应测试。

### 3. 包选型评估

#### 3.1 建议保留（自实现不值得）

| 包 | 版本 | 使用点 | 使用面评估 | 结论 |
| --- | --- | --- | --- | --- |
| `notify` | 7 | P7-6 [cache.rs:151-173](crates/git-service/src/cache.rs) | 跨平台文件监听，缓存失效核心；自实现 ReadDirectoryChangesW/inotify/FSEvents 成本高 | **保留** |
| `notify-debouncer-full` | 0.5 | P7-6 [cache.rs:137-162](crates/git-service/src/cache.rs) | 300ms 去抖 + RecommendedCache 事件合并，大 checkout 事件风暴下显著降噪 | **保留**；建议把基线归属补 P7-6 |
| `parking_lot` | 0.12 | P7-6 [cache.rs:45](crates/git-service/src/cache.rs) `RwLock<HashMap>` | 无毒化锁，命中路径纯内存读，满足 < 50ms | **保留**；但需回填 workspace 基线（见 §4） |
| `tempfile` | 3 | P7-3/4/5/7/8 测试 + P7-4 stage patch 临时文件 | 真实 git 隔离仓库与确定性内容断言基座 | **保留**；dev 已用，runtime 见 V1 |
| `serde` / `thiserror` / `tokio` / `tracing` | 基线 | 全局 | 基础设施，无争议 | **保留** |

#### 3.2 需要重新评估的项

| 项 | 现状 | 选项 | 建议 |
| --- | --- | --- | --- |
| `similar` | 基线声明（[Cargo.toml:127](Cargo.toml)，P7-3）但全仓库**零真实引用**，仅 [parser.rs:8](crates/diff-service/src/parser.rs) 注释；word-level diff 计划未落地 | a) 移出基线，关闭文档承诺；b) 实现 word-level diff 再保留 | **建议 a**。diff-service 已用 git 结构化输出完成 hunk/line 暂存；进程内 word diff 无强需求出现前不引入（与 REVIEW.md §3.2 结论一致） |
| `serde_json`（diff-service 直接依赖） | [diff-service/Cargo.toml](crates/diff-service/Cargo.toml) 声明但**零使用**（模型仅 `#[derive(serde::Serialize/Deserialize)]`，无 `serde_json::` 调用） | 移除该 crate 的直接依赖 | **移除** |
| `thiserror`（diff-service 直接依赖） | 同上声明但**零使用**（diff-service 复用 `git_service::GitError`，未自定义错误类型） | 移除该 crate 的直接依赖 | **移除** |
| `notify-debouncer-full` 归属 | 基线记 P1-8；file-index 实际自实现去抖，真实使用者是 P7-6 git 缓存 | 在基线「关联任务」补 P7-6 | **订正基线描述**（低优先） |

#### 3.3 「自实现替换包」总体判断

P7 范围内**没有命中「应自实现替换」的包**：notify/debouncer/parking_lot/tempfile 的使用面都覆盖核心价值，自实现无收益。反向看，唯一「只用一小部分/零引用」的是 `similar`（应移出基线）与 diff-service 多余的 `serde_json`/`thiserror`（应删除）。当前自实现部分（porcelain/raw/numstat/unified 解析状态机、hunk/line patch 构造、缓存与去抖失效编排）质量高、边界正确，无需替换为第三方。

### 4. 基线偏差清单

规则来源：ROADMAP「依赖选型基线」要求新增依赖同步回填基线表。

| 类型 | 项 | 位置 | 说明 |
| --- | --- | --- | --- |
| 声明未引用 | `similar` | [Cargo.toml:127](Cargo.toml) | P7-3 计划项，word-level diff 未实现，零引用；见 §3.2 |
| 引入未登记 | `parking_lot = "0.12"` | [git-service/Cargo.toml:22](crates/git-service/Cargo.toml) | Phase 7 引入（REVIEW.md §4 已点名，未处理） |
| 引入未登记 | `tempfile = "3"` | [git-service/Cargo.toml:26](crates/git-service/Cargo.toml)（dev）、[diff-service/Cargo.toml:20](crates/diff-service/Cargo.toml)（dev） | dev 依赖也应登记 |
| crate 多余直接依赖 | `serde_json`、`thiserror` | [diff-service/Cargo.toml](crates/diff-service/Cargo.toml) | 零使用，应从该 crate 移除 |
| 基线归属 | `notify-debouncer-full` | [Cargo.toml:108](Cargo.toml) | 基线记 P1-8，真实首用为 P7-6，建议补关联任务 |

**建议**：并入 REVIEW.md §4 的「基线清理小任务」一次性处理（删除 `similar`、回填 `parking_lot`/`tempfile`、移除 diff-service 两个多余依赖、订正 debouncer 归属）。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V10）。

#### V1 [安全·中] hunk stage 用可预测临时文件写 patch（符号链接竞争 / 源码外泄面）

[stage.rs:133-143](crates/git-service/src/stage.rs) 把 patch 写入 [stage.rs:175-183](crates/git-service/src/stage.rs) 生成的 `std::env::temp_dir().join("pawork-hunk-stage-{pid}-{counter}.patch")`：路径名可猜（pid 可观测、counter 从 0 起），位于共享系统 temp 目录。多用户主机或共享 CI 上，攻击者可预先在该路径建符号链接 → `std::fs::write` 跟随符号链接，把 patch 内容（即模型选中的源码片段）写到攻击者指定位置（外泄），或在 write 与 `git apply` 读取之间替换为攻击者 patch（污染暂存）。**建议**：把 `tempfile` 提升为 runtime 依赖，用 `tempfile::NamedTempFile`（随机名、0600、独占创建）；写完保留句柄传给 git，用后删除。

#### V2 [安全·中] git 参数注入（位置参数未防前导 `-`）

`history`/`branch`/`diff` 把 `rev`/`range`/`name`/`start_point`/`a`/`b`/`commit_range` 作为位置参数直传 git，未校验前导 `-`：
- [history.rs:87-89](crates/git-service/src/history.rs)（`range`）、[history.rs:109-123](crates/git-service/src/history.rs)（`show` 的 `rev`）、[history.rs:153-161](crates/git-service/src/history.rs)（`merge_base` 的 `a`/`b`）
- [branch.rs:41-49](crates/git-service/src/branch.rs)（`create` 的 `name`/`start_point`）、[branch.rs:101-104](crates/git-service/src/branch.rs)（`checkout` 的 `name`）、[branch.rs:130-136](crates/git-service/src/branch.rs)（`checkout_new`）
- [service.rs:166-169](crates/diff-service/src/service.rs)（`commit_range`）

以 `-` 开头的值会被 git 解释为选项（历史上有 `--upload-pack`/`-c core.xxx`/`--output` 等参数注入 CVE 类）。这些值最终来自模型/Agent 输出（处理不可信内容时受 prompt 注入影响），并非用户终端手敲。对比之下 `stage`/`stash` 的 `paths` 与 `run_file_patch` 的 `path` 已正确用 `--` 分隔（[stage.rs:73](crates/git-service/src/stage.rs)、[stash.rs:64-66](crates/git-service/src/stash.rs)、[service.rs:161](crates/diff-service/src/service.rs)）。**建议**：在服务边界统一拒绝前导 `-` 的 rev/range/branch/start_point（或对允许的语法白名单校验），并补注入回归测试。

#### V3 [正确性·中] `CacheScope::Staged` 语义未实现

[cache.rs:118-131](crates/git-service/src/cache.rs) 的 `refresh` 用 `let _ = scope;` 显式忽略 `scope`，注释自承「当前 status 解析器统一返回 staged+unstaged+untracked 视角；scope 仅影响缓存槽位区分」。结果 `CacheScope::Staged` 与 `CacheScope::Worktree` 返回**完全相同**的全量视图，调用方若按字面理解为「仅暂存区」会被误导。**建议**：要么实现 staged-only 过滤（`git diff --cached` 视图），要么把 `Staged` 变体移除/重命名并在文档写明语义，避免 API 谎言。

#### V4 [性能/正确性·中] watcher 全量递归监听 + `.git` 路径假设

[cache.rs:165-173](crates/git-service/src/cache.rs) 对 `work_dir` 递归监听（`RecursiveMode::Recursive`）并附加 `work_dir.join(".git")`。两点问题：(1) 大仓库（`node_modules`/构建产物）下递归监听事件量极大，即便 debouncer 去抖仍是高开销，且**未像 file-index 那样接 ignore 规则**过滤；(2) `work_dir.join(".git")` 假设标准布局，linked worktree（git dir 在主仓 `.git/worktrees/<name>`）或 gitfile 布局下该路径不存在/是文件，`.git` 内部变更监听失效（缓存可能不及时）。**建议**：watch 前复用 ignore 过滤；`.git` 路径改由 `git rev-parse --git-dir`（repo.rs 已有 `git_dir`）解析后监听。

#### V5 [健壮性·低] StatusCache 无界增长 + 死字段

[cache.rs:38-40](crates/git-service/src/cache.rs) 的 `computed_at: Instant` 被 `#[allow(dead_code)]` 标记、从未读取；`invalidate` 仅按 work_dir 删除、无 TTL/容量上限。长生命周期进程在多 worktree 间切换时，缓存条目只增不减。**建议**：接上 `computed_at` 做 TTL 失效或 LRU 容量上限。

#### V6 [健壮性·低] Windows verbatim 路径流入 git 子进程 cwd

`git-service`/`diff-service` 均未依赖 `dunce`；[process.rs:76-77](crates/git-service/src/process.rs) 直接把 `cwd` 设为调用方传入路径，[worktree.rs:41-43](crates/git-service/src/worktree.rs) 的 `canon` 用 `std::fs::canonicalize`（Windows 产出 `\\?\` 前缀）。当 cwd 来自 `workspace-service` 的 canonicalize 根（REVIEW.md V3）时，verbatim 路径流入 git 子进程，部分 git 版本对 `\\?\` 路径 cwd 处理不佳。属跨阶段问题，归并 P11-8 统一在出口 `dunce::simplified`。

#### V7 [健壮性·低] commit「nothing to commit」判据过宽

[commit.rs:64-71](crates/git-service/src/commit.rs) 在「stderr 不含 nothing to commit」时，以 `code == Some(1) && stderr.trim().is_empty()` 兜底归一为 `NothingToCommit`。git 其他「退出码 1 + 空 stderr」的失败会被误判为无可提交，掩盖真实错误。**建议**：收紧为仅在确认空暂存（如先 `diff --cached --quiet`）时归类，或保留 stderr 上下文返回 `GitFailed`。

#### V8 [文档/实现·低] docs/features/git-diff.md 能力清单超前于实现

[docs/features/git-diff.md:44](docs/features/git-diff.md) 列出「word-level diff / Ignore whitespace / Hunk discard / 内容指纹」等能力，但 P7-3 实现仅做 unified diff 解析 + hunk/line stage（无 word-level、无 ignore-whitespace 选项、无 hunk discard、无内容指纹）。与 `similar` 声明未引用（§4）同源。**建议**：把未实现项标注为「计划/未交付」，或从能力清单移除，避免误导下游任务。

#### V9 [健壮性·低] build_line_patch 复用 `new_no_newline` 的语义歧义

[hunk_stage.rs:201-206](crates/diff-service/src/hunk_stage.rs) 把未选中的 `Deletion` 转 context 时复用 `push_line`，其依据 `line.new_no_newline`（[hunk_stage.rs:249-256](crates/diff-service/src/hunk_stage.rs)）决定是否输出 `\ No newline`。但该标志在 [parser.rs:59-99](crates/diff-service/src/parser.rs) 中对「旧行无末尾换行」与「新行无末尾换行」均置 true，命名 `new_no_newline` 有歧义；旧侧删除行转 context 后再标记 no-newline 语义偏离。属边界情形，目前测试未覆盖。**建议**：拆分 `old_no_newline`/`new_no_newline`，或补对应回归测试锁定行为。

#### V10 [性能·低] repo_info 串行多次 git 调用

[repo.rs:155-164](crates/git-service/src/repo.rs) 的 `repo_info` 顺序触发 `current_head`（最多 2 次）+ `is_bare` + `git_dir`，共 3-4 次进程 spawn。**建议**：可用单条 `git rev-parse --show-toplevel --absolute-git-dir --is-bare-repository HEAD` + `symbolic-ref` 合并，减少往返。

### 6. 优化建议（按优先级）

#### P0（建议尽早处理）

1. **V1**：stage patch 临时文件改用 `tempfile::NamedTempFile`（安全红线相关，多用户/CI 场景有外泄面）。
2. **V3**：`CacheScope::Staged` 要么实现 staged-only 视图，要么删除/重命名（低成本消除 API 谎言）。

#### P1（近期排期）

3. **V2**：服务边界统一校验/拒绝前导 `-` 的 rev/range/branch/start_point，补注入回归测试。
4. **V4**：watcher 接 ignore 过滤 + 用 `git rev-parse --git-dir` 解析 `.git`（修正 linked worktree 监听）。
5. **基线清理**：§4 清单并入 REVIEW.md §4 的统一小任务——删 `similar`、回填 `parking_lot`/`tempfile`、移除 diff-service 多余 `serde_json`/`thiserror`、订正 `notify-debouncer-full` 归属。
6. **V8**：对齐 `docs/features/git-diff.md` 能力清单与实现。

#### P2（顺手/评估项）

7. **V5**：StatusCache 接 TTL/LRU（复用已存储的 `computed_at`）。
8. **V6**：Windows verbatim cwd 归并 P11-8 统一 `dunce::simplified`。
9. **V7**：收紧 commit「nothing to commit」判据。
10. **V9**：拆分 no-newline 标志或补回归测试。
11. **V10**：`repo_info` 合并 git 调用减少进程 spawn。

### 7. 附录：「优先级 P1」标签任务与跨阶段依赖

| 任务 | 状态 | 说明 |
| --- | --- | --- |
| P7-7 Hunk/Line stage（P1） | 🟢 已交付 | 21 项 diff-service 测试覆盖；rename/copy/typechange/unmerged/binary 已显式拒绝并回退整文件 stage（[hunk_stage.rs:133-152](crates/diff-service/src/hunk_stage.rs)） |
| P7-8 commit/branch/...（P1） | 🟢 已交付 | commit/branch/stash/log/show/merge-base/conflict 真实 git 测试齐全 |
| 与 REVIEW.md 的承接 | — | REVIEW.md §3.2/§4 点名的 `similar` 零引用、`parking_lot`/`tempfile` 未登记在 P7 依然成立，本评审补充 diff-service 多余依赖与 P7-6 debouncer 归属；REVIEW.md V3（verbatim cwd）在 P7 范围内仍未消除（见 V6） |

### 8. 建议的后续动作（本次未执行，供研究）

1. 对 V1/V2 立项（安全优先；V1 涉及源码外泄，V2 涉及 git 参数注入，二者均可能被 prompt 注入触发）。
2. 基线清理小任务（§4，并入 REVIEW.md §4 一次性提交）。
3. `CacheScope` 语义决策（实现 staged-only 或删变体）。
4. watcher ignore 过滤 + linked worktree `.git` 解析方案（影响 P7-6 大仓库体验）。
5. `docs/features/git-diff.md` 能力清单与实现对齐。

---

*评审方法：以 `67d6c4d` 为基线，逐项核对 ROADMAP/plan 状态、源码与依赖清单，并复跑 `git-service`/`diff-service` 的测试与静态门禁；文中所有结论均给出文件与行号级证据。本文档仅为评审记录，不代表已批准的变更。*

---

## 整合说明

- 本文由 `REVIEW.md`（Phase 1）与 `REVIEW-P2.md`…`REVIEW-P7.md` 合并而成；各阶段内容与原文一致，仅将标题层级下调一级以适配整合文档结构。
- 「§0 跨阶段总览」为整合新增，其余章节为各阶段原始评审内容。
- 各阶段内部 V1–Vn 编号独立；跨阶段引用时以 `P<阶段>-V<n>` 前缀区分。
- 各阶段的「优化建议（P0/P1/P2）」与「建议的后续动作」在其本章节内仍为权威源；跨阶段优先级请参考 §0.2–0.3。
- 本文仅为评审记录，不代表已批准的变更；所有结论均有文件与行号级证据，见各阶段正文。
