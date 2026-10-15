# ENV-07 · 网关媒体供应商与账单环境验收

> 状态：正式 Gateway 失败记账已核实；成功图像 / 视频 / Hosted Search 往返与实际费用未验收。阻塞：三类请求均 Gateway 429，需供应商解除；Qwen / xAI 没有可接的权威账单来源。更新：2026-10-13。

[返回 ROADMAP](../ROADMAP.md#环境待验)

## 目标

验收真实图像、视频、搜索供应商、套餐权限和账单来源。

## 当前依据与读取范围

[gateway Spec](../spec/crates/gateway.md)、[模型网关](../spec/model-gateway.md)、[control-plane Spec](../spec/crates/control-plane.md)。

## 验收范围与完成条件

- 每类能力与套餐结果有真实供应商证据，拒绝与失败如实记录。
- 账单来源明确，不用推算或 mock 数字冒充实际扣费。

## 费用来源边界

Gateway 调用记录的 Token、生成图像张数与 `planned_video_seconds` 分别表示用量或计划时长；`complete_record` 只补终态、时间和错误，`cost` 初始为 None。Gateway 没有供应商账单接线，没有权威来源时费用保持未知，不把失败请求推断为零扣费。Native Run 的模型定价是 estimated，Go `/usage` 额度读口也不能代替 Qwen / xAI 的 actual 费用。仓库内没有可接的 Qwen / xAI 账单客户端，这是外部条件，不是本机可补的接线。

## 已核实（2026-10-12）

临时隔离的正式 Gateway HTTP → AppCore → Provider → SQLite 链，三类请求各一次：

| 操作 | 请求 | 结果 |
| --- | --- | --- |
| 图像 `qwen-token-plan/wan2.7-image` | `POST /v1/chat/completions` | Gateway 429 / `rate_limit_exceeded` |
| 视频 `qwen-token-plan/happyhorse-1.1-t2v`（720P / 16:9 / 5 秒） | `POST /v1/video/tasks` | Gateway 429；无上游任务 ID，查询 0 次 |
| Hosted Search `xai/grok-4.7`（`web_search=true`） | `POST /v1/chat/completions` | Gateway 429 |

三条 `task_usage_calls` 与 HTTP `call_id`、client / task / group / subtask 逐字段一致，均 `status=failed`、`tokens=null`、`cost=null`；汇总 `unknown_token_calls=unknown_cost_calls=3`、`usage_records` 0 行。Gateway 把 `RateLimited` 与 `QuotaExceeded` 都映射为 429，未保留上游原始状态，不能据此确定根因。

现行回归锚点：`gateway_tests` 的 `gateway_http_routes_models_completions_and_auth_without_exposing_provider_secrets`、`gateway_models_purpose_filter_and_multimodal_gates`、`gateway_native_video_submits_once_queries_and_redacts_failures`；control-plane `journal_migrates_preserves_usage_and_resumes_video_without_double_counting`。

## 剩余待验

1. **成功往返**：图像 `wan2.7-image`、视频 `happyhorse-1.1-t2v`（有上游任务 ID 才查询，参数保持 720P / 16:9 / 5 秒）、Hosted Search `xai/grok-4.7` 各一次成功。前置：对应账号不再返回 Gateway 429。失败保持 `failed` 且 tokens / cost 为 null；视频目录 200 不证明套餐允许生成。
2. **权威 actual 费用**：前置为供应商提供可核验的账单接口且当次请求被实际计价。没有该来源时 cost 保持未知，不用 Estimated、Go `/usage` 或失败请求的 0 关闭本项。

## 阻塞（2026-10-13）

两项都是外部条件：成功往返取决于供应商解除 429，actual 费用取决于供应商提供可核验账单接口。2026-10-13 文档切片未重发真实请求，状态沿用 2026-10-12 证据；`cost=null` 保持未知，不记为零扣费。
