# B20 · Desktop Probe 功能测试模型裁决

> 状态：已实现；定向回归与独立 review / 复审通过；真实探针验收待完成。独立 review 不能关闭本任务。更新：2026-10-08。基线 `afd7d34189a113fc8ebaac1c280449c5d7272ca4`。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 裁决

`--probe` 不是模型选择，也不是真实功能验证。它只连接、取 snapshot、拉 `model_list`，打印 `connected:` 目录行后退出。`fetch_models` 失败原样失败，不用空目录冒充成功。此模式不选模型、不发消息、不启动 run。

`--probe-smoke` 是真实功能验证。当次消息只携带授权对 `opencode-go` / `glm-5.3-flash`，不写持久默认，也不按 model id 跨 provider 回退。`ModelList` 缺这对即失败。唯一授权模型覆盖不了切换，签名行输出 `model_switch=uncovered`。旧的 `glm-coding` / `glm-4.7` 与 `deepseek-v4-flash` 选择已删除。

回合终态按当前 session 的该 run 判断。`MessageSent` 只绑定活动 session，且只在还没有跟踪 run 时绑定，然后复查已有投影。审批后不再等第二次回执，终态认审批卡上的 `run_id`。已投影终态看时间线标签 `run completed`、`run failed`、`run interrupted`、`run cancelled`（失败带 ` · ` 原因时仍取 `failed`），不看是否离开 active 列表，也不把助手正文当终态。进行中标签继续等；未知 `run …` 标签拒绝。现场事件只认该 run 的 `RunChanged`：`Completed` 成功，`Failed`、`Interrupted`、`Cancelled` 是错误。取消只接受同一 run 的 `Cancelled`。其它 session 的事件不进当前时间线（`DesktopProjection::apply_event` 按活动 session 闸门）。

写文件步骤只看审批卡和该 run 的终态。模型自己的回复不证明目标文件字节已落盘。没有审批卡且 run 完成时记 `approval=not_requested`，不记成文件已写。

断线重连后，run 仍在 active 列表则取消并记 `disconnect_survive=running`。不在 active 列表不等于完成；只有持久化 run 标签 `completed` 才记 `disconnect_survive=completed`。cancelled、failed、interrupted、未终态或未知标签都失败。

`--instance` 与 `PAWORK_DATA_DIR` 只影响 socket 和数据目录，不隔离全局认证。认证文件仍是 `$PAWORK_HOME/auth.json`，未设置时为 `~/.pawork/auth.json`。

## 事实

改前 `--probe-smoke` 先选 `glm-coding` / `glm-4.7`，否则选 `deepseek` / `deepseek-v4-flash`。`pick_model` 在精确对缺失时按 model id 跨 provider 回退。`pick_other_model` 再找 `deepseek-v4-flash`（provider 为 `deepseek` 或 `opencode-go`）发第二轮；找不到则 `second_turn=skipped`，并不把同一模型再发一轮。这与 [功能测试模型](../spec/verification.md#21-功能测试用模型) 不一致。Desktop 没有 `--provider` / `--model` 参数，所以授权对写在冒烟消息上，不写 Global / Workspace 持久默认。产品示例默认仍是 `glm-coding` / `glm-5.2`。

等待器以前会把旧回合正文、非取消终态或「快照里不再 active」读成成功。审查要求改成：审批后的终态认审批卡上的 run；`MessageSent` 只绑定活动 session，且只在尚未跟踪时绑定，并复查已有投影；终态看时间线标签，不看离开 active 列表或助手正文；进行中继续等，未知标签拒绝。重连后的 `completed` 仍来自持久化时间线标签 `run completed`。

## 未纳入

不改 B21。不跑完整 Desktop 套件，不跑真实 Provider / Host，不 `cargo clean`。真实冒烟需要认证或配置，本次没有执行。定向测试通过不能写成探针已在真实模型上通过，独立 review 也不能关闭本任务。已审实现仅本地提交，未推送；真实探针仍在任务验收范围。

## 写入集

- `apps/desktop/src/main.rs`
- `docs/plan/b20-desktop-probe-models.md`
- `docs/spec/crates/desktop.md`
- `docs/ROADMAP.md`

## 完成条件

- 探针执行口径、实际 provider/model 与验证规格一致，失败保留真实终态。
- 改动后的受影响探针验证有实际证据；不能用替换模型掩盖连接失败。

口径已按源码对齐，失败不再读成成功。第二条仍待完成：没有真实 `--probe` / `--probe-smoke` 证据。

## 验证

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

2026-10-08：已实现，定向回归通过，独立 review / 复审 PASS。Sol max 审查对话 `01a11aa3-0512-7562-8f08-f3811635712a` 初审四项问题已由原 Grok xhigh 实现对话 `01a11a71-1fbe-79f3-9b96-0b5bdd455a80` 修复；同一 reviewer 复审确认阻断与非阻断问题均无。真实探针验收待完成，独立 review 不能关闭本任务。`--probe` 只列目录。`--probe-smoke` 固定当次 `opencode-go` / `glm-5.3-flash`。唯一授权模型覆盖不了切换，输出 `model_switch=uncovered`。终态按该 run 的时间线标签与 `RunChanged` 判断。真实 Host / Provider 未执行。
