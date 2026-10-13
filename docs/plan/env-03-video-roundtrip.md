# ENV-03 · MM-2 视频真实往返验收

> 状态：端点、启用模型与视频 gate 已核实；两次视频理解请求均为 HTTP 429，成功往返未验收。阻塞：`qwen-token-plan` compatible-mode 端点对该账号持续 429，需供应商侧解除。更新：2026-10-13。

[返回 ROADMAP](../ROADMAP.md#环境待验)

## 目标

在支持的视频端点与账号上证明真实往返。

## 当前依据与读取范围

[模型网关](../spec/model-gateway.md)、[providers Spec](../spec/crates/providers.md)、[验证规格](../spec/verification.md)。

## 验收范围与完成条件

- 按现行 MM-2 链路提交视频输入，真实端点接收并返回对应结果；失败有真实终态。
- 证据注明账号 / 能力范围但不包含 Secret；不用模拟响应、目录成功或文件存在代替供应商往返。

## 已核实（2026-10-12）

- `qwen-token-plan` 正式基地址 `https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1` 实时目录 14 个模型；`qwen3.8-max` / `qwen3.8-flash` 未禁用、`video_input=true`、传输为 Chat Completions。
- 经正式 `ApiKeyChannelProvider::stream` 两次提交公开视频素材（`max_output_tokens=64`、无工具），均返回归一化 `RateLimited` / HTTP 429，无理解正文。该临时 driver 不经 Host、GUI 或网关账本。
- 现行源码锚点：`crates/providers/src/request.rs` 的 `video_endpoint_supported` 与 `ContentPart::Video` → `video_url` 编码；回归 `video_reference_maps_to_documented_chat_wire`、`video_rejects_unverified_endpoint_and_unsafe_references_before_network`。

## 剩余待验

`qwen-token-plan` 上 `qwen3.8-max`（或 gate 内其它未禁用的 `video_input` 模型）对一条无 userinfo 的 http(s) 视频 URL 返回理解正文。前置：同一 compatible-mode base 不再 HTTP 429。不放宽视频 gate、不启用禁用模型。

## 阻塞（2026-10-13）

唯一阻塞是供应商限流（HTTP 429），不在本机可解范围。2026-10-13 文档切片未重发真实请求，状态沿用 2026-10-12 证据；`RateLimited` 终态不记为往返成功，也不以目录 200 或 gate 通过代替。
