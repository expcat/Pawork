# P4-9：Policy Engine

> Phase 4 · 核心工具与权限 · 状态：🟢已完成 · 依赖：P0-5

**最终目的**：实现 Policy Engine（ApprovalMode、Policy 输入/输出、文件路径安全、Shell 高风险识别），为所有写/命令操作提供统一决策，是安全边界的核心。

**涉及范围**：`policy-engine`

## 细分步骤

1. **ApprovalMode 与 Policy 决策** —— 目的：可配置审批策略。
2. **文件路径安全** —— 穿越/junction/UNC/TOCTOU/`.git` 防护。目的：防越权写。
3. **Shell 高风险识别** —— 目的：危险命令需审批。
4. **安全验收对齐** —— 目的：满足安全验收项。

## 主要产出物

- `policy-engine` crate

## 验收标准

- [x] 路径穿越/junction/UNC/TOCTOU 防护有效
- [x] 高风险命令可触发审批

**相关文档**：[policy](../docs/features/policy.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)
