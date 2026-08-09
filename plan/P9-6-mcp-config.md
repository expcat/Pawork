# P9-6：MCP Config

> Phase 9 · MCP · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P8-1

**最终目的**：实现 MCP 配置（workspace scoped / global），让 MCP server 列表与配置可按工作区与全局管理。

**涉及范围**：`resource-loader`、`mcp-client`

## 细分步骤

1. **配置 schema（workspace/global）** —— 目的：作用域管理。
2. **server 列表加载** —— 目的：发现配置的 server。
3. **与 Resource Loader 协作** —— 目的：统一加载。
4. **测试** —— 目的：作用域正确。

## 主要产出物

- MCP Config

## 验收标准

- [x] workspace/global 作用域正确

## 验证记录（2026-08-09）

- `McpConfig` 从 `config-service::ResolvedConfig` 的已合并 `mcp.servers` keyed map 解析，不自行访问任意文件；global → workspace 的同名 Server 字段递归覆盖已有测试。
- Server 配置可直接构造延迟解密 connector 与 `ManagedMcpClient`，timeout / restart 语义有定向断言；后续 Resource Loader 只需向 `config-service` 提供配置层。

**相关文档**：[mcp](../docs/features/mcp.md) · [skills](../docs/features/skills.md) · [ROADMAP](../ROADMAP.md)

> Plan/实现范围订正（2026-08-10 P9 review）：本任务「涉及范围」原文提及 `resource-loader`，但实际实现中 `resource-loader` 的 Profile v1 把 MCP 维度显式划给 P17-5 v2（`crates/resource-loader/src/profiles.rs` 注释），`McpConfig` 仅从 `config-service::ResolvedConfig.extra["mcp"]` 强类型投影，resource-loader 不参与 MCP 配置解析。该范围不符项不改变 P9-6 的 🟢 与验收（workspace/global 作用域正确），仅记录以免误读为「resource-loader 已承载 MCP 配置」。
