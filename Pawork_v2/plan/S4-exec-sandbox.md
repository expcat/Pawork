# S4：命令执行与沙箱

> 阶段 S4 · 命令执行 · 状态：⚪未开始 · 依赖：S3 · 规模：中

## 目标（本阶段结束时用户能做什么）

Agent 获得受控的命令执行能力：run_command 工具经 shell 风险分类 + 审批 + 沙箱执行，输出流式回传并截断，Ctrl-C / 取消会清理整棵进程树，沙箱后端不可用时 fail-closed 拒绝执行。**至此达成旧计划 M4 的首要验收——`pawork` 在真实仓库端到端完成「读文件 → 改代码 → 跑命令」的完整编码任务**，而且是在每一步都已真实验证过的基础上自然合拢。

## 涉及包与 V1 资产

| V2 包（目录） | 本阶段动作 | V1 来源与方式 |
| --- | --- | --- |
| `pawork-exec`（execution/exec） | 激活：V1 `process-runtime` + `sandbox-runtime` 迁移，平台代码按 `os/{windows,linux,macos}.rs` 重排；进程树管理（Windows Job Object / Unix 进程组）、沙箱后端（AppContainer / Landlock / Seatbelt）、fail-closed 降级；agent-domain 类型中性化（保持 W1 可独立发布）。**`pty-service` 本阶段不迁**（消费者是交互式终端/GUI，S9 登记） | 直接迁移（[archive/M1](archive/M1-execution-security.md) pawork-exec 节） |
| `pawork-tools` | 增强：`run_command` 迁移——cwd 限定 workspace root、超时（`AllowWithConstraints`/descriptor 默认）、`max_output_bytes` 截断、stdout/stderr 经 `ToolOutputDelta(ToolOutputStream::Stdout/Stderr)` 流式入事件 | 直接迁移 |
| `pawork-policy` | 接线：`shell` 风险分类（S3 已随包迁入）→ run_command 的 pre-tool 决策（`CommandRisk::Dangerous` → `AskUser` + `RiskLevel::Dangerous` 提示）；参数注入分类回归 | 接线 |
| `pawork-engine` | 增强：`CancelHandle` + `ProcessTreeCleaner` 语义对齐 V1——run 取消（用户 Ctrl-C / 上限触发）时级联终止工具子进程树，`RunCancelled` 事件收尾 | 语义对齐 V1 `cancel.rs` |
| `pawork-cli` | 增强：命令输出的流式渲染（stderr 区分着色）、执行中命令的取消提示 | 新写 |

## 关键任务

1. **exec 迁移**：Windows 路径先行实测（Job Object 树清理、AppContainer 可用性探测与 fail-closed）；Linux/macOS 代码随迁、交叉 `cargo check` 可选，实跑留 S12。
2. **run_command 工具**：审批提示呈现完整命令 + 风险等级；`ApprovedForRun` 对同 run 重复命令生效。
3. **取消链路**：Ctrl-C → `CancelHandle.cancel(User)` → 工具 cancel token + 进程树清理 → 事件收尾，全链一次打通。
4. **fail-closed**：沙箱后端探测失败 / 显式 `--sandbox off` 之外的任何异常 → 拒绝执行并事件化说明（绝不静默裸跑）。
5. **输出纪律**：命令输出只经工具结果/事件进入模型上下文（截断后），完整输出落工件文件（临时目录），为 S5 上下文预算减负、为 S7 artifact 化铺路。

## 真实测试与评估（冒烟清单）

核心场景（两通道各跑；建议用一个带一处编译错误的 fixture Rust 小项目）：

- [ ] **「读-改-跑」闭环**：「跑 `cargo check`，修复所有报错，再跑一次确认通过」——Agent 应 run_command → 读报错 → edit_file 修复 → 再 run_command 验证。记录两模型的闭环成功率与轮数（本阶段核心评估项）。
- [ ] 长命令（如 `cargo build`）执行中 Ctrl-C：命令进程与其子进程全部终止（任务管理器核对无残留）、run 以 `RunCancelled` 收尾、REPL 可继续。
- [ ] 危险命令审批：诱导 Agent 执行 `Remove-Item -Recurse` / `git push --force` 类命令 → `Dangerous` 级审批弹出；拒绝后 Agent 改用安全方案。
- [ ] 输出洪水：`type` 一个大文件 → 截断生效、上下文不被撑爆、提示已截断。
- [ ] 沙箱降级：人为使沙箱后端不可用（或注入探测失败）→ 命令被拒绝执行且解释清晰（fail-closed 实证）。
- [ ] 超时：`Start-Sleep 600` 类命令按约束超时终止、事件可见。

## 定向自动化测试

- `cargo test -p pawork-exec`：进程树清理回归（孙进程随树回收）、fail-closed 降级、超时终止（V1 测试随迁，Windows 实跑）。
- `cargo test -p pawork-tools`：run_command 超时/截断/退出码/stderr 分流；policy 决策矩阵（Safe 直通按模式、Dangerous 必询）。
- `cargo test -p pawork-policy`：shell 参数注入分类回归（V1 种子集）。
- `cargo test -p pawork-engine`：取消传播（MockProvider + 长驻假工具）→ 事件收尾顺序 golden。
- env 门控真实闭环（`--ignored`）：脚本化「读-改-跑」任务断言最终 `cargo check` 通过 + 事件流含完整链路。

## 退出标准

- [ ] 「读-改-跑」真实闭环在两通道各至少一次成功，评估记录留档（**V2 主干验收达成**）。
- [ ] 进程树清理 / fail-closed / 注入分类 / 超时截断回归全绿（当前平台）。
- [ ] Linux/macOS 平台代码齐全（结构就位，实跑 S12）。
- [ ] 取消链路端到端一次打通（人工 + 自动化双验证）。

## 为后续阶段预留 / 明确不做

- 预留：完整命令输出落工件文件的落点（S7 blob-store artifact 接管）；`AllowWithConstraints` 已消费。
- 不做：PTY / 交互式命令（S9）、后台长驻任务（S11 automation）、三平台沙箱实跑（S12）。

## 并行拆分建议

- 波 A（并行 ×2）：`pawork-exec` 迁移（最大件）；`pawork-tools` run_command + policy 接线。
- 波 B（串行）：engine 取消链路 + cli 渲染 + 装配。
- 波 C：真实闭环冒烟与评估（主代理）。

## 参考

- [../docs/design.md](../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../docs/references.md)（参照项目手册）
- [../ROADMAP.md](../ROADMAP.md) §2（阶段总览与关键节点）
- [archive/M1-execution-security.md](archive/M1-execution-security.md)（pawork-exec 迁移细则、安全红线）
- [archive/M4-engine-closed-loop.md](archive/M4-engine-closed-loop.md)（旧 M4 验收定义——本阶段兑现其核心项）
