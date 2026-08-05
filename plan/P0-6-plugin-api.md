# P0-6：插件协议骨架

> Phase 0 · 架构与协议冻结 · 状态：🟡未开始 · 依赖：P0-2、P0-5

**最终目的**：预留插件/扩展的统一协议骨架（manifest/生命周期事件），为 MCP、WASM、hook 留出对接点。本任务只定义接口、不实现宿主——尽早冻结对外扩展契约，避免后期反复改。

**涉及范围**：`plugin-api`

## 细分步骤

1. **定义 Plugin Manifest** —— id/version/api_version/permissions/capabilities。目的：插件元数据与签名校验基础。
2. **定义生命周期事件枚举** —— load/unload/start/stop/register。目的：hook 派发与宿主生命周期管理。
3. **定义 Plugin Trait（骨架）** —— 占位接口，P9/P10 再落地宿主。目的：尽早冻结对外扩展契约。
4. **复用 capability/权限声明** —— 与 tool-api 的 capability 对齐。目的：与工具/权限模型统一，减少特例。

## 主要产出物

- `plugin-api` crate：Manifest + 生命周期事件 + Trait 骨架

## 验收标准

- [ ] 仅定义接口、不实现宿主
- [ ] 与 tool-api / agent-domain 术语一致

**相关文档**：[plugins](../docs/features/plugins.md) · [mcp](../docs/features/mcp.md) · [ADR-011 MCP 第一](../docs/adr/ADR-011-mcp-first-extension.md) · [ADR-012 WASM](../docs/adr/ADR-012-wasm-first-plugin.md) · [ROADMAP](../ROADMAP.md)
