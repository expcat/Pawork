# P0-11：ADR 与文档基线

> Phase 0 · 架构与协议冻结 · 状态：🟢已完成（文档阶段交付）· 依赖：—

**最终目的**：完成 ADR-001~030 与文档骨架的定稿与交叉引用校验，使后续实现有据可依、零断链。ADR-021~030 落实「Core 与 CLI 一体化、GUI 经协议连接 CLI」的架构修正，并据以修订 ADR-001/006/017/019。本任务已在文档阶段基本交付，列为收尾确认项。

**涉及范围**：`docs/adr`、`docs`

## 细分步骤

1. **校对 ADR-001~030 内容与状态** —— 目的：每条决策可追溯。
2. **校验内部链接零断链** —— 目的：文档可达。
3. **同步 workspace-layout 与各 feature 文档术语** —— 目的：命名一致（CLI Host / GUI Connection Protocol / Transport / 多客户端等新术语）。
4. **核对架构修正一致性** —— 确认 overview/workspace-layout/api-surface/control-flow/cli-host/gui-connection 与 ADR-021~030 表述一致，无遗留「Core 与 GUI 同进程」「core-daemon/core-cli/core-rpc」描述。目的：单一事实源。

## 主要产出物

- 定稿 ADR + 文档骨架 + 链接校验报告

## 验收标准

- [x] ADR-001~030 内容完整、状态明确
- [x] 内部交叉引用零断链
- [x] 架构修正相关文档与 ADR 表述一致，无矛盾残留

**相关文档**：[ADR 目录](../docs/adr/) · [ROADMAP](../ROADMAP.md)
