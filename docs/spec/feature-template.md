# Feature Spec 模板

> 仅在候选已经转正、且跨任务共享的信息足够多时复制本模板。单一小功能直接写入任务书，不建立空 Spec。删除所有占位说明后方可标记 Ready。

## 元数据

| 字段 | 值 |
| --- | --- |
| Feature ID / 名称 | `<ID> / <名称>` |
| 状态 | Draft / Review / Accepted / Implementing / Verified / Deferred |
| Owner | `<主责>` |
| 目标版本/阶段 | `<版本与阶段；未立项不得填写虚假编号>` |
| 最近更新 | `YYYY-MM-DD` |
| 关联 ROADMAP / 任务书 / ADR | `<相对链接>` |

## 1. 问题、用户与目标

- 目标用户：
- 当前问题与可复验证据：
- 用户场景/JTBD：
- 成功指标（可测量）：
- 非目标：

## 2. 当前状态与差距

| 能力 | 当前生产路径/证据 | 缺口 | 结论 |
| --- | --- | --- | --- |
| `<能力>` | `<源码、命令、运行证据>` | `<真实缺口>` | Implemented / Partial / Candidate |

说明归档资产位置（git/tag/历史文档）和复活条件；不得把历史代码等同当前实现。

## 3. 用户流程与需求

### 主流程

1. `<用户动作>`
2. `<系统响应>`
3. `<成功/失败终态>`

### 需求

| ID | 要求 | 优先级 | 验收证据 |
| --- | --- | --- | --- |
| `<FEAT>-001` | `<可观察、可测试的行为>` | Must/Should/Could | `<自动化/真实/人工>` |

明确空状态、错误、取消、重试、断线/恢复、并发和幂等行为。

## 4. 架构与契约影响

- 写入集与当前消费者：
- 依赖方向/架构红线：
- domain/API/wire/schema/config/CLI 变化：
- 版本、兼容、旧数据迁移与回滚：
- golden/fixture/typegen 先行清单：
- 是否需要 ADR，状态为何：

不复制现有完整 API/DDL；链接到 [contracts.md](contracts.md) 和精确事实源。

## 5. 安全与隐私

- 资产、攻击者能力与信任边界：
- Secret 来源、内存/落盘/日志生命周期：
- ToolDescriptor、Policy、approval mode 与 fail-closed 行为：
- workspace path、symlink、`.git`、TOCTOU：
- Sandbox/进程/网络/PTY：
- 审计、删除、保留期限与数据导出：
- 对应安全回归：

## 6. Desktop / CLI / 客户端

- CLI 命令、参数、TTY/JSON 行为：
- Desktop IA、状态、焦点/键盘/IME/a11y：
- GUI/headless/ACP registry 与 capability：
- 响应式、长列表/虚拟化、错误/空态：
- 人工验收步骤与截图基准：

无 UI 影响时明确写 `none`，不要创建占位界面。

## 7. 实现切片

| 切片 | 写入集 | 前置 | 完成条件 | 可并行性 |
| --- | --- | --- | --- | --- |
| `<A>` | `<包/文件>` | `<ADR/golden/其它切片>` | `<可验证结果>` | 串行/并行 |

每个切片应在数小时内可独立完成、独立验收；共享文件由单一 owner 管理。

## 8. 验证计划

| 需求 ID | E1 实现证据 | E2 自动化 | E3 真实环境 | E4 人工/发布 |
| --- | --- | --- | --- | --- |
| `<ID>` | `<路径>` | `<命令/种子>` | `<Provider/OS/客户端>` | `<签字/门禁>` |

必须列出三类关键回归中受影响的类别、测试数据/Secret 隔离、flake 处置和未覆盖项。不得预填“通过”。

## 9. 运行、迁移与回滚

- 配置/feature flag/默认值：
- 数据迁移与向后兼容：
- 部署/启用顺序：
- 观测指标与诊断：
- 失败回滚和用户数据保护：
- 发布任务（若适用）的 License/三平台/供应链前置：

## 10. 文档与收尾

- [ ] `capabilities.md` / `product.md` 状态同步
- [ ] `contracts.md` / `security.md` / `desktop.md` / `operations.md` 按影响同步
- [ ] `docs/design.md`/`docs/architecture.md`、ADR、对应包级 Spec（`docs/spec/crates/`）、ROADMAP/任务书同步
- [ ] 实际验证与已知缺口记录
- [ ] 候选从 [backlog.md](backlog.md) 转为已实现或明确延期

## 11. 决策与开放问题

| ID | 问题/决策 | 选项与取舍 | Owner/时点 | 状态 |
| --- | --- | --- | --- | --- |
| `<D1>` | `<问题>` | `<A/B/C>` | `<人/日期>` | Open/Accepted/Superseded |

