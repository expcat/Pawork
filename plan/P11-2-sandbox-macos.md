# P11-2：macOS Sandbox profile

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟡未开始 · 依赖：P11-1

**最终目的**：实现 macOS sandbox-exec profile，为 macOS 提供系统级命令沙箱。

**涉及范围**：`sandbox-runtime`

## 细分步骤

1. **sandbox profile 生成** —— 目的：macOS 原生沙箱。
2. **路径/网络/进程策略** —— 目的：受限执行。
3. **与 NativeRestricted 协作** —— 目的：统一抽象。
4. **测试** —— 目的：限制生效。

## 主要产出物

- macOS Sandbox profile

## 验收标准

- [ ] macOS 下沙箱限制生效

**相关文档**：[sandbox](../docs/features/sandbox.md) · [ROADMAP](../ROADMAP.md)
