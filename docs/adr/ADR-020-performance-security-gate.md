# ADR-020：性能和安全测试是发布门槛

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

作为本地执行命令、改写文件、调度模型与插件的平台，性能与安全是可用性与信任的前提，不能事后补救。

## 决策

将性能目标与安全验收设为发布门槛：性能基准须区分 Rust Core / Git 子进程 / Provider 网络 / 模型生成 / 外部命令 / GUI 渲染；安全验收须通过 15 项必过项。

## 后果

- 任何 Phase 发布须对照性能与安全门禁。
- 测试体系（含 fuzz/chaos/差分）成为持续要求。
- 门禁失败即阻塞发布。

## 相关

- [性能目标](../quality/performance-targets.md) · [安全验收](../quality/security-acceptance.md) · [测试体系](../quality/testing.md) · [ROADMAP 横切门禁](../../ROADMAP.md)
