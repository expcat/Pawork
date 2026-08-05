# P11-4：Windows AppContainer / Job Object

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟡未开始 · 依赖：P11-1

**最终目的**：实现 Windows AppContainer / Job Object 沙箱后端，为 Windows 提供进程级隔离。

**涉及范围**：`sandbox-runtime`

## 细分步骤

1. **AppContainer 封装** —— 目的：Windows 原生沙箱。
2. **Job Object 资源与进程控制** —— 目的：受限执行。
3. **能力/路径限制** —— 目的：可控。
4. **测试** —— 目的：限制生效。

## 主要产出物

- Windows Sandbox backend

## 验收标准

- [ ] Windows 下沙箱限制生效

**相关文档**：[sandbox](../docs/features/sandbox.md) · [ROADMAP](../ROADMAP.md)
