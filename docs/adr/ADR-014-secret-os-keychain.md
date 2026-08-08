# ADR-014：Secret 存储在 OS Keychain

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

API Key、OAuth Token 等明文 Secret 若落入数据库或日志会带来泄露风险。

> 适用范围（2026-08-08 收窄）：本 ADR 仅限**小型、长期、用户级凭证**（API Key / OAuth Token / Refresh Token）。体积大、频次高、需 retention/GC/compaction 兼容的敏感制品（如 reasoning 加密凭证）不入 OS Keychain，改用 [ADR-032 Protected Blob Store](ADR-032-protected-blob-store.md)。

## 决策

Secret 存储优先级：OS Keychain > 用户指定后端 > 环境变量 > 加密配置文件 > 临时 Session Credential。SQLite 只存 Credential ID、Provider、显示名、过期、Scope、Keychain reference、脱敏状态，不存明文。日志自动脱敏。

## 后果

- 明文 Secret 不落库、不进日志。
- 跨平台需各自实现 Keychain 访问。
- 安全验收包含「Secret 日志泄漏测试」。

## 相关

- [auth](../features/auth.md) · [observability](../features/observability.md) · [安全验收](../quality/security-acceptance.md)
- [ADR-032 Protected Blob Store](ADR-032-protected-blob-store.md)（reasoning 等敏感制品）
