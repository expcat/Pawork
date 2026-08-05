# P1-7：Workspace 服务

> Phase 1 · 基础设施 · 状态：🟢已完成 · 依赖：P0-2

**最终目的**：实现工作区管理（增删改 / 信任 / 多 root / Git 检测），为文件索引、资源加载、Git 提供统一的工作区语义。

**涉及范围**：`workspace-service`

## 细分步骤

1. **添加 / 删除 / 重命名工作区** —— 目的：基本生命周期。
2. **多 root 支持** —— 目的：多仓库场景。
3. **Git 检测与最近访问** —— 目的：识别版本库。
4. **信任状态接口** —— 与 P4-10 联动。目的：参与 Policy 决策。

## 主要产出物

- `workspace-service` crate

## 验收标准

- [x] 多 root 行为一致
- [x] Git 检测正确

**相关文档**：[workspace-index](../docs/features/workspace-index.md) · [policy](../docs/features/policy.md) · [ROADMAP](../ROADMAP.md)
