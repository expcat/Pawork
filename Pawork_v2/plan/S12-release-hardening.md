# S12：Release Hardening 与发布

> 阶段 S12 · 收口与发布 · 状态：⚪未开始 · 依赖：S0–S11 全部完成 · 规模：大（验证 + 发布 + 归档，无新功能）

## 目标

开发期（S0–S11）明确「不做」的全部门禁一次性补回并跑至全绿，W1→W4 十五个高外部价值包按依赖方向发布 crates.io，V1 目录归档冻结。**执行清单沿用归档的 [archive/M8-release-hardening.md](archive/M8-release-hardening.md) 全文**（八项验证清单、五步发布流程、V1 归档动作、退出标准），本文件只记录增量计划带来的差异与补充。

## 相对旧 M8 的差异与补充

1. **编号与引用**：旧文中「M0–M7 交付」对应本计划 S0–S11；「里程碑退出标准」的引用按 [../ROADMAP.md](../ROADMAP.md) §2 阶段表理解。包集合、发布波次（W1–W4）、冻结候审清单均不变。
2. **真实通道回归**：三平台矩阵之外，增加一轮**双通道真实冒烟总回归**——用 GLM Coding Plan 与 OpenCode Go 把 S0–S11 各阶段冒烟清单的核心项（对话/resume/工具闭环/审批/回滚/MCP/SDK/多 Agent）串成一个发布前手工回归脚本并留档。
3. **`--json`/headless 协议**：S10 已对齐正式协议，schema drift 检查覆盖 headless 帧与 `.d.ts`/JSON Schema 产出（旧 M8 第 4 节照常执行）。
4. **env fallback 安全复核**：S6 引入的「Keychain 为主、env fallback」在安全验收清单（旧 M8 第 8 节）中追加一项：fallback 路径不落盘、不入日志、文档明示适用场景（headless/CI）。
5. **评估记录汇总**：S0–S11 沿途的模型评估记录（tool-calling 可靠性、闭环成功率、协议对比、多 Agent worker 对比）汇总为一份《双通道模型评估报告》，作为默认模型推荐与文档 FAQ 的依据（新增产出物，非门禁）。
6. **experimental 清账**：S11 登记的 experimental 项（如 memory 待 EmbeddingProvider、provider-control 完整层）逐项决策：激活、保留登记、或移入冻结候审。
7. **性能基准按需重建**：V1 `benches` 全为 no-op 占位、不迁移；如需性能验收（V1 MVP「性能达 Core 目标」项），在本阶段以 `pawork-benches` 重建针对性基准（事件追加吞吐、diff 解析、SSE 解析），非门禁项。

## 验证清单（沿用旧 M8 八项，逐项链接）

- [ ] 1. workspace 全量 build/test/clippy/fmt 四件套 + feature 关键组合矩阵（[archive/M8 §1](archive/M8-release-hardening.md)）
- [ ] 2. 三平台真实 runner 矩阵 + sandbox/PTY/Named Pipe 定向（[archive/M8 §2](archive/M8-release-hardening.md)）——S4/S10 留待的 Linux/macOS 实跑在此兑现；含 S7 Desktop 最小窗口在开发机之外的补测
- [ ] 3. cargo-fuzz 五目标（路径解析/unified diff/shell 分类/SSE/partial-JSON）（[archive/M8 §3](archive/M8-release-hardening.md)）
- [ ] 4. schema/typegen drift 接回 CI + 协议版本映射一致（[archive/M8 §4](archive/M8-release-hardening.md)）
- [ ] 5. 依赖卫生（machete/udeps/audit）+ 依赖方向 lint（canonical 纯净、rmcp/wasmtime 锁定）（[archive/M8 §5](archive/M8-release-hardening.md)）
- [ ] 6. license 拍板 + cargo-deny licenses + inventory（[archive/M8 §6](archive/M8-release-hardening.md)）
- [ ] 7. W1–W4 十五包 `cargo publish --dry-run` 零错误（[archive/M8 §7](archive/M8-release-hardening.md)）
- [ ] 8. 安全验收清单集中回归 + 三平台复跑（含本文差异 4）（[archive/M8 §8](archive/M8-release-hardening.md)）

## GUI 增量

按 [gui-design.md](../docs/gui-design.md) §5：三平台窗口/输入/打包证据。不是新功能页，不换壳。

## 发布与归档

- [ ] W1→W4 波次发布（波内并行、波间串行，五步流程照旧）；`apps/pawork` 经 `cargo install` 渠道冒烟。
- [ ] V1 目录归档冻结（归档分支/tag + 冻结候审资产索引），处置结论回写 [../ROADMAP.md](../ROADMAP.md) §4。
- [ ] 双通道真实冒烟总回归通过并留档；《双通道模型评估报告》产出。
- [ ] experimental 清账完成。

## 参考

- [../docs/design.md](../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../docs/references.md)（参照项目手册）
- [archive/M8-release-hardening.md](archive/M8-release-hardening.md)（执行主文档，全文有效）
- [../docs/design.md](../docs/design.md) §7（发布策略）· [../ROADMAP.md](../ROADMAP.md) §4（未决事项）
