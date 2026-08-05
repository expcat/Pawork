# P4-4：apply_patch

> Phase 4 · 核心工具与权限 · 状态：🟡未开始 · 依赖：P4-2

**最终目的**：实现多文件 apply_patch（create/delete/rename、dry run、原子、部分失败回滚），让 Agent 能一次性安全地应用多文件改动。

**涉及范围**：`builtin-tools`、`checkpoint-service`

## 细分步骤

1. **多文件 create/update/delete/rename** —— 目的：覆盖完整改动语义。
2. **dry run** —— 目的：预演不落盘。
3. **原子提交** —— 目的：全成功才生效。
4. **部分失败回滚 + 路径安全** —— 目的：不留下中间态、防穿越。

## 主要产出物

- apply_patch 工具

## 验收标准

- [ ] 多文件原子提交
- [ ] 部分失败回滚
- [ ] 路径安全（无穿越）

**相关文档**：[tools](../docs/features/tools.md) · [checkpoint](../docs/features/checkpoint.md) · [policy](../docs/features/policy.md) · [ROADMAP](../ROADMAP.md)
