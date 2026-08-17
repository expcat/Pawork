# S8：Git、Diff 与 Checkpoint

> 阶段 S8 · 版本控制与回滚 · 状态：🔵进行中（波 A ✅）· 依赖：S3（写工具在位；run_command 非必需）· 规模：中 ·（与 S5/S6 可并行；S7 GUI 非阻塞，但有 GUI 时应同步 Changes 面）

## 目标（本阶段结束时用户能做什么）

Agent 的改动变得可审阅、可撤销：`pawork diff` 以结构化 diff 呈现会话累计改动，写工具执行前自动创建 checkpoint 快照，`pawork rollback` 一键回滚到任意 checkpoint；git 状态感知（status/branch/worktree）进入上下文与 CLI；审批 UX 从「路径 + 行数预览」升级为真正的 diff 预览。

## 涉及包与 V1 资产

| V2 包（目录） | 本阶段动作 | V1 来源与方式 |
| --- | --- | --- |
| `pawork-git`（vcs/git） | 激活：V1 `git-service`（GitRunner async 系统 git、repo/status/stage/commit/branch/stash/history/conflict/worktree、StatusCache + watcher 失效）+ `diff-service`（unified diff 状态机 parser、`DiffFile/DiffHunk/DiffLine` model、paginate、hunk_stage）合并迁移；roots 由调用方注入（不依赖 pawork-workspace）；shell 参数注入防御（`validate_position_arg` 等）随迁 | 直接迁移（[archive/M3](archive/M3-storage-session.md) pawork-git 节全文适用） |
| `pawork-blob-store`（storage/blob） | 激活：V1 `artifact-store` + `protected-blob-store`（feature `protected`，AEAD）+ `checkpoint-service`（feature `checkpoint`，写快照/回滚）合并迁移；`PWB1` 格式 golden 先行；S4 预留的「完整命令输出落工件」接到 artifact | 直接迁移（[archive/M1](archive/M1-execution-security.md) pawork-blob-store 节） |
| `pawork-engine` | 增强：写工具执行前自动 checkpoint（`CheckpointCreated` 事件）、回滚（`CheckpointRolledBack` 事件）；两事件类型 S1 起已在位 | 接线 |
| `pawork-app` | 增强：装配 git（roots 注入）与 blob/checkpoint；会话改动集投影（哪些文件被本会话改过） | 接线 |
| `pawork-cli` | 增强：`pawork diff [--session <id>]`（结构化渲染 + 分页）、`pawork rollback [<checkpoint>]`（列出可选点、确认后回滚）、审批 UX 升级为 diff 预览（edit/apply_patch 审批时展示将产生的 hunk） | 新写 |

## 关键任务

1. **golden 先行**：`PWB1` 格式 golden、unified diff parser golden + proptest 种子（rename/binary/untracked/submodule/CRLF/无末尾换行/Unicode 文件名/分页）先迁先绿。
2. **git 迁移**：注入防御回归（stage/commit/branch 等用户输入路径）；worktree 创建/删除不动用户数据。
3. **checkpoint 闭环**：写前快照 → 改动 → 回滚 → 文件系统状态一致（含新建文件删除、被改文件还原）；checkpoint 与事件流互链（回滚后事件流如实记录，不抹历史）。
4. **diff 呈现链**：会话改动集 → git diff（工作区）或快照对比（非 git 目录）→ 结构化渲染；中文文件名/CRLF 正确。
5. **审批 diff 预览**：edit/apply_patch 的审批提示升级（S3 预留的升级点，接口不变）。

## 真实测试与评估（冒烟清单）

- [ ] 真实任务改 2–3 个文件（经审批）→ `pawork diff` 呈现全部 hunk（与 `git diff` 人工核对一致）→ `pawork rollback` → 文件还原、`git status` 干净 → 再 `pawork diff` 为空。
- [ ] 非 git 目录中同样的快照/回滚闭环可用。
- [ ] 含中文文件名 + CRLF 文件的 diff 正确呈现。
- [ ] 审批时的 diff 预览与最终落盘一致。
- [ ] git worktree 下运行会话：roots 正确、状态不串主工作区。
- [ ] 诱导注入：文件名形如 `--force`/`-o xx` 的路径经 git 操作不被解释为参数（防御实证）。
- [ ] **评估记录**：模型对 diff 上下文的利用（回滚后能否理解「改动已撤销」并重新规划）。

## 定向自动化测试

- `cargo test -p pawork-git`：注入防御回归、diff golden + 种子、status/stage/commit/worktree 冒烟（临时仓库）。
- `cargo test -p pawork-blob-store`（default 与 `--all-features`）：`PWB1` golden、protected 加密断言（密文不含明文片段）、checkpoint 快照/回滚闭环。
- `cargo test -p pawork-engine`：checkpoint 事件对 golden、回滚后事件流追加语义（append-only 不破）。

## 退出标准

- [ ] 冒烟全项通过；checkpoint 快照/回滚闭环（旧 M1 硬指标）达成且有真实消费者。
- [ ] `PWB1`/diff golden 全绿；git 注入防御回归全绿。
- [ ] `pawork-git` 不依赖 `pawork-workspace`（roots 参数化，`cargo tree` 断言）。
- [ ] 审批 diff 预览上线（S3 升级点兑现）。

## GUI 增量

按 [gui-design.md](../docs/gui-design.md) §5：在已有 Agent 壳上加 Changes（会话 diff、rollback、审批 hunk 预览）。无 Desktop 时本阶段 CLI 仍独立可验收。

## 为后续阶段预留 / 明确不做

- 预留：hunk/line 级暂存（`HunkStageService`）已迁入，消费者（GUI 交互式暂存）随本阶段 Changes 或 S10 补齐；ForgeAdapter 类评审能力属 S11 review。
- 不做：自动 commit / branch 管理 UX（按需求另议）、checkpoint 的远端同步。

## 并行拆分建议

- 波 A（并行 ×2）：`pawork-git`；`pawork-blob-store`（golden 先行）。
- 波 B（串行）：engine/app/cli 接线 + 审批预览升级 + 冒烟。

## 参考

- [../docs/design.md](../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../docs/references.md)（参照项目手册）
- [archive/M3-storage-session.md](archive/M3-storage-session.md)（pawork-git 迁移细则）
- [archive/M1-execution-security.md](archive/M1-execution-security.md)（pawork-blob-store 迁移细则）
