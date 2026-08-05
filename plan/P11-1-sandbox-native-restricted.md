# P11-1：NativeRestricted backend

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟡未开始 · 依赖：P4-9、P4-12

**最终目的**：实现 NativeRestricted 沙箱后端（workspace 路径限制、env 清洗、资源限制、网络策略提示），为命令执行提供可控安全边界，未通过审批的命令受限运行。

**涉及范围**：`sandbox-runtime`

## 细分步骤

1. **workspace 路径限制** —— 目的：限制可访问范围。
2. **env 清洗** —— 目的：移除敏感变量。
3. **资源限制** —— 目的：CPU/内存/时间。
4. **网络策略提示 + 命令审批** —— 目的：安全可控。

## 主要产出物

- NativeRestricted backend

## 验收标准

- [ ] 命令审批与路径限制生效

**相关文档**：[sandbox](../docs/features/sandbox.md) · [policy](../docs/features/policy.md) · [ROADMAP](../ROADMAP.md)
