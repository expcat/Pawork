# P2-1：HTTP 运行时

> Phase 2 · 首个真实 Provider · 状态：🟢已实现 Provider 基线；通用抽离待后续收敛 · 依赖：P0-4

**最终目的**：建立跨平台的 HTTP 运行时（超时 / 代理 / 自定义 header / trace ID / 请求取消），先作为所有 Provider 网络访问的统一底层；在 P17 User Hooks / Marketplace 与 Forge Adapter 接入前，将 Provider 无关部分抽为 `http-runtime`，避免通用模块反向依赖 `provider-runtime`。

**涉及范围**：Phase 2 基线落于 `provider-runtime::http`；后续新增 `http-runtime`，由 `provider-runtime`、`user-hooks`、`marketplace` 与 Forge Adapter 共同依赖，`http-runtime` 不依赖 `provider-api` 或具体 Provider。

## 细分步骤

1. **选型并封装 HTTP 客户端** —— 目的：统一底层。
2. **超时 / 代理 / 自定义 header** —— 目的：覆盖部署环境需求。
3. **trace ID 贯穿** —— 目的：可关联日志与请求。
4. **请求取消** —— 目的：可在流式中途取消。
5. **通用 runtime 抽离** —— 目的：冻结 Provider 无关的 request/response、timeout、proxy、header allowlist、trace、cancel、retry 接口并迁入 `http-runtime`；Provider-specific SSE/JSONL 解析与错误映射仍留 `provider-runtime`。
6. **迁移兼容** —— 目的：`provider-runtime` 改为依赖 `http-runtime`，保持现有 Provider contract 行为不变；Hooks/Marketplace/Forge 只依赖通用层，不复制 HTTP client 或重试实现。

## 主要产出物

- `provider-runtime` 的 HTTP 层
- 通用 `http-runtime` crate 与 Provider 基线兼容迁移（后续收敛）

## 验收标准

- [ ] 跨平台（三平台）行为一致
- [ ] 超时与取消有效
- [ ] `http-runtime` 无 Provider 依赖，`provider-runtime` 单向依赖它；User Hooks / Marketplace / Forge 不反向依赖 Provider runtime
- [ ] 抽离前后现有 Provider Contract Tests 行为一致

**相关文档**：[providers](../docs/features/providers.md) · [ROADMAP](../ROADMAP.md)
