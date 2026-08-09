# P11-8：跨平台路径

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P4-9

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

- [x] `dunce` canonicalization 统一移除 Windows verbatim 前缀并解析 symlink/junction
- [x] component-aware root boundary 防字符串前缀碰撞，Windows 大小写不敏感
- [x] `.git` 大小写绕过与真实 Windows junction escape 有回归测试
- [x] policy-engine、git-service、resource-loader 与 sandbox-runtime 路径消费者统一使用该边界

## 验证记录（2026-08-09）

- Windows 原生：policy-engine 57 tests、git-service 64 tests、resource-loader 51 tests 通过，含真实 junction escape 与 `\\?\`/大小写用例。
- `process-runtime` / `sandbox-runtime` / `pty-service` 在 Linux GNU 与 macOS aarch64 目标检查通过；平台路径纯函数由共享 L0 覆盖。
- canonical check 不替代写入工具的原子/独占创建协议；TOCTOU 边界已在 [policy](../docs/features/policy.md) 明确。

**相关文档**：[policy](../docs/features/policy.md) · [sandbox](../docs/features/sandbox.md) · [ROADMAP](../ROADMAP.md)
