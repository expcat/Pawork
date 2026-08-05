# P11-8：跨平台路径

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟡未开始 · 依赖：P4-9

**最终目的**：实现跨平台路径规范化与 symlink/junction 策略，保证三平台路径处理一致且安全。

**涉及范围**：多 crate

## 细分步骤

1. **路径规范化** —— 目的：统一表示。
2. **symlink / junction 策略** —— 目的：不跟随越界。
3. **大小写与分隔符** —— 目的：跨平台一致。
4. **三平台测试** —— 目的：一致。

## 主要产出物

- 跨平台路径工具

## 验收标准

- [ ] 三平台路径处理一致、symlink/junction 安全

**相关文档**：[policy](../docs/features/policy.md) · [sandbox](../docs/features/sandbox.md) · [ROADMAP](../ROADMAP.md)
