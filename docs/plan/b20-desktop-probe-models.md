# B20 · Desktop Probe 功能测试模型裁决

> 状态：✅ 已实现，定向回归与本轮真实探针通过；独立 review PASS。更新：2026-10-12。旧基线 `afd7d34189a113fc8ebaac1c280449c5d7272ca4`。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 裁决

2026-10-12 用户新授权将本轮当次精确模型改为优先 `glm-coding / glm-5.3`（用户标识 `zai/glm-5.3`），其次 `xai / grok-4.7`。证据位于 `/tmp/pawork-b20-probe-acceptance/evidence`。

`--probe` 不是模型选择，也不是真实功能验证。它只连接、取 snapshot、拉 `model_list`，打印 `connected:` 目录行后退出。`fetch_models` 失败原样失败，不用空目录冒充成功。此模式不选模型、不发消息、不启动 run。

`--probe-smoke` 是真实功能验证。只从正式 `ModelList` 选择精确 provider/model 且 `enabled && text` 的授权候选，优先 `glm-coding / glm-5.3`，其次 `xai / grok-4.7`；两者均不可用时按 missing / disabled / text_false 明确失败。选择发生在请求前，请求发出后不改选另一对。当次消息携带选定模型，不写持久默认、认证或禁用列表，也不按 model id 跨 provider 回退。当前流程只使用选定的一对，模型切换未覆盖，签名行仍输出 `model_switch=uncovered`。旧 Go 固定对仅保留在历史完成记录。

回合终态按当前 session 的该 run 判断。`MessageSent` 只绑定活动 session，且只在还没有跟踪 run 时绑定，然后复查已有投影。审批后不再等第二次回执，终态认审批卡上的 `run_id`。已投影终态看时间线标签 `run completed`、`run failed`、`run interrupted`、`run cancelled`（失败带 ` · ` 原因时仍取 `failed`），不看是否离开 active 列表，也不把助手正文当终态。进行中标签继续等；未知 `run …` 标签拒绝。现场事件只认该 run 的 `RunChanged`：`Completed` 成功，`Failed`、`Interrupted`、`Cancelled` 是错误。取消只接受同一 run 的 `Cancelled`。其它 session 的事件不进当前时间线（`DesktopProjection::apply_event` 按活动 session 闸门）。

写文件步骤只看审批卡和该 run 的终态。模型自己的回复不证明目标文件字节已落盘。没有审批卡且 run 完成时记 `approval=not_requested`，不记成文件已写。

断线重连后，run 仍在 active 列表则取消并记 `disconnect_survive=running`。不在 active 列表不等于完成；只有持久化 run 标签 `completed` 才记 `disconnect_survive=completed`。cancelled、failed、interrupted、未终态或未知标签都失败。

`--instance` 与 `PAWORK_DATA_DIR` 只影响 socket 和数据目录，不隔离全局认证。认证文件仍是 `$PAWORK_HOME/auth.json`，未设置时为 `~/.pawork/auth.json`。

## 历史实施事实（2026-10-08）

改前 `--probe-smoke` 先选 `glm-coding` / `glm-4.7`，否则选 `deepseek` / `deepseek-v4-flash`。`pick_model` 在精确对缺失时按 model id 跨 provider 回退。`pick_other_model` 再找 `deepseek-v4-flash`（provider 为 `deepseek` 或 `opencode-go`）发第二轮；找不到则 `second_turn=skipped`，并不把同一模型再发一轮。这与 [功能测试模型](../spec/verification.md#21-功能测试用模型) 不一致。Desktop 没有 `--provider` / `--model` 参数，所以授权对写在冒烟消息上，不写 Global / Workspace 持久默认。产品示例默认仍是 `glm-coding` / `glm-5.2`。

等待器以前会把旧回合正文、非取消终态或「快照里不再 active」读成成功。审查要求改成：审批后的终态认审批卡上的 run；`MessageSent` 只绑定活动 session，且只在尚未跟踪时绑定，并复查已有投影；终态看时间线标签，不看离开 active 列表或助手正文；进行中继续等，未知标签拒绝。重连后的 `completed` 仍来自持久化时间线标签 `run completed`。

## 历史未纳入（2026-10-08）

不改 B21。不跑完整 Desktop 套件，不跑真实 Provider / Host，不 `cargo clean`。真实冒烟需要认证或配置，本次没有执行。定向测试通过不能写成探针已在真实模型上通过，独立 review 也不能关闭本任务。已审实现仅本地提交，未推送；真实探针仍在任务验收范围。

## 写入集

- `AGENTS.md`、`docs/spec/verification.md`：同步本轮功能测试模型授权。
- `apps/desktop/src/main.rs`
- `crates/app/src/gui_host/bus.rs`、`crates/app/src/gui_host/handlers/run_start.rs`、`crates/app/src/gui_host/tests/run.rs`：修复终态先于执行槽释放导致下一轮被误拒绝，并验证取消后立即启动。
- `crates/app/src/gui_host/tests/mod.rs`：旧队列刷新与新登记的并发回归。
- `docs/spec/crates/app.md`
- `docs/plan/b20-desktop-probe-models.md`
- `docs/spec/crates/desktop.md`
- `docs/ROADMAP.md`

## 完成条件

- 探针执行口径、实际 provider/model 与验证规格一致，失败保留真实终态。
- 改动后的受影响探针验证有实际证据；不能用替换模型掩盖连接失败。

2026-10-08 口径按当时源码对齐，失败不再读成成功；当时第二条仍待完成。本轮已补真实 `--probe` / `--probe-smoke` 证据，独立 review PASS，按本项既定完成条件收口。

## 历史验证（2026-10-08）

```text
rustfmt --edition 2021 --config skip_children=true apps/desktop/src/main.rs
cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders probe_run_terminal -- --test-threads=1
```

`rustfmt` exit 0。随后这一次 Cargo exit 0：2 passed；0 failed；275 filtered out。`Finished test profile in 26.33s`。stdout、stderr 与退出码分别是 `/tmp/pawork-b20-desktop-probe-models/cargo-test-waiter.stdout`、`cargo-test-waiter.stderr`、`exit_code-waiter`。

更早一次同过滤命令在测试里用局部变量 `projection` 遮蔽构造函数，编译失败（`E0618`；`cargo-test-compile-fail.stderr`，退出码未单独落盘）。改名 `fresh_projection` 后的旧通过日志仍是 `cargo-test.stdout`、`cargo-test.stderr`、`exit_code`（2 passed，`Finished test profile in 39.64s`）。那次源码还没有审查修复，不能当作本次证据。这些旧日志都保留。

未跑整个 Desktop，未跑真实 `--probe` / `--probe-smoke`，未跑真实 Provider。

```text
Validated: rustfmt --edition 2021 --config skip_children=true apps/desktop/src/main.rs；cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders probe_run_terminal -- --test-threads=1（exit 0，2 passed / 275 filtered out，Finished test profile in 26.33s）
Targeted regressions: 当前 run 完成、审批后不再次回执即完成、终态早于回执、提前失败、其它 session/run、提前取消；拒绝失败回合、Completed 冒充取消、失败审批
Full workspace gate: NOT RUN
```

## 完成记录

本轮独立审查：Sol max 对话 `01a12d7c-c7f9-75f0-95ff-f1d5ed3aecb0`，turn `01a12d7c-cb37-7e30-9174-8b38e80d125b` completed / error=null / PASS，阻断与非阻断问题均无。受审 11 文件快照为 53,841 bytes / SHA-256 `3e3e30e733d81b5f81ed6893aca63d0f4fc996b0414315058ca8d7cbd1f5bd1a`；独立只读核验源码锁序、原始日志、退出码、SQLite 与文件 / Host 状态。审批未触发、模型切换未覆盖、真窗口及 auth 摘要变化归属仍保留上述边界，不作为已验。

2026-10-12：执行对话 `01a12d3b-0804-7612-a1ba-9a0e2da03edc`（Grok 4.7 xhigh）完成最小模型选择与运行槽修复。GUI session 从首条终态起暂存它与后续事件；收尾先清 scope / approvals / run / browser，再在同一短同步锁中释放会话槽并按原顺序写入既有 EventHub。新登记和同 session 发布等待该锁，旧队列不能清掉新登记或被新事件超越。sink 在接受终态时登记，durable seal 去重保留；PTY / Global 流仍即时发布。

本轮 Desktop 两条模型选择与两条既有等待器回归通过（4 passed / 300 filtered）；该源码之后未改。最终 App 两条回归通过（2 passed / 309 filtered）：正式 RunStart / RunCancel 后收到取消终态即可再启动，以及旧刷新不清新登记、不超越新事件。最终 `cargo build -p pawork --offline` exit 0。Desktop 正式二进制构建 exit 0；指挥对本轮五个 Rust 文件的 `rustfmt --check --edition 2021 --config skip_children=true` 与 diff 检查 exit 0。

新临时 Host 的 `probe-smoke-final` exit 0：`models=23 first=glm-coding/glm-5.3 first_turn=pong trusted=0 approval=not_requested assistant_turns=4 cancelled=1 persisted=17 disconnect_survive=running model_switch=uncovered`。指挥直接只读 SQLite 核实四个 Run 为 completed / completed / cancelled / cancelled，任务用量对应 succeeded / succeeded / cancelled / cancelled，实际 provider/model 均为 `glm-coding / glm-5.3`。真实数据与核证见 `coordinator-final-smoke-check.json`。本轮未触发审批，`workspace/target/s7-wave-d-smoke.txt` 不存在，不声明文件写入或审批通过；模型切换仍未覆盖。CLI 产物 SHA-256 为 `577c226287dc061a9592e858dcab29c1d3c3372e362341e7b638826f9b9c4866`。

配置 `config.toml` 与偏好 `desktop.json` 的前后摘要一致；`auth.json` 摘要变化的归属未核明，保留现场，不声称认证字节未变。原 Host 51223 由执行者停止；复验 Host 55625 经指挥核实精确二进制与 `b20probe` 后发送 SIGTERM，随后确认不存在。用户窗口未用于本轮探针。

较早的 27 项 run 测试输出只剩工具回显重建稿 `cargo-test-run-visible-terminal.stdout`，没有原始退出码，不计最终验证。原始环境临时文件已在指挥读取前移除；辅助脚本的引号 / 换行与两次误调用失败不计成功，详情见执行对话及 `coordinator-evidence-cleanup.json`。最终成功证据只使用新日志和直接观察的退出码。

```text
Validated: Desktop probe filter（exit 0，4 passed）；最终 App 两 filter（exit 0，2 passed）；正式 pawork / Desktop 构建（exit 0）；--probe（exit 0，23 项目录）；--probe-smoke（最终 exit 0）；只读 SQLite / 文件事实 / 配置摘要 / 本项 Host 回收；rustfmt / diff / 相对链接检查
Targeted regressions: 精确模型选择与 enabled/text 拒绝、既有当前 run 终态等待器、取消终态可见即再启动、旧队列刷新与新登记的顺序隔离
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

2026-10-12 本轮首轮真实 `--probe` exit 0，正式 Host 返回 23 项模型目录。首次 `--probe-smoke` exit 1：第四次发送返回 `session already has an active operation`。真实 SessionStore 的前两轮为 completed，第三轮为 cancelled；账本对应 succeeded / succeeded / cancelled。该失败不是环境缺失，终态广播早于 Host 运行槽释放，已按上述最终版本修复并复验。原始 `probe-smoke.stdout`、`probe-smoke.exit` 与独立核证 `coordinator-first-smoke-check.json` 保存在本项证据目录，复验使用新日志。

2026-10-08：已实现，定向回归通过，独立 review / 复审 PASS。Sol max 审查对话 `01a11aa3-0512-7562-8f08-f3811635712a` 初审四项问题已由原 Grok xhigh 实现对话 `01a11a71-1fbe-79f3-9b96-0b5bdd455a80` 修复；同一 reviewer 复审确认阻断与非阻断问题均无。真实探针验收待完成，独立 review 不能关闭本任务。`--probe` 只列目录。`--probe-smoke` 固定当次 `opencode-go` / `glm-5.3-flash`。唯一授权模型覆盖不了切换，输出 `model_switch=uncovered`。终态按该 run 的时间线标签与 `RunChanged` 判断。真实 Host / Provider 未执行。
