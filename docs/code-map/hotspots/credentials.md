# 凭证与脱敏

明文 token 的允许停留点：`SecretBackend` 内部、adapter 瞬时 `expose_secret()`、受保护 AEAD 信封。其它地方必须是引用或 `[REDACTED]`。

## 解析链

1. `pawork-auth::locator`：env 名（`api_key_env_name`）、service 前缀 `pawork` / `pawork.mcp.`、文件名 `mcp-auth.json`。
2. `resolve_provider_credential`：`CredentialSource::{AuthFile, EnvFallback, None}`。env 仅 headless/CI fallback。
3. 宿主 `provider_assembly` 把 `ResolvedCredential` 注入通道适配器。`Debug` 脱敏、无 `Serialize`。
4. OAuth（ChatGPT PKCE / xAI Device）参数来自 `CHANNEL_REGISTRY` 的 `OAuthPreset`，不在 adapter 再写一份。client secret 不进仓库。

`FileBackend`：`$PAWORK_HOME/auth.json` 否则 `~/.pawork/auth.json`；`0600` + 原子写；损坏 fail-closed。OS Keychain 已删除。

## 日志与事件

- 二进制：`apps/pawork/src/redact.rs` 的 `RedactingFmtLayer` 覆盖全部 tracing 字段。
- Provider HTTP：错误路径不得拷贝 request body 进 `ProviderError.message`。
- Reasoning：事件只带 `ProtectedBlobRef`；明文在 PWB1（`storage` `protected` feature，宿主 `app/protected.rs` 注入）。
- MCP：独立后端文件，禁止与主 auth 混用。

## 配置

`PaworkConfig` / `ProviderConfig` **无 `api_key` 字段**；`extra` 会剥离该键。compat 导入遇到明文 Secret 拒绝。

模块图：[auth](../../../crates/auth/MODULE.md) · [providers](../../../crates/providers/MODULE.md) · [storage](../../../crates/storage/MODULE.md) · [apps/pawork](../../../apps/pawork/MODULE.md)
