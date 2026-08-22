# pawork-auth

Secret 后端、凭证解析与 OAuth。依赖 `pawork-domain`。ADR-039 不合并清单成员。

## 职责

明文 token 只活在 `SecretBackend`（auth 文件或测试用内存）里；对外给出脱敏视图、默认账户读写、以及 PKCE / Device 的 OAuth 流程。R5 起 `locator` 是 env 名、service 前缀、MCP 域隔离的单一事实源。OS Keychain 已移除。

## 模块树

```
src/
  lib.rs
  backend.rs / file_backend.rs   # SecretBackend / FileBackend
  credential.rs / default_credential.rs
  locator.rs                     # pub mod
  oauth.rs                       # pub mod
  resolve.rs / masked.rs / error.rs / base64url.rs
```

无 `tests/` 目录；回归在各文件 `#[cfg(test)]`。

## 对外入口/API 面

`pub mod locator`、`pub mod oauth`；其余经 crate 根 `pub use`。

- **后端**：`SecretBackend`（`store` / `get` / `delete` / `store_batch`）、`FileBackend`、`MemoryBackend`。
- **凭证**：`StoredCredential`（字段 `secret_service` / `secret_account`，serde alias 可读旧 `keychain_*`）、`ApiKeyCredential`、`MaskedCredential`、`CredentialSource::{AuthFile, EnvFallback, None}`。
- **解析**：`resolve_provider_credential`、`store_default_api_key` / `delete_default_api_key`；OAuth 默认账户一组 `load_default_oauth_*` / `store_default_oauth_token` / `refresh_default_oauth_credential_if_needed`。
- **locator**：`secret_service_for`、`oauth_secret_service`、`api_key_env_name`、`read_api_key_from_env`；常量 `PROVIDER_SERVICE_PREFIX = "pawork"`、`MCP_SERVICE_PREFIX = "pawork.mcp."`、`MCP_AUTH_FILE_NAME = "mcp-auth.json"`。
- **oauth**：`start_pkce_flow` / `exchange_pkce_code` / `request_device_authorization` / `poll_device_token` / `refresh_access_token` 等；`TokenSet`、`Pkce`、`CallbackServer`。

`generate_credential_id` 等为模块内 `pub`、**不是** crate 根可见。

## 依赖与被依赖

- **依赖**：`pawork-domain`。`reqwest` / `directories` / `sha2` / `getrandom`。无 feature。
- **被依赖**：`pawork-app`、`pawork-tools`（MCP OAuth）。二进制经 app/cli 间接使用。

## 红线与注意事项

- 明文 token **不得**进入 `StoredCredential` 字段语义之外的落盘形状、事件、日志；`MaskedCredential` 的 `Debug`/`Display`/`Serialize` 无明文。`TokenSet` / `Pkce` 等 Debug 脱敏。
- `FileBackend`：默认 `$PAWORK_HOME/auth.json` 否则 `~/.pawork/auth.json`；`0600` + 原子 rename；损坏 fail-closed；`FORMAT_VERSION = 1`。
- MCP 凭证走独立 `mcp-auth.json` 与 `pawork.mcp.*` service 前缀，禁止与主 auth 混用。
- 测试只用 `MemoryBackend` 或显式临时 `FileBackend`，不读真实用户 auth 文件。
- `keychain_*` serde alias 是一个版本期的读兼容；auth.json entries 落盘键本就不是该词汇。

## 相关文档

- [docs/design.md](../../docs/design.md) §2 / §4 S6
- [plan/R5-provider-neutrality.md](../../plan/R5-provider-neutrality.md)
- [AGENTS.md](../../AGENTS.md) §2（Secret）
- [代码地图总索引](../../docs/code-map/README.md)
