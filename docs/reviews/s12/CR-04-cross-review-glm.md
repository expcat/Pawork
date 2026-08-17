# CR-04 High Findings 交叉复核（GLM）

- 复核对象：[CR-04-secret-net-mcp.md](CR-04-secret-net-mcp.md) 中 4 条 High（S12-CR04-01 ~ 04）
- 复核人：zai/glm-5.3（glm_reviewer）
- 复核日期：2026-08-18
- 方法：不采信报告转述，逐条独立打开源码（路径+符号+行号）核对实际行为；重点复核 SecretRef 解析链与 workspace 配置层的真实优先级；遵守 S12 只读纪律，未运行任何构建/测试/二进制。

## 裁定表

| 编号 | 原严重度 | 裁定 | 一行理由 |
| --- | --- | --- | --- |
| S12-CR04-01 | High | uphold（维持 High） | SecretRef 解析链端到端核实：workspace extra.mcp 的任意 (service, account) 可从与 Provider 共享的 FileBackend 取出全局明文，经 MCP HTTP 头 / stdio env 送出，且 stdio env 无任何过滤。 |
| S12-CR04-02 | High | uphold（维持 High） | workspace 层对 proxy_url / providers[].base_url 均有真实覆盖优先级；reqwest 0.12.28 默认跟随 10 次跳转且跨 host 只剥 Authorization/Cookie 族头，Anthropic x-api-key 会原样带出。 |
| S12-CR04-03 | High | uphold（维持 High） | 512B 原始 error body 经 classify_status 原样进 ProviderError.message → ErrorContext → RunFailed 落库；事件库脱敏是按键名启发式，自由文本 message 内嵌密钥不会被清洗，该防线不构成缓解。 |
| S12-CR04-04 | High | uphold（维持 High） | 未信任 workspace 的 auto_start stdio 在装配期零审批拉起任意命令（NativeRestricted 软沙箱 + Hint 网络 + allow_spawn:true）；trusted=true 确实绕过未信任硬门，但写入类工具单次调用仍会触发审批（见边界澄清）。 |

## 逐条复核记录

### S12-CR04-01 — workspace MCP SecretRef 读取全局 auth.json（uphold）

- 配置优先级（报告核心主张，逐行核实）：
  - foundation/config/src/loader.rs resolve_sources：按 tier 排序后 builtin → global → profile → workspace → session → run 依次 merge（285-310）；merge_json 对 object 递归合并、workspace 同键覆盖 global（foundation/config/src/merge.rs 51-73）。
  - 唯一被分层剥离的键是 trust_workspaces（loader.rs 290-305 remove_top_level_key）；sanitize_secrets 只删顶层与 providers[].api_key（loader.rs 214-221、351-364）。extra["mcp"]（含 SecretRef 的 env/headers）无任何剥离或降级。
- SecretRef 解析链：
  - extensions/mcp/src/config.rs TransportSpec::Stdio.env / Http.headers 均为 BTreeMap<String, SecretRef>（189-201）；resolve_transport → resolve_secret_map（251-271、451-463）调用 SecretRef::resolve 后 expose_secret() 直接进入子进程 env / HTTP headers。
  - extensions/mcp/src/security.rs SecretRef::resolve（52-57）：backend.get(&self.service, &self.account)，无 service 前缀约束、无调用方 allowlist。
- 同一后端（关键交叉证据）：host/app/src/lib.rs load_with 默认 FileBackend::new()（408-411），assemble_provider 与 MCP build_client 共用 self.backend（host/app/src/extensions.rs 148）。providers/auth/src/credential.rs keychain_service_for（51-53）= "pawork.<provider>"；resolve.rs PROVIDER_DEFAULT_ACCOUNT="default"（21）与 resolve_provider_credential 读取口径（95-104）；default_credential.rs OAuth 条目 pawork.<provider>.oauth / default.access|refresh|meta（43-57）。FileBackend::get 对任意 (service, account) 返回明文（providers/auth/src/file_backend.rs 159-164）。
- 外泄通道端到端核实：
  - HTTP：extensions/mcp/src/codec.rs serve_http 把 resolve 后的 headers 转成 rmcp custom_headers（305-316），随每个请求发送；非回环 http+secret 的 HTTPS 校验（config.rs 205-224、codec.rs 296-298）不拦截攻击者自有 https 域名。
  - stdio：extensions/mcp/src/sandbox.rs SandboxedStdioSpawner::spawn 把 cfg.env 原样写入 command.env（99-106）；execution/exec/src/process.rs spawn_child 直接 command.env(k, v)（486-488）。stdio_runtime 的策略用 ..SandboxPolicy::default()（host/app/src/extensions.rs 442-461），其 env_allowlist/env_denylist 为空 Vec、env_clear=false（execution/exec/src/sandbox.rs 56-68 derive Default），apply_env 的三个触发条件全不命中 → 全部 env 透传（exec sandbox.rs 544-559）。
- 补充事实（报告未提，加重而非削弱）：因 env_clear=false，MCP 子进程还继承宿主完整环境，含 PAWORK_API_KEY_* env fallback 凭证。
- 裁定：完整攻击链（workspace TOML → SecretRef{pawork.<provider>, default} → FileBackend.get → HTTP 头/子进程 env）每一跳均有源码证据，High 维持。

### S12-CR04-02 — workspace 可覆盖 proxy_url/base_url + 默认跳转不剥 x-api-key（uphold）

- 配置层：foundation/config/src/schema.rs proxy_url 为普通可合并 Option<String>（50 附近），ProviderConfig.base_url 同为普通字段；两者均不像 trust_workspaces 那样分层剥离。merge_json 数组整体替换，workspace providers 表可整体接管并给默认 provider 设任意 base_url。
- 装配层：host/app/src/lib.rs assemble_provider 从合并后的 config.providers 取 config_base（1816-1818），ChatGPT/xAI/API-key/OpenAI-compatible/Anthropic 五条路径均接受覆盖（1829-1833、1844-1847、1862-1864、1886-1899），并把 config.proxy_url 注入各通道 http.proxy；OAuth 刷新/探测客户端同样走 http_from_config(config.proxy_url)（696-708）。
- 跳转与凭证头：
  - net/net/src/http.rs HttpClient::new（100-118）未设置 redirect policy；vendored reqwest-0.12.28 async_impl/client.rs 310 默认 redirect::Policy，redirect.rs 160-165 = Policy::limited(10)。
  - reqwest-0.12.28/src/redirect.rs remove_sensitive_headers（239-251）：跨 host 仅删 authorization/cookie/cookie2/proxy-authorization/www-authenticate，x-api-key 不在清单；调用点在 redirect.rs 338。Cargo.lock 5808-5810 确认 workspace reqwest = 0.12 → 0.12.28。
  - providers/adapters/src/anthropic/provider.rs auth_headers（85-88）以 per-request 头发送 x-api-key。
  - net/net/src/http.rs loopback_aware_proxy（266-288）：仅回环/.local/.localhost 直连，workspace 指定代理收到其余全部出站（含凭证）。
- 裁定：所有子主张核实成立。单 workspace proxy_url 一项即可截获全部 Provider/OAuth 凭证流量，High 维持；redirect/x-api-key 是叠加放大项而非唯一依据。

### S12-CR04-03 — 512B error body 进入可持久化 RunFailed（uphold）

- 生成链：net/net/src/http.rs get_json_with_headers（222-231）与 handle_response（246-255）对非 2xx 读 response.text()、truncate 512 后交 classify_status；net/net/src/retry.rs classify_status（14-35）把 body_snippet 原样 format! 进 ProviderError.message——注释自称「脱敏响应正文片段」但无任何脱敏实现。
- 传播链：foundation/api/src/lib.rs From<ProviderError> for ErrorContext（680-690）原样复制 message；foundation/domain/src/error.rs ErrorContext 契约（26-27）明文禁止 Secret 或未经脱敏的响应正文；engine/engine/src/session_turn.rs（159-163）emit AgentEvent::RunFailed{error: context}；foundation/domain/src/events.rs RunFailed（200-201）。
- 持久化链（报告未展开，本次补齐）：host/app/src/persist.rs PersistThenRender（16-23）对每个 envelope 先 SessionStore::append_event；storage/session/src/event_store.rs append_event → persist_event_in_transaction（197-266、390-422）把整个 payload 序列化写入 session_events.payload_json。
- 关键反证检验（本人主动排查，结论为不构成缓解）：event_store.rs 存在持久化脱敏层（424-457 redact_sensitive_json），但其脱敏完全基于键名启发式（authorization/apikey/secret/token 等族，527-576）；RunFailed 的 body 片段位于自由文本 "message" 键下，不属于任何敏感键族，内嵌的密钥/prompt 原文不会被清洗。报告结论成立。
- 裁定：契约违约明确（domain error.rs:26）+ 一次失败即落盘 + 事件库脱敏不覆盖该形态，High 维持。

### S12-CR04-04 — workspace 自封 trusted + auto_start 拉起任意 stdio 命令（uphold）

- 配置层：foundation/config/src/loader.rs 只剥离 workspace 的 trust_workspaces（290-305）；extensions/mcp/src/config.rs McpServerConfig 的 auto_start / trusted 均为普通反序列化字段（83-97），TransportSpec::Stdio.command 为任意字符串，无二进制 allowlist。
- 启动路径：host/app/src/extensions.rs start_mcp_servers（116-198）对 auto_start 直接 build_client + register_server_tools(..., server.trusted)，全程无 self.workspace_trusted 交叉校验；prime_extensions 在 AppCore 装配期调用（105-114），即打开工作区即执行，零审批。
- 沙箱实际强度：
  - stdio_runtime（host/app/src/extensions.rs 442-461）：allow_spawn:true、NetworkMode::Hint（审计提示非强制）、write_roots 仅随宿主 trust 变化；且直接 new NativeRestricted（460），不经过 SandboxSelector::pick——MCP stdio 连 macOS sandbox-exec 硬隔离都不会选上。NativeRestricted 自述为非对抗性软沙箱（execution/exec/src/sandbox.rs 185-189）。
  - 信任门语义：execution/policy/src/engine.rs decide（44-53）第一道硬门是 !trusted && !allowed_in_untrusted_workspace → Deny；extensions/mcp/src/capabilities.rs descriptor（119-124）allowed_in_untrusted_workspace = read_only || self.trusted；execution/tools/src/scheduler.rs check_gate（345-397）把宿主 workspace_trusted 与 descriptor 的自封值一起喂给该门。workspace 写 trusted=true 即绕过。
- 边界澄清（一处需在整改时注意，不影响定级）：「写入类 MCP 工具在未信任区免于禁止执行」应表述为「绕过未信任硬门，但单次调用仍触发审批」——capabilities.rs:119 requires_approval = !read_only，scheduler.rs:363-370 会把非 Deny 决策升级为 AskUser，除非审批宿主自动放行。auto_start 任意命令执行则无此限定，构成 High 的充分依据。
- 裁定：auto-start 任意本地命令 + 自封 trusted 绕过未信任门均成立，High 维持。

## 复核补充事实（不构成新 finding）

- stdio MCP 策略使用 derive Default 的 SandboxPolicy（env_allowlist 为空、env_clear=false），意味着 MCP 子进程完整继承宿主环境（execution/exec/src/sandbox.rs 56-68、544-559；process.rs 483-488）。CR04-01 的 stdio env 外泄不依赖 SecretRef 也可经继承的 PAWORK_API_KEY_* 发生，整改时 env 清洗应与 SecretRef locator 约束同步收口。
- MCP stdio 不经 SandboxSelector::pick（host/app/src/extensions.rs 460 直接 NativeRestricted::new()），与 run_command 的后端选择链不同；CR-03 的 Hard 隔离结论不适用于 MCP 路径。
- 事件库脱敏（event_store.rs 424-457）是键名启发式而非内容扫描，任何进入自由文本字段（message/reason 等）的密钥都不会被清洗；CR04-03 的整改若只补 Redactor 而不动 ErrorContext 生成点，同类问题会在其他自由文本字段复现。
