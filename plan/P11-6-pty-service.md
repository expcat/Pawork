# P11-6：PTY Service

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟡未开始 · 依赖：P4-12

**最终目的**：实现集成终端 PTY 服务（创建/resize/write/exit/kill/重连/有界缓冲），支持终端会话归属与自动清理。

**涉及范围**：`pty-service`

## 细分步骤

1. **PTY 创建/resize/write/exit/kill** —— 目的：终端基本操作。
2. **重连 + 有界缓冲** —— 目的：断连可恢复。
3. **session 归属与自动清理** —— 目的：资源不泄漏。
4. **三平台测试** —— 目的：一致可用。

## 主要产出物

- `pty-service` crate

## 验收标准

- [ ] session 归属正确、自动清理
- [ ] 重连可恢复缓冲

**相关文档**：[process](../docs/features/process.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：PTY 基础用 portable-pty，但其上游迭代慢——开工前先评估维护中的 fork（如 xpy/portable-pty-psmux）或 vendor 兜底；会话层（重连 / 有界缓冲 / 归属 / 自动清理）自实现在其上。
