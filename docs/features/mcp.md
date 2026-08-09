# MCP 支持

## 职责

`mcp-client` 是 Pawork 的 MCP Client 边界，也是第一种外部扩展接入方式。它负责连接外部 MCP Server、发现 Tools / Resources / Resource Templates / Prompts，并把 Tool 投影到现有 `tool-runtime`；Agent Engine 不感知具体 Server 名称、transport 或 `rmcp` 实现。

## 设计要点

- **协议隔离**：固定使用 `rmcp = 2.2.0`，SDK、握手与 transport 类型只留在 `mcp-client` 内，后续协议升级不扩散到 Core；对应 workspace MSRV 为 Rust 1.85。
- **两种 Transport**：本地 Server 使用 stdio 子进程；远程 Server 使用 Streamable HTTP，不支持已弃用的旧 HTTP+SSE transport。
- **故障隔离**：每个 Server 由独立 manager 持有连接、健康状态、超时与重启状态；Server 断连或崩溃只使该 Server 的调用失败。
- **统一工具边界**：发现到的 MCP Tool 使用 Server 作用域名称注册为 `AgentTool`，沿用 `ToolRegistry`、`PolicyEngine` 与 `ToolResult`，不会在 Agent Loop 中增加 Provider/Server 特例。
- **确定性配置**：`config-service` 先按 global → workspace 等既有优先级完成合并，`mcp-client` 再解析合并结果中的 `mcp.servers` keyed map；workspace 只覆盖同名 Server 的字段。

## Transport 与生命周期

stdio transport 负责显式命令、参数、可选工作目录和环境注入，并在子进程退出时关闭当前 session。Streamable HTTP transport 负责 endpoint 与请求头。两者在初始化时完成 MCP `initialize` 握手，并经统一 peer 接口提供：

- 能力发现与 Tool 调用；
- Resource 读取与 Prompt 获取；
- `ping` 健康检查、调用 timeout 与主动 shutdown；
- Agent cancellation 向 MCP request cancellation 的传播；
- 有界重启与脱敏后的结构化生命周期日志。

transport 配置的 `Debug` 输出必须脱敏；URL userinfo / fragment 被拒绝，含 Secret 的非 loopback HTTP endpoint 必须使用 HTTPS。Secret 只在启动进程或组装 HTTP 请求前解析，不能进入配置快照、错误文本或日志字段。stdio 子进程收到 Secret env 时，其 stderr 内容整体脱敏，仅保留有界 chunk 的字节计数。

## 能力映射

| MCP 能力 | Pawork 接入方式 |
| --- | --- |
| Tools | 转换为带 Server 命名空间的 `AgentTool`，注册到 `ToolRegistry` |
| Resources | 仅在初始化能力中声明后，通过 `mcp-client` 的统一 peer 列举和读取 |
| Resource Templates | 随能力快照列举，调用方按 URI template 选择 |
| Prompts | 通过统一 peer 列举并按参数获取 |

MCP Tool 统一声明为外部插件能力。调用前先执行该 Server 的 allowlist 与 Policy 判定；输入必须是 JSON object。返回后执行硬字节上限，文本按 UTF-8 边界截断、超限二进制内容丢弃，并以 `ToolResult.truncated = true` 明确标记；`structuredContent` 仅在同一预算内保留到 metadata。

## 配置与作用域

Server 使用 keyed map 而不是数组，以复用 `config-service` 的递归 object 合并语义。全局层可声明共享 Server，工作区层可覆盖同名 Server 字段，也可增加仅该工作区可见的 Server。`mcp-client` 只读取 `ResolvedConfig.config.extra["mcp"]`，不自行读取任意文件。合并后的 schema 示例：

```json
{
  "servers": {
    "filesystem": {
      "transport": {
        "kind": "stdio",
        "command": "mcp-filesystem-server",
        "args": ["--root", "."],
        "env": {
          "API_KEY": {
            "kind": "secret_ref",
            "service": "pawork.mcp.filesystem",
            "account": "default"
          }
        }
      },
      "auto_start": true,
      "timeout_ms": 30000,
      "restart": { "enabled": true, "max_restarts": 3, "delay_ms": 1000 },
      "trusted": false,
      "permissions": {
        "approval_mode": "ask_for_writes",
        "allowed_tools": ["read_file"],
        "allowed_workspaces": ["workspace-1"],
        "max_output_bytes": 1048576
      }
    }
  }
}
```

HTTP transport 使用 `kind = "http"`、`url` 与同样只接受 `secret_ref` 的 `headers` map。无效 transport、空命令、非法 URL、零 timeout / 输出上限或无效 restart 配置均在连接前 fail closed；inline Secret 在类型层不可表示。

## 安全

每个 MCP Server 独立配置 trust 与 capability policy。Tool 调用同时经过 Server allowlist 和现有 `PolicyEngine`，审批结果不会跨 Server 复用。Secret 注入通过 `auth-service::SecretBackend` 在每次连接 / 重连前即时解析引用。OAuth 复用 `auth-service` 的 PKCE、凭证存储、singleflight refresh 与 refresh-token rotation；`OAuthHttpConnector` 在请求前检查 token，发生轮换时重建 HTTP transport，不在 `mcp-client` 复制 Token 生命周期实现。OAuth credential 元数据由宿主从 `auth-service` 注入 connector，不进入 `mcp.servers` 的 Secret 字段。

安全不变量：

- Secret 明文不写入配置、数据库、日志和持久事件；
- 一个 Server 的授权不能访问另一个 Server 的 Secret 或审批结果；
- Tool 输出必须受硬上限约束；
- Server 崩溃、超时或取消不能中止 Agent Core；
- transport 与协议错误对外使用稳定、已脱敏的错误类别。

## 优先级

- P0：stdio、Streamable HTTP、能力发现、Tool 注册、健康/重启/取消、Server policy、输出限制、Secret 引用和 global/workspace 配置。
- P1：保护型 MCP Server 的 OAuth 接入；Phase 9 已与 P0 能力一并交付。

## 验收标准

- stdio 与 Streamable HTTP 均能完成初始化和调用。
- MCP Tools 可注册到 `tool-runtime`，Resources / Prompts 可发现和读取。
- Server 崩溃、断连、timeout 与 cancellation 均被隔离并产生脱敏日志。
- 每个 Server 的 allowlist、Policy、Workspace、Secret 与输出上限独立生效。
- global/workspace 合并确定且覆盖粒度为单个 Server 字段。
- OAuth access token 可解析、注入和刷新；轮换后的 access / refresh token 由 `auth-service` 持久化，连接使用新 bearer 重建。

## 相关文档

- [plugins](plugins.md) · [policy](policy.md) · [auth（OAuth）](auth.md)
- [安全验收](../quality/security-acceptance.md)
- [ADR-011 MCP 第一扩展机制](../adr/ADR-011-mcp-first-extension.md)
- [ROADMAP Phase 9](../../ROADMAP.md)
