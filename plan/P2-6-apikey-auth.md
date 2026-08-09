# P2-6：API Key 认证

> Phase 2 · 首个真实 Provider · 状态：🟢已完成 · 依赖：P0-4

**最终目的**：实现 API Key 认证方式，明文 token 存 OS Keychain 不落库，为首个真实 provider 提供鉴权（ADR-014）。

**涉及范围**：`auth-service`

## 细分步骤

1. **API Key 认证方式** —— 目的：基础鉴权。
2. **OS Keychain 存取** —— 目的：明文不落库。
3. **脱敏状态（显示名/尾号）** —— 目的：UI 可识别不泄漏。
4. **DB 只存 Credential ID / reference** —— 目的：不存明文。

## 主要产出物

- `auth-service` API Key 方式 + Keychain 后端

## 验收标准

- [x] 明文 token 不写入数据库与日志

**相关文档**：[auth](../docs/features/auth.md) · [ADR-014 Secret Keychain](../docs/adr/ADR-014-secret-os-keychain.md) · [ROADMAP](../ROADMAP.md)
