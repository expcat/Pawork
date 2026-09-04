# AGENTS.md — Pawork 工作指南

本文件是面向在 Pawork 仓库中工作的代理与人类协作者的工程约定。它与服务级 `AGENTS.md`（全局行为）叠加生效，冲突时以本文件为准。

## 1. 核心原则

- **事实源优先**：以当前分支、工作区差异、源码、生成物、运行日志与真实远程状态为准；历史结论只作检索线索，使用前重新验证。
- **Spec vs 源码**：[docs/spec/crates/](docs/spec/README.md) 各包 Spec 是理解包内功能的**首选读物**（目标：读文档即可了解该包全部功能，尽量少读代码），但**不是**事实源——公开 API 与行为以源码 / rustdoc / golden 为准；架构布局与冻结契约以 [docs/architecture.md](docs/architecture.md) 为准。冲突以源码为准并**同批回写 Spec**，禁止按过期 Spec 改代码。
- **按写入集加载 Spec**：进某包前读 [docs/spec/crates/](docs/spec/README.md) 该包一篇；不要一次读完 21 份。跨包链路（Agent loop / GUI 连接 / 事件持久化与重放 / 凭证与脱敏）读 [docs/spec/flows.md](docs/spec/flows.md) 对应一条。
- **最小写入集**：保留用户已有未提交改动；新增改动只触碰任务必需的文件。
- **先确认已落地的内容，再补缺口**：避免重复规划或重做已完成的工作。
- **范围明确的实现 / 修复任务，定位后直接执行**：不把简单任务过度规划。

## 2. 架构红线（不可违反）

- CLI 与 Core 同进程同二进制（`pawork` 是唯一正式宿主），纯 Rust 实现；不引入 Node / Bun / V8 / 嵌入式 JS Runtime。GUI 以独立 GPUI 进程（`apps/desktop`）经 GUI Connection Protocol 连接 CLI，不嵌入 Core；Desktop 构建链同样保持纯 Rust。
- `pawork-domain`（crates/domain）不得依赖任何 GUI framework（包括 GPUI/Tauri）、SQLite、HTTP Client、OS Keychain、Git、任何具体 Provider。
- 禁止包间循环依赖；包布局与依赖方向见 [docs/architecture.md](docs/architecture.md) §2。
- Agent Engine 不得通过判断 Provider 名称走特例逻辑（统一走 canonical domain）。
- Secret（明文 Token）不写入数据库与日志。
- 所有 Agent 事件必须可持久化、可重放。
- GUI 不得直接访问 Provider、数据库与工具；只能通过 GUI Connection Protocol 连接 CLI，经 CLI 宿主访问 Core（不直接加载 Core crate）。

违反以上任意一条须先向用户确认。冻结契约、包布局或安全语义的破坏式改动，确认后写入 [docs/architecture.md](docs/architecture.md) 与对应 Spec，golden 先行。

## 3. 命名与结构约定

- 项目名：`Pawork`；CLI 二进制名：`pawork`。
- `pawork`（apps/pawork）是 Core 的唯一正式宿主；不存在独立的 daemon / rpc 入口。
- 仓库根即 Cargo workspace 根。
- 当前布局为 21 成员（19 库 + 2 应用）：19 库平铺 `crates/<短名>`（目录 = 包名去 `pawork-` 前缀），2 应用 `apps/{pawork,desktop}`；包清单与依赖方向见 [docs/architecture.md](docs/architecture.md) §2。
- crate 统一 `pawork-` 前缀。**当前不新增包**，只往既有包加模块；包布局变更须向用户确认。
- 归档资产以 git tag `v2-final` 兜底，复活条件登记在 [docs/spec/backlog.md](docs/spec/backlog.md)；不得把归档代码复制回仓库其它位置。

## 4. 任务粒度

- 每个任务应在数小时内可独立完成、独立验收。
- 写入集尽量收敛到单一包或一组紧相关文件；不同任务不修改同一文件。
- 用户可见能力、契约、安全、Desktop 或验证边界变化时，同批更新对应 Spec 与架构/设计文档。
- 阶段外候选登记见 [docs/spec/backlog.md](docs/spec/backlog.md)。

## 5. 验证决策

少测试、无全量门禁：只做能证明本任务核心行为的关键定向测试。默认死表为 `cargo test -p <crate> --offline --lib --tests`（多包可一次多个 `-p`，但仍是一个 Cargo 进程，不因包多改用 `--workspace`）。`cargo check -p <crate>` 仅在该包无测试或只需类型检查时使用。三类关键测试不推迟：安全红线定向回归、持久化与重放契约 golden、协议与解析 golden/种子；邻包 golden/probe/e2e/desktop/`cargo check -p pawork` 默认不跑，仅主代理收口且对应文件确有改动时加跑一次。

补充约定：

- **主干可用**：`pawork` 二进制可编译、可运行，既有冒烟行为不回退；合并/归档波补跑 `cargo tree` 断言（无环、`-p pawork` 闭包不膨胀）。
- **冻结契约不静默破坏**（清单见 [docs/architecture.md](docs/architecture.md) §3.2）：golden 先于实现改动；schema/wire 演进须用户确认。
- 全量门禁与发布不在默认范围；获明确授权后须另立任务和门禁。
- **功能测试用模型**：需要真实 Provider 的功能验证（真窗口 Run、流式/工具/审批、live smoke）固定使用 `opencode-go` / `glm-5.3-flash`。只通过当次 Host/CLI `--provider opencode-go --model glm-5.3-flash` 覆盖，不写持久默认；产品示例默认仍是 `glm-coding` / `glm-5.2`。连接失败按真实终态记录，不得换未指定模型或伪造成功。口径见 [docs/spec/verification.md](docs/spec/verification.md) §2.1。

硬约束：

- 禁止 `cargo clean`；复用默认 `target/` 增量缓存，仅清理本任务临时输出。stale incremental 用 `python3 scripts/clean-stale-incremental.py` 按前缀清理，禁止 `rm -rf target`。
- 全会话同一时刻只允许一个 Cargo 进程；并行轨不得抢同一 `target/` 锁。审查者读 worker `/tmp` 日志，不再编译。
- 文档或不影响构建行为的配置改动只做链接、格式与 diff 检查，不为形式完整跑编译。
- 前一层失败先收敛原因，不盲目扩大范围。
- Secret、Policy、路径越界、持久化/重放、破坏性文件/进程操作等高风险改动必须带对应定向回归。

任务结束报告至少包含：

```text
Validated: <实际命令 / tests / checks，或 none 及理由>
Targeted regressions: <实际覆盖，或 none>
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

## 6. 文档约定

- 中文撰写，保留关键术语英文。
- 常设文档（入口 [README.md](README.md)）：[docs/architecture.md](docs/architecture.md)（架构）· [docs/design.md](docs/design.md)（功能设计）· [docs/gui-design.md](docs/gui-design.md)（Desktop GUI，配套 [design/](design/README.md) 视觉基准）· [docs/references.md](docs/references.md)（参照项目与调研）· [docs/spec/README.md](docs/spec/README.md)（产品与包级 Spec）· 本文（工程约定与经验）。
- **Spec 边界**：`docs/spec/` 产品篇是跨事实源的产品化汇总，包级 Spec 是包内功能的文档化镜像；均不替代源码/golden、`docs/architecture.md` 的布局与冻结契约。用户可见能力、契约、安全、Desktop、验证或运维边界变化时，同批更新对应 Spec；「已实现」「已验证」「已人工验收」「已发布」必须分开表述。
- **包级 Spec 维护规则**：固定八节结构（见 [docs/spec/README.md](docs/spec/README.md)）。写入集改了模块树、对外 API、`pawork-*` 依赖边、feature 门、红线相关行为或测试资产时**同批**更新该包 `docs/spec/crates/<pkg>.md`；冲突以源码为准并回写。
- 交叉引用使用仓库内相对路径链接。

## 7. 提交与分支

- 分支前缀默认 `codex/`，用户另有要求时遵从用户。
- 提交、推送、发布仅在用户请求或已确认任务链明确包含时执行。
- 不使用 `git reset --hard` / `git checkout --` 清理用户改动，除非用户明确要求。
- 优先非交互式 git 命令。

## 8. 安全与权限

- 不执行递归删除、覆盖 workspace 外路径、`$HOME` / 根目录等宽范围破坏性操作。
- 文件操作输入必须基于 `workspace_id + relative_path`，禁止模型直接传任意绝对路径。
- 子进程、网络、Secret 访问须经 Policy / Sandbox 约束：红线见 [docs/architecture.md](docs/architecture.md) §1/§4；实现承载于 `pawork-policy` / `pawork-exec`（Spec：[policy](docs/spec/crates/policy.md) / [exec](docs/spec/crates/exec.md)）；凭证链路见 [docs/spec/flows.md](docs/spec/flows.md) §4。

## 9. 子代理使用

- 文档等一致性关键产物由主代理直接撰写。
- 实现阶段：边界清晰、写入集互不重叠的任务可并行派发，遵循服务级 `AGENTS.md` 的路由与并发上限。
- 派发实现 / 核查子代理时，提示词须点名写入集各包 `docs/spec/crates/<pkg>.md`；不要让子代理先通读全部 Spec。
- 确定性检查先于模型审查；每个门禁只调用一个审查者。

## 10. 验证命令模板

```bash
cargo test -p <crate> --offline --lib --tests
```

仅在该包无测试或只需类型检查时改用 `cargo check -p <crate> --offline`。protocol golden、probe、spawn_e2e、desktop、`cargo check -p pawork` 默认不跑（probe/spawn_e2e/app smoke 已按 required-features 门控，默认死表不再编译，复跑命令见包级 Spec）。合并 / 归档波追加 `cargo tree` 断言（无环、`cargo tree -p pawork` 闭包对比）。

## 11. 工程经验

长期复用的调试知识。修同类问题时先核对此节，再搜源码。

**tracing-core interest 缓存投毒**
测试里 tracing 断言会偶发全绿变全空。每个 callsite 在全局注册表只缓存一份 Interest；`has_just_one=true` 时若在无 scoped default 的线程首次命中，会缓存 `Interest::never()`，之后所有线程该 callsite 被宏门跳过。修法：`RecordingCapture` 对同一 subscriber 做两次 `Dispatch::new`（注册表推到 ≥2），钉住新 callsite 走 Read 路径。

**gpui 前台执行器无 tokio reactor**
Desktop 真窗口启动崩溃（exit 134），probe-smoke 不复现。握手后 `ack`/`subscribe_all` 若在 gpui 前台执行器上 await，`tokio::time` 会 panic。修法：握手与订阅全部 `runtime.spawn`；真窗口启动没有自动门禁，改连接路径时要人工开窗。

**FollowScroll 滚轮双计**
gpui 0.2.2 Bubble 相监听按注册逆序分发，容器先应用偏移、用户监听再投影 delta，同一次滚动计两次。修法：放弃 delta 投影，直读 `is_scrolled_to_bottom()`。

**Seatbelt 读白名单在 Darwin 25+ 不可行**
deny-default + 枚举 `/usr` `/System` `/bin` 仍会 SIGABRT：firmlink/cryptex 使枚举覆盖不了进程启动路径。正式模型是读整盘 allow + secret 挖洞，隔离靠写闸和网络闸。

**Seatbelt symlink 根**
规则按 canonical 路径匹配；只写 `/var` 会落空，必须 raw + canonical 双形态进 profile。

**InFlight 同键不同 command_id**
幂等等待方按自身 command_id 注册 Notify，占位行持有者是另一 id，叠加丢唤醒会挂死；record 失败若不释放 inflight，同进程重试继续挂。修法：有界等待后回 loop 重查 SQLite CAS；DB 类错误先幂等重试再 release。

**EventHub Lagged 禁止 seq-0 旁路**
Lagged 后不得伪造起点直发；改经 hub 真序列取信封，并回 `ReplayUnavailable`。

**host 30s 心跳 × desktop 空闲**
`gui_server` 心跳超时 30s，任意入站帧刷新。Desktop 若不周期心跳，空闲约 30s 必断（Reconnect 可恢复、Run 不取消）。修法：controller 泵循环连续约 15s 空闲发 `heartbeat()`。

**shell 反引号灾难地板**
`` echo `rm -rf /` `` 曾因收尾反引号被推进 inner、地板 `== "/"` 不命中而静默放行。收尾符 break 前不得入 inner；command substitution 与 backtick 用同一套 danger+floor 断言。

**client 事件泵抢占命令错误帧**
`FrameWant::Event` 若匹配所有 `ServerFrame::Error`，常驻事件泵会抢走带 `request_id` 的命令错误，等待方超时误报 Disconnected。Response/Snapshot/Resume 只接同 request_id 的错误，Event 只接 `request_id=None` 的连接级错误。

**.jsonl 嗅探截断**
真实 session_meta 首行可超过 8KiB；按 8KiB 截断再 JSON 解析会误判格式。嗅探必须读完整首行。

**notify Rescan 空路径**
后端溢出发 `Flag::Rescan` 且路径为空时，按路径映射会直接丢弃。应转为每个 workspace root 的 Upsert，触发全量重扫。

**Claude subagent sidecar**
`agent-*.jsonl` 复用父会话 sessionId。`session_scan` 必须排除这些 sidecar，否则全量导入会 `CompatImportConflict`。

**Desktop 连接与诚实性**

- 关闭窗口不取消已进入 Core 的 Run。
- 功能结论必须同时有真实窗口状态与源码外事实（文件、Git、Host 或 PTY 输出）；截图不写入仓库。
- Terminal 是过滤 ANSI/VT 的纯文本视图，不是完整 VT emulator。
- 菜单开着时 Timeline 条目被虚拟化卸载，浮层随条目回收，属可接受行为。
- `apps/desktop` 直接业务依赖只允许 `pawork-client`。
- 功能测试用模型固定为 `opencode-go / glm-5.3-flash`（当次 Host 参数，不写持久默认）。
