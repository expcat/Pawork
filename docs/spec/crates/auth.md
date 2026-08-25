# pawork-auth

> 身份认证与 Secret 管理：Secret 存储后端（文件/内存）、凭证解析链、全局脱敏与 OAuth（PKCE / Device Flow / auto refresh / 本地回调）；只依赖 `pawork-domain`（见 [domain.md](domain.md)），被 `pawork-app` 与 `pawork-tools`（MCP OAuth）依赖。

## 1. 职责与边界

- **职责**：`SecretBackend` 抽象与两个实现（生产 `FileBackend`、测试 `MemoryBackend`）；API key 与 OAuth 凭证的存取/元数据（`StoredCredential` / `ApiKeyCredential` / default OAuth 条目）；凭证解析链 `resolve_provider_credential`（auth 文件 → env fallback → 无凭证）；OAuth 三流程（PKCE 授权码、Device Flow、refresh）与一次性本地回调服务器；脱敏（`MaskedCredential`）；命名单一事实源（`locator`）；本地 `base64url` 编解码。
- **不做**：`pawork auth` CLI 接线、config 凭证引用、六通道装配与 `auth list` 展示（`pawork-app` / workspace config 承载）；OAuth 端点预设数据（`pawork-providers` 的 `CHANNEL_REGISTRY`，见 [providers.md](providers.md)）；reasoning blob 加密的 master key（`pawork-app` protected 模块）。
- **核心红线**：明文 token 绝不进入 `StoredCredential` / `ApiKeyCredential` 的可序列化字段，只存于 `SecretBackend`；一切错误、日志、`Debug` 输出不携带明文。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~50 | crate 门面：红线说明；`locator` / `oauth` 为 pub 模块，其余私有模块 + 选择性 re-export |
| `src/error.rs` | ~60 | `AuthError`：`Storage` / `NotFound` / `InvalidSecret` / `MalformedMetadata` / `OAuth` / `TokenEndpoint{error,description}` / `ExpiredToken` / `Callback` / `Http` / `Io` / `Url`；任何变体 Display 不含明文 |
| `src/backend.rs` | ~200 | `SecretBackend` trait（`store` / `store_batch` / `get` / `delete` / 隐藏扩展点 `refresh_lock_path`）；`MemoryBackend`（测试用，故意不派生 Debug） |
| `src/file_backend.rs` | ~580 | `FileBackend`：单 JSON 文件（`version` + `service→account→secret`）、0600、独立临时文件 + rename 原子写、跨进程 write/refresh 锁、损坏 fail-closed；`try_acquire_file_lock` / `FileLockGuard`（crate 内共用） |
| `src/locator.rs` | ~70 | 命名单一事实源：`PROVIDER_SERVICE_PREFIX`（`pawork`）、`MCP_SERVICE_PREFIX`（`pawork.mcp.`）、`MCP_AUTH_FILE_NAME`（`mcp-auth.json`）、`secret_service_for` / `oauth_secret_service` / `is_mcp_secret_service` / `api_key_env_name` / `read_api_key_from_env` |
| `src/masked.rs` | ~110 | `MaskedCredential`：`mask`（按字符数分档脱敏）/ `from_masked` / `as_str`；`Display`/`Debug`/`Serialize` 永不含明文 |
| `src/credential.rs` | ~390 | `StoredCredential`（纯元数据 + 定位，可序列化）、`ApiKeyCredential`（store / store_with_scopes / from_stored / resolve / delete）、`CredentialId`、crate 内 `generate_credential_id` |
| `src/resolve.rs` | ~240 | `resolve_provider_credential` 解析链、`CredentialSource`（AuthFile / EnvFallback / None）、`store_default_api_key` / `delete_default_api_key`、`PROVIDER_DEFAULT_ACCOUNT` |
| `src/default_credential.rs` | ~670 | 每 provider 唯一 default OAuth 条目（`default.access` / `.refresh` / `.meta` 三账户）：store/load/update/delete、`DefaultOAuthMeta`（含 ChatGPT `account_id` claim 提取）、`refresh_default_oauth_credential_if_needed` / `default_oauth_needs_refresh` |
| `src/oauth.rs` | ~1.9k | PKCE（`Pkce` / `start_pkce_flow(_with_callback)` / `exchange_pkce_code`）、Device Flow（`request_device_authorization` / `poll_device_token`）、refresh（`refresh_access_token` / singleflight `refresh_oauth_credential_if_needed` / `resolve_oauth_credential_for_request`）、`store_oauth_token` / `update_oauth_token` / `read_refresh_token` / `needs_refresh`、`CallbackServer`、`TokenSet`、crate 内 `decode_jwt_payload` |
| `src/base64url.rs` | ~240 | 本地 base64url（URL-safe、无填充）`encode` / `decode` 与 `Base64UrlDecodeError`；拒绝非规范输入（填充、`len%4==1`、余位非零） |

共 11 个 `.rs` 文件，约 4.5k 行；无独立 `tests/` 目录，回归全部内联在各文件 `#[cfg(test)]`。

## 3. 对外 API 面

### 3.1 Secret 后端

- `SecretBackend`（`Send + Sync`）：以 `(service, account)` 定位。`store` / `get`（缺失报 `NotFound`）/ `delete`；`store_batch` 默认逐条写，正式后端（FileBackend）覆写为单次原子提交；`refresh_lock_path()`（`#[doc(hidden)]` 扩展点）返回跨进程 refresh 锁路径，默认 `None`。
- `FileBackend::new()`：默认路径 `$PAWORK_HOME/auth.json`，未设时 `~/.pawork/auth.json`；`with_path`（测试）；`path()` 诊断（不含 secret）。
- 文件格式：`{ version: 1, entries: { service: { account: secret } } }`；只接受 `FORMAT_VERSION = 1`，版本不符 fail-closed。
- 锁文件（与 auth 文件同目录、非机密）：写锁 `auth.write.lock`（10ms 重试、30s 超时）；OAuth refresh 锁 `auth.refresh.lock`（经 `refresh_lock_path` 暴露给 refresh 编排）。
- `MemoryBackend::new()` / `len` / `is_empty`：进程内 HashMap，仅单元测试用；故意不派生 Debug 防明文入断言输出。
- OS Keychain 后端已按用户决策移除；secret 统一走文件后端（参照 Codex CLI auth.json 形态）。

### 3.2 命名与定位（locator 单一事实源）

- service：Provider API key 用 `pawork.<provider>`（`secret_service_for`）、OAuth 用 `pawork.<provider>.oauth`（`oauth_secret_service`）、MCP 用 `pawork.mcp.` 前缀（`is_mcp_secret_service` 判定）；MCP 凭证文件名常量 `MCP_AUTH_FILE_NAME = "mcp-auth.json"`。
- env fallback 名：`api_key_env_name(provider_id)` → `PAWORK_API_KEY_<ID 大写、- 换 _>`（如 `glm-coding` → `PAWORK_API_KEY_GLM_CODING`）；`read_api_key_from_env` 读取（空串视为未设）。

### 3.3 凭证类型与解析

- `StoredCredential`：`masked` / `id` / `provider` / `display_name` / `secret_service` / `secret_account` / `created_at` / `expires_at` / `scopes`——纯元数据可序列化（`Debug` 与 JSON 均无明文）；`new(...)` + `with_expires_at` 构造，`secret_backend_ref()` 返回定位二元组。
- `ApiKeyCredential::store(_with_scopes)`（写后端 + 返回元数据）/ `from_stored`（校验形态）/ `resolve`（→ `ResolvedCredential`，`CredentialKind::ApiKey`）/ `delete`。
- `resolve_provider_credential(backend, provider_id) -> Result<CredentialSource, AuthError>`：见 §4.1。`store_default_api_key` / `delete_default_api_key` 操作主条目（account 固定 `default`，删除幂等）。
- default OAuth 条目（每 provider 唯一，`OAUTH_DEFAULT_ACCOUNT = "default"`）：
  - 写：`store_default_oauth_token`（`default.access` / `.refresh` / `.meta` 三账户一次 `store_batch`）；`update_default_oauth_token`（refresh 后轮换写回）；`delete_default_oauth_token`。
  - 读：`load_default_oauth_credential`（由 meta 无网络重建 `StoredCredential`）与 `load_default_oauth_meta`（`auth list` 展示用）；条目不存在返回 `None`，由调用方 fail-closed。
  - 刷新：`refresh_default_oauth_credential_if_needed`、`default_oauth_needs_refresh`（内置 30s grace）。
  - `DefaultOAuthMeta { masked, created_at_ms, expires_at_ms, scopes, account_id }`：非机密 JSON（可打印、存 meta 账户）；meta 损坏报 `MalformedMetadata`。

### 3.4 OAuth 流程

- 类型：`PkceFlowConfig`（client_id / auth_url / token_url / redirect_uri / scopes / provider / extra_auth_params）与 `PkceSession`；`Pkce`（S256；verifier = 48 随机字节的 base64url，恰 64 字符，满足 RFC 7636 43–128 且无取模偏差）；`DeviceFlowConfig` / `DeviceUserPrompt`（user_code / verification_uri(_complete) / device_code / expires_in / interval）；`OAuthRefreshConfig { token_url, client_id, refresh_skew }`；`TokenSet { access_token, refresh_token?, id_token?, expires_in?, token_type, scope? }`（明文只短暂在内存，Debug 全脱敏，绝不落盘）。
- 入口：`start_pkce_flow` / `start_pkce_flow_with_callback`（绑定一次性 `CallbackServer` 并回填实际端口）/ `exchange_pkce_code`（state 不符即 CSRF 拒绝）；`request_device_authorization` / `poll_device_token`（`authorization_pending` 续轮询、`slow_down` interval +5s、`expired_token` 或超出 max_duration → `ExpiredToken`）；`refresh_access_token`；`store_oauth_token` / `update_oauth_token`（多凭证形态，service=`pawork.<provider>.oauth`，account=`<cred_id>.access/.refresh`；空 access/refresh 先拒绝且不产生部分写入；轮换场景整批 `store_batch` 提交，兼容后端至少先写 refresh 再写 access）；`resolve_oauth_credential(_for_request)`（后者先 auto-refresh 再返回 `CredentialKind::OAuthBearer`）；`read_refresh_token`（缺失归一 `NotFound`）；`needs_refresh(stored, skew)`（无 `expires_at` 视为不需刷新）；`random_state`（32 随机字节 base64url）。
- `CallbackServer::start(port)`（监听 127.0.0.1）/ `local_addr` / `bind_redirect_uri`（强制 http + loopback host、端口回填校验）/ `wait_for_code(timeout)`；单连接、5 分钟 accept 上限、请求头 64 KiB 上限、响应固定纯文本（不回显 query 输入）。
- 错误：token endpoint 标准错误归一为 `TokenEndpoint { error, description }`；其余流程错误 `OAuth(String)` / `Callback(String)`，都不含 token。

### 3.5 脱敏与编码

- `MaskedCredential::mask` 分档（Unicode 标量计数）：≤4 全遮蔽 `••••`；5–8 只留尾 2（`…xy`）；>8 留前 3 + 尾 4（`pre…wxyz`）。
- `base64url::encode/decode`：URL-safe 无填充；decode 拒绝字母表外字符、显式填充、`len%4==1`、末符号非零余位，供 PKCE 与 JWT payload（`decode_jwt_payload`，不验签、仅提取非机密 claim）使用。

## 4. 核心行为与数据流

### 4.1 Provider 凭证解析链（装配期统一入口）

1. `resolve_provider_credential` 查 `SecretBackend` 主条目（`pawork.<provider>` / account `default`）。
2. 命中 → `CredentialSource::AuthFile(StoredCredential)`（仅元数据；需要明文时经 `ApiKeyCredential::resolve` 换 `ResolvedCredential`）。
3. 仅 `NotFound` 允许降级：读 `PAWORK_API_KEY_<ID>` env，命中 → `CredentialSource::EnvFallback(ResolvedCredential)`（env 值只进 `ResolvedCredential`，Debug 脱敏，不落日志）。
4. 两级都缺 → `CredentialSource::None`，调用方 fail-closed（绝不构造伪凭证）。
5. 后端损坏 / IO 失败原样上抛 `AuthError`，不降级到 env——损坏状态不可静默绕过。

### 4.2 OAuth 登录（PKCE 与 Device）

1. PKCE：`start_pkce_flow_with_callback` 校验 redirect_uri 为 http + loopback（host 原样保留——`localhost` 与 `127.0.0.1` 对授权服务器 allow-list 不可互换，只回填实际端口）→ 起 `CallbackServer` → 生成 `Pkce`（S256）与高熵 `state` → 构造授权 URL（`response_type=code` + challenge + state + extra params）。
2. 用户浏览器授权后回调 `GET /?code=&state=`；`CallbackServer` 后台任务 accept 单连接（上限 5 分钟），增量读请求头（64 KiB 上限、容忍分片），解析 query 后回固定纯文本 200（带 `nosniff` / `no-store`，不回显输入）。
3. `wait_for_code` 取回 `(code, state)`；带 `error` 参数的回调判授权失败。
4. `exchange_pkce_code` 校验 state（不符即 CSRF 拒绝）后 POST token endpoint（`grant_type=authorization_code` + verifier）换 `TokenSet`；OAuth2 标准错误归一 `TokenEndpoint`。
5. Device：`request_device_authorization`（form POST；`verification_uri`/`verification_url` 两种拼写都接受，缺省 interval 5s）拿 `DeviceUserPrompt` → `poll_device_token` 按 interval 轮询直到成功、`expired_token` 或超出 max_duration。
6. `TokenSet` 立即经 `store_default_oauth_token`（首发 default 条目）或 `store_oauth_token` 写入 `SecretBackend`；返回值只有元数据。ChatGPT 流从 `id_token` JWT claim（`https://api.openai.com/auth` 命名空间下的 `chatgpt_account_id`）提取 `account_id` 存入 meta（路由头用，非 secret）。

### 4.3 请求前置 auto refresh（singleflight + 跨进程锁）

1. `resolve_oauth_credential_for_request` → `refresh_oauth_credential_if_needed`：`needs_refresh`（`expires_at ≤ now + skew`；无过期时间不刷）不满足直接返回。
2. 进程内 gate：同 `(service, account)` 的并发请求共用一个 `RefreshGate`（async 锁 + 最新元数据发布）；等待者醒来后若后端 access 指纹（SHA-256）仍是已发布版本，直接复用元数据，**不再次消费一次性 refresh token**。
3. 跨进程锁：后端提供 `refresh_lock_path`（FileBackend 为 `auth.refresh.lock`）时，锁外先快照 access/refresh，取锁（10ms 重试、120s 超时）后重读；若别的 Pawork 进程已完成轮换则直接采用其结果并返回。
4. 仍需刷新才 `refresh_access_token`；成功后按注入的持久化策略（多凭证 `update_oauth_token` / default 条目 `update_default_oauth_token`）把轮换后 token 与过期元数据整批写回（先 refresh 后 access 的原子批），最后向 gate 发布新 access 的 SHA-256 指纹与脱敏元数据。刷新响应缺 `expires_in` 时保留旧到期时间（下次仍尝试刷新，而不是误判永不过期），缺 refresh_token 时保留后端旧值；响应携带 `scope` 时按空白拆分覆盖 scopes。
5. 两种条目形态共用同一 singleflight 核心 `refresh_oauth_credential_with`（crate 内），只注入到期判断、持久化与 reload 策略，gate 与发布顺序保持唯一实现——default 条目额外在锁内 reload meta 以吸收其他进程写入。

### 4.4 FileBackend 写路径

1. 每次写操作在 write 锁（`auth.write.lock`，10ms 重试、30s 超时）内 load-modify-save。
2. load：文件不存在/空 → 空表；JSON 损坏或版本不符 → `Storage` 错误 fail-closed（不静默清空）。
3. save：序列化 → 唯一命名临时文件以 0600 新建写入 → 同目录 `rename` 原子替换；崩溃不会暴露半写文件。

## 5. 契约与不变量

- **明文只在后端**：`StoredCredential` / `ApiKeyCredential` / `DefaultOAuthMeta` 可序列化字段永无明文；`TokenSet` / `Pkce` / `PkceSession` / `DeviceAuthorization` / `DeviceUserPrompt` 的 `Debug` 输出对 token / verifier / state / device_code / user_code 一律 `[REDACTED]`（有内联回归断言）。
- **刷新状态无明文**：singleflight gate 只保存脱敏元数据 + access 的 SHA-256 指纹（对应结构不实现 Debug），不持有 token 明文。
- **`ResolvedCredential`（domain 定义）Debug 脱敏、无 `Serialize`**：仅供 adapter 构造认证请求时短暂使用。
- **错误不携带 Secret**：`AuthError` 全部变体的 Display 只含归因描述；token endpoint 错误只保留标准 `error/error_description`。
- **命名唯一事实源**：service 前缀（`pawork` / `pawork.<p>.oauth` / `pawork.mcp.`）、env 名推导、`mcp-auth.json` 文件名只在 `locator` 定义，消费方不得自拼。
- **auth 文件**：0600 权限、原子写、损坏与版本不符 fail-closed；格式版本当前固定 1。
- **refresh 安全**：一次性 refresh token 绝不被并发重复消费（进程内 singleflight gate + 跨进程文件锁 + 指纹校验三层防线）；轮换 refresh token 已落盘后才返回 bearer。
- **回调安全**：redirect_uri 强制 http + loopback（host 原样保留、只回填端口）；回调响应固定文本不回显 query（防反射）；请求头 64 KiB 上限。
- **base64url 严格解码**：非规范编码（填充/余位）一律拒绝，防止同值多表示。
- **写入前验证**：`store_default_api_key` / `store_oauth_token` 等对空 secret 先拒绝（`InvalidSecret`）且不产生部分写入；`delete_default_api_key` / `delete_default_oauth_token` 幂等（不存在视为成功）。
- **测试纪律**：自动测试只用 `MemoryBackend` 或显式临时路径的 `FileBackend`，不读真实 auth 文件。

## 6. 依赖关系

- **上游**：仅 `pawork-domain`（`ProviderId` / `CredentialId` / `Timestamp` / `ResolvedCredential` / `CredentialKind`）。三方：`reqwest`（token endpoint）、`tokio`（回调/轮询）、`serde(_json)`、`sha2`（PKCE S256 与指纹）、`getrandom`（熵源）、`url`、`directories`（home 定位）、`thiserror`、`async-trait`。
- **下游**：`pawork-app`（六通道装配、`pawork auth`）与 `pawork-tools`（MCP OAuth）。依赖方向见 [../../design.md](../../design.md) §2，产品侧安全边界见 [../security.md](../security.md)。
- 模块可见性：`locator` / `oauth` 为 pub 模块（可全路径引用），其余模块私有、仅经 crate 根 re-export 暴露（`generate_credential_id` 等保持 crate 内可见）。

## 7. 测试与验证资产

无 `tests/` 目录；回归内联于各文件 `#[cfg(test)]`（dev-dependencies 仅 `wiremock` 与多线程 tokio（含 test-util）；文件后端测试使用显式临时路径，不引入 tempfile）。默认验证命令：`cargo test -p pawork-auth --offline --lib --tests`。

| 位置 | 覆盖点 |
| --- | --- |
| `backend.rs` / `file_backend.rs` | store/get/delete 往返、`store_batch` 原子性、0600 权限、原子替换、损坏文件与版本不符 fail-closed、write 锁竞争（测试用 `std::env::temp_dir()` 唯一路径） |
| `locator.rs` / `resolve.rs` | env 名推导（大写、`-`→`_`）、解析链三分支、仅 `NotFound` 降级、损坏上抛、env 值不入日志字段 |
| `masked.rs` / `base64url.rs` | 三档脱敏边界、Unicode 安全、非规范 base64url 拒绝 |
| `credential.rs` / `default_credential.rs` | 元数据序列化无明文、default 三账户读写、meta 损坏报 `MalformedMetadata`、ChatGPT account_id claim 提取 |
| `oauth.rs` | PKCE 与 Device 全流程、refresh 语义、回调服务器行为（见下） |

`oauth.rs` 内联回归要点：

- PKCE verifier 长度/字符集/无偏 base64url、S256 challenge 对 RFC 7636 附录 B 向量确定性一致、state 高熵 URL-safe；
- `TokenSet` / `PkceSession` / `DeviceUserPrompt` Debug 全脱敏；`store_oauth_token` 元数据与序列化无明文、空 refresh 拒绝且零写入；
- `update_oauth_token` 轮换落盘、刷新缺 `expires_in` 保留旧到期时间、`needs_refresh` skew 边界；
- wiremock 驱动：PKCE 交换成功 / state 不符拒绝 / token endpoint 标准错误归一、Device pending→成功轮询、refresh 换新 token、请求前置解析自动刷新并持久化轮换；
- `concurrent_refreshes_share_one_singleflight_exchange`：并发刷新只发生一次 token exchange（`.expect(1)`）；
- 回调服务器：code/state 解析、错误回调不反射 query 输入、分片请求头（8 KiB cookie）读取、PKCE 回调流使用实际监听端口。

## 8. 注意事项与已知限制

- 任务说明中常提的「master.key 与并发首建」不在本包：reasoning blob 加密的 master key 由 `pawork-app` protected 模块管理（见 [app.md](app.md)）；本包的并发原语是 OAuth refresh 的 singleflight gate 与 `auth.refresh.lock` 跨进程锁。
- 首发阶段每 provider 只有一条 default OAuth 条目（`default.access/.refresh/.meta`）；多凭证/账号池是登记在案的后续项（见 [../backlog.md](../backlog.md)）。多凭证形态的 `store_oauth_token`（`<cred_id>.access/.refresh`）已可用但无独立 meta 持久化。
- `MemoryBackend` 的 `store_batch` 是逐条语义（默认实现），不具备 FileBackend 的整批原子性；测试断言原子回滚行为时需注意。
- `decode_jwt_payload` 不验签，只用于提取非机密路由 claim，不构成信任边界。
- 回调服务器一次性、单连接、固定 200 文本响应；不支持 https redirect（上游 allow-list 均为本机 http）。`CallbackServer::start` 需要已存在的 tokio runtime（`Handle::try_current`），纯同步上下文无法启动。
- 时间口径统一为 Unix 毫秒（`now_unix_millis`），到期判断依赖本机时钟；`refresh_skew` / 30s grace 用于吸收时钟偏差与网络延迟。
- 跨包链路（装配、CLI 交互）见 [../flows.md](../flows.md)；任务状态见 [../../../ROADMAP.md](../../../ROADMAP.md)。
