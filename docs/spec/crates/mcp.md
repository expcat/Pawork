# pawork-mcp

> MCP 客户端与 canonical 工具桥。2026-09-27 从 tools/mcp 整体迁移；宿主直接依赖本包，将 MCP 工具注册到 tools 的同一 ToolRegistry。

## 1. 职责与边界

承载 MCP 配置、连接生命周期、协议转换、工具发现、stdio 沙箱、Secret locator 与 OAuth。依赖 tools 的注册表，tools 不反向依赖 MCP；不创建第二套工具调度或审批实现。rmcp 的类型仅在私有 codec 内出现。

## 2. 模块树

| 文件 | 行数量级 | 职责 |
| --- | --- | --- |
| `src/lib.rs` | ~110 | MCP 边界类型：`McpError`（10 变体）、`McpServerCapabilities`、`McpToolInfo`、`McpToolCall`、`McpPeer` trait；re-export sandbox 三件套。 |
| `src/tests.rs` | ~150 | 原根模块测试：公开源码 SDK 隔离、内置与 MCP 共用注册表 |
| `src/capabilities.rs` | ~740（逻辑 ~300 + 测试） | 能力桥：`McpCapabilities::discover`、`McpToolAdapter`（MCP 工具 → `AgentTool`）、`register_server_tools` / `register_discovered_tools`、`namespaced_name`；workspace/工具白名单、非对象输入拒绝、输出预算、信任钳制。 |
| `src/codec.rs` | ~700（逻辑 ~430 + 测试） | **rmcp SDK 唯一隔离点**（crate 私有 mod）：`RunningClient`/`ClientPeer` 包装、initialize 握手、`Tool → McpToolInfo`（read_only_hint）、`CallToolResult → ToolResult`（structured_content 进 metadata、is_error 转 error context）、`apply_tool_result_budget`（UTF-8 安全截断）、`timed`/`should_retry`、streamable-http 构建、`test_support::InProcessConnector`。 |
| `src/config.rs` | ~875（逻辑 ~485 + 测试） | `McpConfig`（读 `ResolvedConfig.extra["mcp"]` 已合并层）、`McpServerConfig{transport, auto_start, timeout_ms, restart, permissions, trusted}`、`TransportSpec::{Stdio, Http}` 校验、`RestartPolicy`、`McpPermissions`、`StdioSandboxRuntime`、`SecretResolvingConnector`、服务器名禁 `.`。 |
| `src/manager.rs` | ~590（逻辑 ~375 + 测试） | `ManagedMcpClient`：惰性连接、指数退避有界重启（耗尽后冷却 4×max_delay）、请求超时、shutdown 取消在途、`HealthSnapshot`/`ConnectionState`；`should_retry` 触发单次强制重连重试；实现 `McpPeer`。 |
| `src/oauth.rs` | ~370（逻辑 ~150 + 测试） | PKCE：`begin_pkce_login` / `complete_pkce_login`（换码 + 存储）；`McpBearerProvider`（到期自动 refresh）；`OAuthHttpConnector`；测试与构造一律 `pawork_auth::http_client()`（`redirect(none)`），不使用 `Client::new()`。 |
| `src/sandbox.rs` | ~580（逻辑 ~390 + 测试） | stdio 托管：`StdioSpawner` trait / `SandboxedStdioSpawner`（唯一生产实现，走 `SandboxBackend::spawn_interactive`）/ `SpawnedStdio`（AsyncRead/AsyncWrite 适配，stdout 预算 8 MiB fail-closed 断连）；`apply_mcp_stdio_env_hygiene`（env_clear + untrusted allowlist + 追加 deny `PAWORK_API_KEY_*`，不改 network_mode）。 |
| `src/security.rs` | ~205（逻辑 ~115 + 测试） | `SecretRef{service, account}`（只序列化 locator；`resolve` 强制 `pawork.mcp.*` 前缀，Provider/OAuth 域 fail-closed）；`ResolvedSecret`（Debug/Display 恒 `[REDACTED]`）。 |
| `src/transport.rs` | ~400（逻辑 ~300 + 测试） | 传输配置（crate 私有 mod，类型 pub 但包外不可命名）：`StdioTransportConfig` / `HttpTransportConfig` / `TransportConfig`（携密字段 Debug 手写 redact、URL 打码 userinfo/query/fragment）、`McpConnector` trait（pub(crate)）、`DefaultConnector`。 |

## 3. 对外 API 面


- **配置入口**：`McpConfig::from_resolved(&ResolvedConfig)` / `from_value(&Value)`（读已按 global→workspace→session→run 合并后的 `extra["mcp"]`）；`servers: BTreeMap<name, McpServerConfig>`；服务器名非空且禁 `.`（进入工具命名空间）。
- `McpServerConfig::build_client(name, Arc<dyn SecretBackend>, Option<StdioSandboxRuntime>) -> Result<ManagedMcpClient, McpError>`：
  - stdio 传输缺 runtime 直接 `McpError::Config`（fail-closed）；http 不需要 runtime。
  - `runtime_options` 暴露请求超时（默认 30s）与 `RestartPolicy`（默认 max_attempts=1、base 200ms、cap 10s）。
- **传输规格**：`TransportSpec::Stdio{command, args, env: BTreeMap<String, SecretRef>}` / `Http{url, headers: BTreeMap<String, SecretRef>}`（serde tag=`kind`）。校验：仅 http/https scheme、拒 URL userinfo 与 fragment、明文 http 携密 header 仅 loopback 允许。
- **权限**：`McpPermissions { allowed_tools: BTreeSet<String>, allowed_workspaces: BTreeSet<String>, max_output_bytes: u64（默认 1 MiB） }`：
  - 空集 = 不限制；非空 = 白名单。
  - `allowed_tools` 双重生效：注册期过滤（不在名单的工具不进 registry）+ 调用期复核。
  - `allowed_workspaces` 调用期按 `context.workspace_id` 校验，违规返回 Authorization 失败结果。
  - `max_output_bytes` 是 codec 输出预算（`apply_tool_result_budget` 的上限来源）。
- **边界类型**：`McpServerCapabilities { tools, resources, prompts: bool }`（服务器 initialize 广播；未广播 tools 的服务器跳过工具注册）；`McpToolInfo { name, description, input_schema, read_only }`；`McpToolCall { name, arguments }`；`McpPeer` trait（`server_capabilities` / `list_tools` / `call_tool`）是 manager 与 capabilities 之间的抽象缝，测试用 in-process peer 替换。
- **受管客户端**：`ManagedMcpClient` 实现 `McpPeer`；另有 `ping()`、`health() -> HealthSnapshot{state: ConnectionState, transport, last_error, last_connected_at, restart_attempts, max_restart_attempts}`、`shutdown()`（5s 优雅关闭）。
- **能力桥**：`register_server_tools(registry, server, peer, permissions, trusted, host_trusted)` → `McpCapabilities::discover`（握手能力 + list_tools）+ 白名单过滤 + 注册，返回 descriptors；`register_discovered_tools` 供已有发现结果复用。`McpToolAdapter` descriptor 规则：
  - 注册名 = `namespaced_name(server, tool)`：`{server}.{tool}` 拼接后把 `[A-Za-z0-9_-]` 之外的字符全部折叠为 `_`（上游 Provider 拒绝带 `.` 等字符的工具名，HTTP 400，2026-09-16 `echo.echo` 实证）。
  - `read_only_hint=true` → `ToolCapability::ReadOnly` + `requires_approval=false`。
  - 否则 → `ExternalPlugin` + `requires_approval=true`（descriptor 叠加闸生效，policy 放行后仍需 resolver 确认）。
  - `allowed_in_untrusted_workspace = read_only || trusted`，且注册期 `trusted &&= host_trusted`（MCP 配置的 trusted 不得越过宿主信任地板）。
- **Secret 域**：`SecretRef::new(service, account)` / `.resolve(&dyn SecretBackend) -> Result<ResolvedSecret, McpError>`；service 必须 `pawork.mcp.*` 前缀（Provider/OAuth 命名空间 fail-closed）；`ResolvedSecret` 与全部 transport 配置 Debug/Display 手写 redact；`McpError` 文案不含明文。
- **OAuth**：`begin_pkce_login(PkceFlowConfig) -> PkceSession`；`complete_pkce_login(session, code, state, http, backend, display_name) -> StoredCredential`；`McpBearerProvider::bearer()`（到期自动 refresh）；`OAuthHttpConnector`（transport 层注入 `Authorization: Bearer`，拒绝配置里已有 Authorization header；token 轮换要求重建 transport）。
- **stdio 托管**：`StdioSpawner` trait + `SandboxedStdioSpawner` + `SpawnedStdio`（`pub use` 于 `mcp`）；`apply_mcp_stdio_env_hygiene(&mut SandboxPolicy)`。
- `McpError` 变体：`Config / Transport / Protocol / Disconnected / Timeout(Duration) / Cancelled / PermissionDenied / Secret / OAuth / Registry(ToolRegistryError)`。
- 内部但值得知道：`codec.rs` 私有；`StdioTransportConfig`/`HttpTransportConfig` 在私有 `mod transport`——以其为参数的公开函数实际只能由 crate 内装配。

宿主使用 [tools](tools.md) 的 `ToolScheduler::with_tools` 追加 Host 工具，保留内置与 MCP 工具以及旧 Run 快照。


## 4. 关键行为与语义


1. `McpConfig::from_resolved` 解析校验；每个 server `build_client`：stdio 必须携 `StdioSandboxRuntime{backend, policy, workspace_roots}`（缺失 fail-closed），构造 `SecretResolvingConnector`。
2. 首次请求触发惰性 connect：`SecretRef` 逐项 `resolve`（仅 `pawork.mcp.*`）→ 组装 transport → stdio 经 `SandboxedStdioSpawner.spawn`（`apply_mcp_stdio_env_hygiene`：env_clear + untrusted allowlist + 追加 deny `PAWORK_API_KEY_*`；`spawn_interactive` 进沙箱；stdout 预算 8 MiB 超限断连）→ codec initialize 握手。
3. `register_server_tools` 发现工具 → 白名单过滤 → `McpToolAdapter` 以清洗后的 `{server}_{tool}` 注册进同一 `ToolRegistry`（与内置工具同表同闸门）。
4. 调用：scheduler 闸门（ExternalPlugin 走 descriptor 叠加审批）→ adapter 校验 workspace/tool 白名单（违规 → Authorization 失败结果而非异常）→ 非对象输入拒绝 → `ManagedMcpClient::call_tool`（超时/取消/`should_retry` 单次强制重连重试）→ codec 转换 → `apply_tool_result_budget` 按 `max_output_bytes` UTF-8 安全截断（structured_content 超预算整体丢弃并标记 truncated）。
5. 断连恢复：指数退避（base×2^n 封顶 max_delay）至 `max_attempts` 耗尽 → `Disconnected`；冷却 4×max_delay 后允许再试；crash 重启复用同一 spawner（沙箱保证不降级）。

Policy 和审批闸门由 tools 的同一 scheduler 承担，MCP 不另设旁路。


## 5. 依赖与 feature

内部依赖 domain、tools、workspace、exec、auth。沿用原 MCP 的 rmcp、reqwest、url、tokio、async-trait、serde/serde_json、thiserror；无新第三方生产依赖、无 feature。Host 在 app 中装配，engine 只消费 domain 的 AgentTool。

## 6. 红线与不变量

stdio 经 SandboxBackend，环境清空并拒绝 Provider Secret 透传；HTTP 的 URL/携密头校验保持原样。SecretRef 仅允许 pawork.mcp.* 命名空间；权限、信任钳制、工具命名冲突、输出预算与取消全部沿用既有实现。MCP 工具与内置工具共用 ToolRegistry 和 ToolScheduler。

## 7. 测试与 golden 资产

原 MCP 全部内联测试随源码迁移，执行 `bash scripts/test.sh mcp tools`。包括配置拒绝、HTTP 安全、OAuth refresh、Secret 域、工具权限/预算、单注册表、超时/重连/取消及沙箱子进程。公开源码 SDK 隔离测试的路径随迁，不删除拒绝回归。

## 8. 相关文档

[tools](tools.md) · [app](app.md) · [auth](auth.md) · [exec](exec.md) · [跨包链路](../flows.md) · [架构](../../architecture.md)。HTTP transport 不经进程沙箱；ManagedMcpClient 无后台心跳，断连在请求时发现。配置的 auto_start 由宿主编排。私有 transport 模块继续隔离 SDK 装配类型。
