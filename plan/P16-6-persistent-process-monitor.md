# P16-6：Persistent Process / Monitor（常驻进程与监视循环）

> Phase 16 · Modern Agent Workflow · 状态：🟢已完成 · TargetVerified（有界：library/core verified，host composition deferred） · 依赖：P16-4、P11-6、P11-7、P1-4、P13-2

**最终目的**：让一个进程或监视循环能脱离单次 Run 独立常驻——例如长跑的 dev server、watch 构建、日志监视——在 CLI/GUI 断连后继续运行、重连可接管，输出被捕获为可检索的事件流，使「后台 watch / 常驻服务」成为 Agent 可观测、可控的一等公民，而非游离于管理之外的孤儿进程。

**涉及范围**：`process-runtime`（扩展）、新增 `monitor-service`；复用 PTY Service（P11-6）、进程树清理（P11-7）、`agent-events`、`session-store` 与 P13-2 Event Hub；P13-5 多 GUI 只增加订阅者，不改变任务归属。

## 细分步骤

1. **常驻进程模型** —— 目的：在 `process-runtime` 之上定义 `PersistentProcess`——绑定到一个 workspace，生命周期跨 Run/连接，重启 Core 后可重新发现并接管（基于持久化的 PID/句柄与 canonical event 日志）。
2. **Monitor 循环（Plugin Package 的统一执行宿主）** —— 目的：`monitor-service` 提供声明式监视循环（文件变化/进程退出/正则命中/端口状态），命中后产出 canonical event，可作为 P16-5 `event` 触发器的来源，形成「监视 → 触发自动化」链路。同时作为 [P17-2](P17-2-plugin-package-format.md) Plugin Package 中 Monitors 类型声明的**唯一运行时执行点**：package manifest 只声明 monitor 配置/trigger/permissions/lifecycle/required capability，统一进入 `monitor-service` / `task-manager` 执行，不重定义运行时语义。
3. **输出捕获与节流** —— 目的：常驻进程的 stdout/stderr 流式捕获为事件流，超量输出走 artifact（P13-8）+ 滚动裁剪，复用慢客户端隔离（P13-5）避免高吞吐进程回压 Core。
4. **断连续存与重连接管** —— 目的：复用 PTY Service 的重连能力（P11-6），断连不杀进程；重连按 snapshot 恢复视图并续接增量；进程异常退出按进程树清理（P11-7）不留孤儿。
5. **资源与安全** —— 目的：常驻进程同样受 sandbox（P11）与 policy 约束；监视循环的文件访问范围限定在 workspace（复用 file-index 信任边界）。
6. **查询面** —— 目的：`core-api` 暴露常驻进程/监视器列表、输出流订阅与控制（重启/停止）。
7. **定向 / Mock 测试** —— 目的：常驻进程断连存活与重连接管、监视循环命中触发事件、超量输出裁剪、异常退出无孤儿。仅定向 + Mock smoke，不要求 workspace 全量门禁。

### Core-owned 进程统一 Sandbox/Process Runtime 所有权（显式约束）

> 对齐 `.codex-brief/ENHANCEMENT-BRIEF.md` §4「后续 Runtime 执行所有权」：所有 Core-owned 本地进程必须经 `Sandbox Runtime → Process Runtime` 统一执行，常驻/后台执行不构成例外。

- 常驻进程 / monitor 禁止直接 `tokio::process::Command` / `tokio::spawn` 绕过 Sandbox Runtime；统一经 `Sandbox Runtime → Process Runtime` 执行。
- 不自复制 Job Object / process-group cleanup：统一复用 P11-7 进程树清理，`process-runtime` / `monitor-service` 不另起一套清理逻辑。
- 不自定另一套 filesystem / network policy：统一复用 `sandbox-runtime` 的 `SandboxPolicy`，常驻进程不因 persistent 而放宽。
- 不因 persistent/background 绕过 guarantee reporting：进程实际获得的 `SandboxGuarantees`（含各维度降级）照常上报、可持久化、可观测。
- **policy/guarantee 跨生命周期一致**：同一 persistent process 的 sandbox policy 与 guarantee 在 spawn / restart / reattach / recovery 全流程保持一致；restart 必须走同一受沙箱路径，不得降级为 unsandboxed 重启；重连接管与崩溃恢复后重新核对 guarantee 状态并上报，禁止「先 unsandboxed 拉起再补沙箱」。
- 已有 ToolKind 三执行位点设计（见 [P17-10](P17-10-browser-computer-runtime.md) 与 [sandbox](../docs/features/sandbox.md)；`ExecutionOwner` 冗余枚举已按 [P15-10](P15-10-review-remediation.md) 删除，位点由 `ToolKind` 直接承载）则直接复用，不另起一套进程所有权 / 执行模型。

## 主要产出物

- `process-runtime` 的 `PersistentProcess` 扩展
- `monitor-service` 声明式监视循环
- 输出捕获/节流与重连接管
- 定向测试

## 验收标准

- [ ] 未达成：`PersistentProcess`（attach/detach/reconnect/restart、Core 重启重新发现接管）未实现 → 登记 **P16-10 #T5**
- [ ] 部分达成：四类 Observation 的纯 `evaluate` 与 `MonitorEvent::Triggered` 事件已验（测试支撑：`file_change_matches_watched_path` / `process_exit_matches_by_pid_or_task` / `regex_match_returns_matched_substring` / `port_state_reports_open_closed` / `lifecycle_emits_started_triggered_stopped`）；无真实 driver/watcher（driver 已删除，观测样本由调用方注入，本 crate 不内置 watcher）→ 未达成，登记 **P16-10 #T5**
- [ ] 未达成：P17-2 Plugin Package Monitors 未落地，宿主接线未实现 → 登记 **P16-10 #T5**
- [ ] 部分达成：`Throttle` 有界裁剪已验（`never_grows_unbounded_under_high_throughput` 等）；输出不进 artifact，无回压/慢客户端隔离接线 → 未达成，登记 **P16-10 #T5**
- [ ] 部分达成：经注入 task-manager 的 process 路径复用 SandboxBackend → ProcessRuntime 且无直接 spawn（`no_direct_process_spawn_in_source` / `monitor_registers_as_task_kind_monitor`）；常驻进程宿主、异常退出清理与 guarantee 跨生命周期一致性未实现 → 未达成，登记 **P16-10 #T5**

**相关文档**：[background-task-manager (P16-4)](P16-4-background-task-manager.md) · [scheduled-automation (P16-5)](P16-5-scheduled-automation.md) · [process](../docs/features/process.md) · [sandbox](../docs/features/sandbox.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；复用 PTY Service（P11-6，基线 `portable-pty`）与文件监听（基线 `notify` / `notify-debouncer-full`）。新 crate `monitor-service` 依赖方向：`agent-domain → monitor-service → app-service`。

## 校准记录（2026-08-12）

依据 [p16-review](../docs/review/p16-review.md) 评审结论与当前工作区 remediation 状态校准：四类 Observation 的纯 evaluate、命中事件化、输出节流与「经注入 task-manager 统一执行」约束属库级实现且有测试支撑，保留 **TargetVerified（library/core）**；「PersistentProcess、四类 source 真实 driver、artifact 输出、sandbox/policy/guarantee 一致性、P17-2 唯一执行宿主」未达成，登记 **P16-10** 并映射后续任务 #T5。

验证记录：`scripts/p16-gate.sh` 四类全 PASS（独立 `target/gates` 跑完已清理）；monitor-service 测试 24 passed（lib 18 + 集成 6）。

```text
Validation Level: L2（P16 簇门禁脚本，文档校准任务附带复跑）
Affected crates: none（本任务仅改 plan/P16-6 文档）
Validated: scripts/p16-gate.sh；monitor-service 定向测试
Targeted regressions: evaluate 纯函数与事件化
Full workspace gate: NOT RUN（未命中升级条件）
```
