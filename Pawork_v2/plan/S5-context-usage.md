# S5：上下文预算与用量

> 阶段 S5 · 上下文与用量 · 状态：🔵进行中（2026-08-15 波 A 完成） · 依赖：S4 · 规模：中 ·（S5/S6/S7 相互无包级交叉，可并行）

## 目标（本阶段结束时用户能做什么）

长任务不再炸上下文：每轮组装前计算上下文预算，触及软/硬上限时自动截断/压缩（compaction）且对话仍连贯；CLI 显示每轮与累计 token 用量、按模型定价估算费用；模型目录（context window / max output / 定价）由共享 registry 提供，`glm-5.2`（约 1M 上下文）与小上下文模型行为都正确。修复 V1 两大「零消费者」缺口：context-engine 首次接入主循环、compaction-engine 首次有真实消费者。

## 涉及包与 V1 资产

| V2 包（目录） | 本阶段动作 | V1 来源与方式 |
| --- | --- | --- |
| `pawork-provider-core`（providers/core） | 激活：V1 `provider-runtime` 剩余模块（`usage`/`negotiate`/`capability`/`reasoning` trait；`stream_assembly` 若 S0 已提前迁则归位）+ V1 `model-registry` 整包（`ModelRegistry`/`CatalogEntry`/`ModelPricing` micros 定价/`estimate_cost`/别名解析）。reasoning protector 只留 trait（实现属宿主，激活推迟）；不依赖 blob store | 直接迁移（[archive/M2](archive/M2-providers.md) pawork-provider-core 节） |
| `pawork-engine` | 增强：V1 `context-engine` 并入为 `context` 模块并**真实接入 turn 组装**——预算计算（模型 context window 来自 registry）、触软限发压缩请求、触硬限截断；`CompactionStarted/Completed` 事件化；budget 模块对齐 V1 | 直接迁移 + 接线（修复 V1 未接主循环缺口） |
| `pawork-session` | 增强：`compaction` feature——V1 `compaction-engine`（engine/retention/snapshot）并入，`TokenEstimator` trait 由 engine 侧注入（依赖倒置，session 不依赖 engine） | 直接迁移（[archive/M3](archive/M3-storage-session.md) 关键动作 2） |
| `pawork-app` | 增强：registry 装配（`builtin_models()` 目录合并 + config `models` 覆盖 + 运行期 `/models` 探测合并）；TokenEstimator 注入 | 接线 |
| `pawork-cli` | 增强：每轮尾部用量行（输入/输出/累计 token、估算费用）；`pawork sessions show` 含会话累计用量；REPL `/compact` 手动触发压缩（与自动触发同一事件链） | 新写 |

## 关键任务

1. **provider-core 迁移**：usage 计量对齐 `ProviderStreamEvent::UsageUpdated`（S0 起已透传，本阶段开始聚合）；pricing 用 micros 整数，`BUILTIN_RATE_VERSION` 机制保留。
2. **context 接线**：预算输入 = 消息历史 + 工具输出（S4 已截断）+ 系统提示；软限触发 compaction、硬限截断策略事件化，重放后语义一致。
3. **compaction 闭环**：压缩产物作为 canonical 事件落流（`CompactionCompleted` 携带摘要引用），resume 后基于压缩态重建上下文。
4. **registry 目录**：GLM Coding Plan 与 OpenCode Go 的模型条目进 builtin 目录或 config 覆盖（含 context window：`glm-5.2` ≈ 1M、其他 ≈ 200k，以官方目录为准）；`pawork models` 显示 context window 与定价。
5. **费用估算**：无定价条目的模型显示 token 不显示费用（不编造）。

## 真实测试与评估（冒烟清单）

- [ ] 构造超长任务（连续读多个大文件并总结，人为把模型 context window 配小如 32k 加速触发）：软限触发压缩 → 对话继续且早期关键信息仍被引用；硬限不溢出、无 4xx 上下文超限错误。
- [ ] 压缩后 `--resume`：基于压缩态续聊连贯。
- [ ] REPL `/compact` 手动压缩：立即触发、事件流可见、压缩后继续对话正常。
- [ ] token 对账：一轮对话的 usage 与厂商控制台/响应 usage 字段抽查一致（GLM 与 OpenCode Go 各一次）。
- [ ] `pawork models`：两通道模型的 context window / 定价显示正确；`glm-5.2` 大上下文条目正确。
- [ ] **评估记录**：压缩对任务质量的影响（压缩前后回答对比）、两模型在接近满上下文时的行为差异。

## 定向自动化测试

- `cargo test -p pawork-provider-core`：registry 解析/别名/`estimate_cost` 回归；usage 聚合。
- `cargo test -p pawork-engine`：预算计算边界（软/硬限）、压缩/截断触发的事件 golden、重放一致性。
- `cargo test -p pawork-session --features compaction`：compaction round-trip、retention；默认 feature 不拉入 engine 链（`cargo tree` 断言）。
- MockProvider 长对话仿真：注入大量轮次断言永不超硬限。

## 退出标准

- [ ] 冒烟全项通过；V1 两个零消费者缺口（context-engine、compaction-engine）在装配链上有真实调用点。
- [ ] `CompactionStarted/Completed` 事件重放一致；compaction 为 feature 门控且默认路径不拉引擎链。
- [ ] token/费用显示与厂商侧抽查一致；无定价模型不编造费用。

## 为后续阶段预留 / 明确不做

- 预留：usage 聚合的会话级投影为 S11 `usage-ledger`/`dedup_key` 铺路（本阶段不建控制面表）；reasoning trait 在位待宿主实现。
- 不做：`pawork usage` 配额子命令（S11 quota）、多租户计量（S11）、prompt cache 偏好优化（S6 随厂商完整化）。

## 并行拆分建议

- 波 A（✅ 2026-08-15）：`pawork-provider-core`（usage/registry）；`pawork-session` compaction feature。
- 波 B（下一波，串行）：engine context 接线（依赖波 A trait）+ app/cli 收口。
- 与 S6、S7 无包级交叉，可整阶段并行推进。

## 参考

- [../docs/design.md](../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../docs/references.md)（参照项目手册）
- [archive/M2-providers.md](archive/M2-providers.md)（provider-core 拆分细则）
- [archive/M3-storage-session.md](archive/M3-storage-session.md)（compaction/TokenEstimator 细则）
- [archive/M4-engine-closed-loop.md](archive/M4-engine-closed-loop.md)（context 接入主循环的目标形态）
