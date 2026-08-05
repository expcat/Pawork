# P11-3：Linux Bubblewrap

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟡未开始 · 依赖：P11-1

**最终目的**：实现 Linux Bubblewrap（bwrap）沙箱后端，为 Linux 提供容器级隔离。

**涉及范围**：`sandbox-runtime`

## 细分步骤

1. **bwrap 调用封装** —— 目的：Linux 原生沙箱。
2. **挂载/网络/进程隔离** —— 目的：受限执行。
3. **可用性检测（无 bwrap 时回退）** —— 目的：稳健。
4. **测试** —— 目的：限制生效。

## 主要产出物

- Linux Bubblewrap backend

## 验收标准

- [ ] Linux 下沙箱限制生效（或优雅回退）

**相关文档**：[sandbox](../docs/features/sandbox.md) · [ROADMAP](../ROADMAP.md)
