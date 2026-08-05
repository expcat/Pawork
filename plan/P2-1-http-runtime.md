# P2-1：HTTP 运行时

> Phase 2 · 首个真实 Provider · 状态：🟡未开始 · 依赖：P0-4

**最终目的**：建立跨平台的 HTTP 运行时（超时 / 代理 / 自定义 header / trace ID / 请求取消），作为所有 Provider 网络访问的统一底层。

**涉及范围**：`provider-runtime`

## 细分步骤

1. **选型并封装 HTTP 客户端** —— 目的：统一底层。
2. **超时 / 代理 / 自定义 header** —— 目的：覆盖部署环境需求。
3. **trace ID 贯穿** —— 目的：可关联日志与请求。
4. **请求取消** —— 目的：可在流式中途取消。

## 主要产出物

- `provider-runtime` 的 HTTP 层

## 验收标准

- [ ] 跨平台（三平台）行为一致
- [ ] 超时与取消有效

**相关文档**：[providers](../docs/features/providers.md) · [ROADMAP](../ROADMAP.md)
