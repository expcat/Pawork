# S12 CR-04 审查报告：Secret / 网络 / MCP 边界

| 项 | 值 |
| --- | --- |
| CR 编号 | CR-04 |
| 主审范围 | `providers/auth`、`providers/core`、`providers/adapters`、`net/net`、`foundation/config`、`foundation/diagnostics`、`extensions/mcp`、`control-plane/provider-control`（含 tests；`account-control-v1` 的 account/routing/health/factory/reconciler 降采样） |
| 审查日期 | 2026-08-18 |
| 主审模型 | Grok（xai/grok-4.6） |

## 实际审查路径

- `providers/auth/src/{backend,file_backend,oauth,resolve,default_credential,credential,masked,error,lib}.rs`：FileBackend 0600 / 原子 rename / write+refresh 锁；RefreshGate + lock-inner reread；Debug 脱敏；确定性 service/account
- `providers/core/src/{lib,negotiate,registry,pricing,reasoning,usage,error}.rs`：能力协商诚实性与 Provider 无特例
- `providers/adapters/src/{lib,provider,responses,api_key,xai,chatgpt,anthropic/{mod,provider,request,stream}}.rs`：认证头覆盖检查、Anthropic 能力缺口、error body 入口
- `net/net/src/{http,retry,sse,jsonl,partial_json,lib}.rs`：HttpClient 构造、redirect 默认、extra_headers、loopback_aware_proxy、classify_status
- `foundation/config/src/{loader,schema,merge,paths,env,error,lib}.rs`：sanitize_secrets 只剥 `api_key`、workspace 可覆盖 `proxy_url`/`base_url`/`extra.mcp`、`trust_workspaces` 仅 builtin/global
- `foundation/diagnostics/src/{lib,logging}.rs` + `apps/pawork/src/main.rs`：`RedactingFmtLayer` 挂载点
- `extensions/mcp/src/{config,security,capabilities,sandbox,transport,codec,manager,oauth,lib}.rs`：SecretRef 解析、trusted/auto_start、stdio spawn、HTTP 头
- `host/app/src/{lib,extensions}.rs`（跨包装配，为证明热路径）：同一 `FileBackend` 同时服务 Provider 凭证与 MCP；`proxy_url`/`base_url` 注入；MCP 启动
- `control-plane/provider-control/src/{lib,credential,lease,binding}.rs` + `tests/error_matrix.rs`：CredentialPool / LeaseRecord / SessionBinding 只持 opaque ID
- `execution/exec/src/sandbox.rs` `default_secret_paths`（对照 CR-03，不重复建项）
- `foundation/api/src/lib.rs` `ResolvedCredential` / `ProviderError`；`foundation/domain/src/{error,events}.rs` `ErrorContext` / `RunFailed`
- `engine/engine/src/session_turn.rs`（跨包热路径：ProviderError 到可持久化事件）
- 已知基线：`ROADMAP.md` §3.2 K-01、K-10；`docs/task-guide.md` §3.1 Secret 红线；`docs/design.md` §3.2 配置无 `api_key`

## 核对结论（无违约 / 不建项）

1. **FileBackend 落盘红线**：`providers/auth/src/file_backend.rs` 默认 `~/.pawork/auth.json`（`PAWORK_HOME` 可覆盖），`write_new_file_0600` + 独立临时文件 `rename` 原子写；`store_batch`/`delete` 持 write lock；OAuth 另有 `auth.refresh.lock`。未发现明文 token 写入 session/sqlite。
2. **OAuth 并发刷新**：`RefreshGate` 进程内 singleflight；FileBackend 路径下先拍锁外快照，再持 `refresh.lock` 重读，他进程已轮换则不再消费旧 refresh token（`oauth.rs:765-820`）。
3. **类型级 Debug 脱敏**：`TokenSet` / PKCE / device-flow、`ResolvedCredential`、`ResolvedSecret`、MCP `HttpTransportConfig` 的 token/headers 均 `[REDACTED]`；`StoredCredential` 只含 `MaskedCredential`。
4. **diagnostics 挂载**：`apps/pawork/src/main.rs:11-22` 将 `RedactingFmtLayer` 装到 tracing Registry；fmt 字段走 `Redactor`。此层挡不住已写入 `AgentEvent` / session 库的明文。
5. **provider-control 凭证边界**：`LeaseRecord` / `SessionBinding` / `CredentialLease` 只有 opaque ID；`CredentialResolver` 错误变体禁止回传后端原文。`account-control-v1` 未接宿主，见未覆盖。
6. **配置无 api_key 字段**：`ProviderConfig` 无该字段；`sanitize_secrets` / extra 反序列化剥离顶层与 `providers[].api_key`，与 design §3.2 一致。缺口是未覆盖 `mcp.*.SecretRef` 与 `proxy_url`/`base_url`。
7. **K-01 / K-10**：配置路径发现与 Anthropic Messages 能力收口已在 ROADMAP §3.2。本包补充证据并分别链接，不作为新项。

## Findings

### S12-CR04-01

- **类别**：Security
- **严重度**：High　**置信度**：Confirmed
- **证据**：
  - `foundation/config/src/loader.rs:214-221,351-361`：`sanitize_secrets` 只删除顶层与 `providers[].api_key`，`extra["mcp"]` 原样进入合并配置。
  - `extensions/mcp/src/config.rs:45-56,187-201,451-461`：workspace 可声明 HTTP `headers` / stdio `env` 为 `SecretRef{service,account}`；`resolve_secret_map` 调用 `SecretRef::resolve` 后 `expose_secret()` 写入请求头或子进程环境。
  - `extensions/mcp/src/security.rs:52-56`：`backend.get(&self.service, &self.account)` 无 service 前缀 / 调用方 allowlist。
  - `host/app/src/lib.rs:408-411,458` 与 `host/app/src/extensions.rs:117-157,435-460`：宿主默认 `FileBackend::new()`，同一后端同时解析 Provider 凭证与 MCP SecretRef。
  - 确定性定位：`providers/auth/src/resolve.rs:21,95-104` 主条目 `pawork.<provider>` / `default`；`providers/auth/src/default_credential.rs:43-56` OAuth `pawork.<provider>.oauth` / `default.access|refresh|meta`。`FileBackend::get`（`file_backend.rs:159-164`）对任意 `(service,account)` 原样返回明文。
  - `execution/exec/src/sandbox.rs:578-590` `default_secret_paths()` 不含 `~/.pawork/auth.json`（与 [CR-03](CR-03-exec-cli.md) S12-CR03-01 同源对照，不重复建项）。
  - **实际行为**：恶意或被投毒的 workspace `.pawork/config.toml` 只需写 MCP HTTP `SecretRef` 指向 `pawork.openai`/`default`（或 chatgpt/xai OAuth 账户），`auto_start` 后宿主会从全局 `auth.json` 取出用户 token，经 HTTP 头发给攻击者 URL，或经 stdio env 交给任意命令。
  - **期望行为**：配置层 Secret 红线（task-guide §3.1）要求明文不进不可信配置驱动的出站通道。MCP SecretRef 应限制在 `pawork.mcp.*`（或独立 MCP 后端），且 workspace 层不得解析用户全局 auth 文件。
  - **影响面**：所有使用默认 FileBackend 并开启 workspace MCP 的 `pawork` 会话。攻击前提是打开含恶意配置的工作区（或被写入 `.pawork/config.toml`），不需要用户把 token 写进仓库。
- **验证建议**（S12 内不执行）：在临时 FileBackend 写入 `pawork.openai/default`，workspace 配置一个 HTTP MCP，`SecretRef` 指向该条目；断言启动后不得向该 URL 发出明文。再断言 `service` 非 `pawork.mcp.*` 时 fail-closed。
- **整改边界**：最小写入 = `extensions/mcp/src/{security,config}.rs` + `host/app/src/extensions.rs`（解析前校验 locator / 使用独立 MCP backend）。不可顺带改 FileBackend 格式或重做 CR-03 的 sandbox deny 清单；`auth.json` 补入 `default_secret_paths` 归 CR-03。

### S12-CR04-02

- **类别**：Security
- **严重度**：High　**置信度**：Confirmed
- **证据**：
  - `foundation/config/src/schema.rs:50,83,175-176`：`proxy_url` 与 `providers[].base_url` 为普通可合并字段；workspace 层不像 `trust_workspaces`（`loader.rs:290-304`）那样被剥离。
  - `host/app/src/lib.rs:696-706,1829-1863,1886-1900`：装配期把 `config.proxy_url` 写入 ChatGPT / xAI / API-key / OpenAI-compatible / Anthropic 的 `HttpClientConfig.proxy`；`config_base` 覆盖默认上游。
  - `net/net/src/http.rs:100-116`：`HttpClient::new` / host `http_from_config` 均未 `redirect(Policy::none())`。reqwest 0.12.28 默认跟随最多 10 次跳转。
  - 第三方 `reqwest-0.12.28/src/redirect.rs:239-249`：跨 host 只删除 `Authorization` / `Cookie` / `Proxy-Authorization`，不删 `x-api-key`。Anthropic 认证头是 `x-api-key`（`anthropic/provider.rs:85-88`），跨 host 302 会原样带出。
  - `net/net/src/http.rs:277-287`：`loopback_aware_proxy` 只让目标为 loopback/`.local` 时直连；workspace 指定的代理仍接收全部非回环出站（含 Bearer token）。
  - **实际行为**：workspace 可把 `proxy_url` 指到攻击者代理，或把 `base_url` 指到攻击者源站。前者在到代理的路径上截获全部 Provider/OAuth 请求；后者让宿主主动把凭证打到攻击者 URL。再叠加默认跟随跳转，Anthropic `x-api-key` 可被跨域带走。
  - **期望行为**：`proxy_url` / 非回环 `base_url` 与 `trust_workspaces` 同级，仅 builtin/global（或显式用户批准）可设；出站客户端应对跨 origin 跳转 fail-closed，或至少剥离全部凭证头（含 `x-api-key`）。
  - **影响面**：任何打开恶意 workspace 的 chat / catalog / OAuth 刷新路径。不依赖 MCP。
- **验证建议**：workspace 配置本地 sink 代理与伪造 `base_url`，对 API-key 与 Anthropic 各打一发；检查 sink 是否看到 Bearer / `x-api-key`。再让 `base_url` 返回跨 host 302，确认 `x-api-key` 是否跟随。S12 内不执行。
- **整改边界**：最小写入 = `foundation/config/src/loader.rs`（workspace 剥离或降级 `proxy_url`/`base_url`）+ `net/net/src/http.rs`（禁用或收紧 redirect，统一剥凭证头）+ host 装配注释。不可顺带实现 egress broker（K-09）或改 OAuth 协议。

### S12-CR04-03

- **类别**：Security
- **严重度**：High　**置信度**：Confirmed
- **证据**：
  - `net/net/src/http.rs:226-231,254-255`：非 2xx 响应 `response.text()` 截断 512 字节后交给 `classify_status`，无脱敏。
  - `net/net/src/retry.rs:12-35`：注释写脱敏响应正文；实现把 `body_snippet` 原样写入 `ProviderError.message`。
  - `foundation/api/src/lib.rs:610-623,680-688`：`From<ProviderError> for ErrorContext` 原样复制 `message`。
  - `foundation/domain/src/error.rs:26-36`：`ErrorContext` 契约写明不得包含 Secret 或未经脱敏的响应正文。
  - `engine/engine/src/session_turn.rs:159-163` + `foundation/domain/src/events.rs:200-201`：该 `ErrorContext` 进入可持久化 `AgentEvent::RunFailed`。
  - **实际行为**：上游 4xx/5xx 若在 body 回显 API key、Authorization、cookie 或 prompt，最多 512 字节会写入 session 事件流，并可被 GUI/重放/导出读出。diagnostics fmt 红线管不到这条持久化路径。
  - **期望行为**：红线要求 Secret 不进事件 payload。`classify_status` 只保留 status / 稳定错误码，正文进 `redacted_details` 且必须过 Redactor，或丢弃。
  - **影响面**：所有经 `HttpClient` 的 Provider 失败路径（含 Anthropic / OpenAI-compatible / Responses）。一次失败即落盘。
- **验证建议**：mock 401 body 含测试明文 token，跑一轮 session，断言 `session_events` 与 `RunFailed` JSON 不含该明文。S12 内不执行。
- **整改边界**：最小写入 = `net/net/src/retry.rs`（及 http.rs 调用点）+ 一条定向回归。不可顺带改事件 schema 或 GUI 错误展示文案语义。

### S12-CR04-04

> **交叉复核裁定**（2026-08-18 主代理回写，GLM 复核，详见 [CR-04-cross-review-glm.md](CR-04-cross-review-glm.md)）：**uphold High**，边界表述修正——trusted=true 只绕过未信任 workspace 硬门，写入类 MCP 工具单次调用仍触发审批，「免于执行」应表述为「绕过未信任硬门」。另补两项加重事实：MCP 子进程 env_clear=false 完整继承宿主环境（含 PAWORK_API_KEY_*）；MCP stdio 路径不经 SandboxSelector 硬隔离。

- **类别**：Security
- **严重度**：High　**置信度**：Confirmed
- **证据**：
  - `foundation/config/src/loader.rs:290-304`：只剥离 workspace 的 `trust_workspaces`，不剥离 `extra.mcp`。
  - `extensions/mcp/src/config.rs:84-96`：`auto_start`、`trusted` 默认可由 workspace TOML 设为 true；`TransportSpec::Stdio.command` 无二进制 allowlist。
  - `host/app/src/extensions.rs:135-157,435-460`：`prime_extensions` 到 `start_mcp_servers` 对 `auto_start` 直接 `build_client` + `register_server_tools(..., server.trusted)`，不与 `self.workspace_trusted` 交叉校验。
  - `extensions/mcp/src/capabilities.rs:106-124`：`allowed_in_untrusted_workspace = read_only || trusted`。workspace 写 `trusted = true` 即可让写入类 MCP 工具在未信任工作区免于禁止执行。
  - `host/app/src/extensions.rs:446-458` + `extensions/mcp/src/sandbox.rs:86-119`：stdio 策略 `allow_spawn: true`、`NetworkMode::Hint`（非 Enforce）；`write_roots` 仅随 host `workspace_trusted` 变化，挡不住读出站与再 spawn。
  - **实际行为**：未信任 workspace 仍可 auto_start 拉起任意本地命令，并自封 `trusted=true` 把 MCP 写工具标成可在未信任区运行。这与 `trust_workspaces` 的自我提权防护目标相反。
  - **期望行为**：workspace MCP 默认不 auto-start、不可自封 trusted；stdio 命令需全局 allowlist 或用户确认；MCP `trusted` 不得高于宿主 `workspace_trusted`。
  - **影响面**：S9 MCP 热路径。与 CR04-01 叠加时可把读到的 token 经子进程网络送出（Hint 模式不强制断网）。
- **验证建议**：未信任 workspace 配置 `trusted=true` + `auto_start` stdio `/usr/bin/true`，断言不得启动或 `allowed_in_untrusted_workspace` 仍为 false。S12 内不执行。
- **整改边界**：最小写入 = `foundation/config`（workspace 剥离 `mcp.*.trusted`/`auto_start` 或整段 mcp）+ `host/app/src/extensions.rs` + `extensions/mcp/src/capabilities.rs`。不可顺带实现完整 MCP 审批 UI 或改 K-09 网络 Enforce。

### S12-CR04-05

- **类别**：Security
- **严重度**：Medium　**置信度**：Confirmed
- **证据**：
  - `providers/adapters/src/lib.rs:8-16` + `provider.rs:71-81` + `responses.rs:78-87` + `xai.rs:249-258`：Chat / Responses / xAI 在构造期拒绝 `extra_headers` 覆盖认证头。
  - `providers/adapters/src/anthropic/provider.rs:64-88`：`AnthropicProvider::new` 无对等检查；`auth_headers` 无条件推 `x-api-key`。`net/net/src/http.rs:159-163` 先附加 `config.extra_headers` 再附加 per-request 头，重复 `x-api-key` 可并存或覆盖。
  - `net/net/src/http.rs:17-29`：`HttpClientConfig` derive `Debug`，`extra_headers` 以明文进入 Debug。适配器配置被 Debug 打印时，自定义头里的 token 绕过 `ResolvedCredential` 脱敏。
  - **实际行为**：Anthropic 路径缺少与其它渠道一致的不可覆盖认证头闸门；HTTP 配置 Debug 可打印明文头。
  - **期望行为**：全渠道统一拒绝凭证头覆盖；`HttpClientConfig` Debug 对 header 值脱敏。
  - **影响面**：当前 host 装配不把用户 TOML 映射进 `extra_headers`，默认热路径较窄；任何把配置打进日志/诊断的调用点与未来可配 header 的扩展会立刻放大。
- **验证建议**：为 Anthropic 复用 xAI 的 `fixed_credential_header_is_rejected` 用例；Debug 打印带 `x-api-key` 的 `HttpClientConfig` 不得含明文。S12 内不执行。
- **整改边界**：最小写入 = `anthropic/provider.rs` + `net/net/src/http.rs` Debug。不可顺带改 Anthropic 请求体（那是 K-10）。

### S12-CR04-06

- **类别**：Requirement Gap（能力诚实性）
- **严重度**：Medium　**置信度**：Confirmed
- **已知基线**：K-10（ROADMAP §3.2）。本条只补证据，不新增 ROADMAP 任务。
- **证据**：
  - `providers/adapters/src/anthropic/mod.rs:7-8`：显式 TODO — prompt cache / thinking / hosted tools / signature / server_tool / citations 不写 wire。
  - `providers/adapters/src/anthropic/request.rs:1-3,112-135,348-371`：`to_messages_body` 忽略 `thinking` / `cache_control`；测试断言不写这些字段。
  - `providers/core/src/negotiate.rs:49-75`：协商器对未声明 hosted tools / citations 进 `unsupported`+`Reject`，本身诚实；缺口在 Anthropic adapter 未实现或未把模型能力标成不支持，与 S6 完成声明不一致。
  - **实际行为**：调用方按 S6/能力表请求 thinking、cache、citations 时，Anthropic 通道静默丢弃，不报 unsupported。
  - **期望行为**：K-10 — 逐项实现、显式 unsupported 或延期，并同步能力表。
  - **影响面**：Anthropic Messages 用户可见的能力谎言；无直接 Secret 泄漏。
- **验证建议**：见 K-10 任务书。S12 内不执行。
- **整改边界**：归 K-10。本包不另立写入集。

## 未覆盖路径与原因

- `control-plane/provider-control` 的 `account.rs` / `routing.rs` / `health.rs` / `factory.rs` / `reconciler.rs` 及 `account-control-v1` 仓储：任务书允许降采样；host 以 `default-features=false` 关闭该 feature，无生产接线。只确认 lease/binding/pool 无 secret 字段。
- `extensions/mcp/src/oauth.rs`、`manager.rs` 重试细节、stdio codec 帧解析：抽查 HTTP 头与 sandbox spawn，未逐分支审 OAuth-for-MCP。
- `providers/adapters` 的 stream/usage/error_table 全部分支、`providers/core` pricing 表：与 Secret/SSRF 无直接关系，未逐模型核对报价。
- `foundation/diagnostics` `experimental` bundle/metrics：默认关闭，只确认 default 的 `RedactingFmtLayer`。
- K-01 仓库根 config 路径闭环：`paths.rs` 的 global/workspace 发现逻辑已读，目录摊平后的真实家目录布局未在本机跑；不作为新 finding。
- 运行时验证：按 S12 纪律未执行 cargo test/build、未跑 pawork、未做真实 OAuth/MCP 冒烟。

## 统计

| 严重度 | 条数 |
| --- | --- |
| Critical | 0 |
| High | 4 |
| Medium | 2 |
| Low | 0 |

| 置信度 | 条数 |
| --- | --- |
| Confirmed | 6 |
| Needs Verification | 0 |

跨包链接：CR04-01 对照 [CR-03](CR-03-exec-cli.md) 的 `default_secret_paths` 缺口，不重复建项。CR04-06 链接 K-10，不回写新的 §3.2 行。Confirmed 项的 ROADMAP 回写由父代理按任务书执行。
