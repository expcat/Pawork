# P12-5：结果聚合 / patch merge / 冲突检测

> Phase 12 · Multi-Agent · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P12-3（复用 `diff-service`）

**最终目的**：实现 Worker 结果聚合、patch merge 与冲突检测，由 Parent 决定是否合并，保证多 Agent 改动可审查。

**涉及范围**：`orchestration`、`checkpoint-service`

## 细分步骤

1. **结果聚合** —— 目的：汇总 worker 产出。
2. **patch merge** —— 目的：合并改动。
3. **冲突检测 + 文件锁** —— 目的：安全合并。
4. **parent 决定是否合并** —— 目的：人/父代理在环。

## 主要产出物

- 结果聚合与合并

## 验收标准

- [ ] parent 可审查 worker patch 并决定合并
- [ ] 冲突可检测

**相关文档**：[multi-agent](../docs/features/multi-agent.md) · [checkpoint](../docs/features/checkpoint.md) · [ROADMAP](../ROADMAP.md)
