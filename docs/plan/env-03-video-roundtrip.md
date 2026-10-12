# ENV-03 · MM-2 视频真实往返验收

> 状态：当前端点、启用模型与素材已核实；两次视频理解请求均为 HTTP 429，成功往返仍待供应商条件。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#缺环境与外部条件)

## 目标

在支持的视频端点与账号上证明真实往返。

## 当前依据与读取范围

本轮重新核当前凭证与 MM-2 视频输入链；按现行端点 gate 和专项范围验收，不以日常文本模型缺视频能力或旧网络失败推断整个环境不可用。

[模型网关](../spec/model-gateway.md)、[providers Spec](../spec/crates/providers.md)、[验证规格](../spec/verification.md)。

## 验收范围

- 准备对应端点、账号与可用视频输入，按现行 MM-2 链路执行。
- 记录实际提交和结果，不用模拟响应或文件存在代替供应商往返。

## 完成条件

- 真实端点接收并返回对应输入的结果，失败有真实终态。
- 证据注明账号/能力范围但不包含 Secret，未支持端点不伪装通过。

## 完成记录

2026-10-12 完成本轮环境核实与实际请求记录，视频理解成功验收仍待供应商条件。实时目录可读和静态 gate 命中，不能证明该账号当时的视频请求额度或授权可用。

本次经正式 `FileBackend` 解析现有 `qwen-token-plan` default 凭证，仅在请求进程内使用。正式基地址为 `https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1`。实时目录返回 14 个模型，`qwen3.8-max` 与 `qwen3.8-flash` 均在目录、未禁用、`video_input=true`、传输为 Chat Completions。本次选用 `qwen3.8-max`；官方公开视频素材为 `https://help-static-aliyun-doc.aliyuncs.com/file-manage-files/zh-CN/20241115/cqqkru/1.mp4`。

临时 Rust driver 调用正式 `ApiKeyChannelProvider::list_models` 和 `ModelProvider::stream`，提交 `ContentPart::Video`；不经 Host、GUI 或网关账本，不能当这些链路的验收。两次调用间隔约 35 秒，均为 `max_output_tokens=64`、无工具，返回归一化 `RateLimited` / HTTP 429，无视频理解正文；`env03-video-live-1.exit` 与 `env03-video-live-2.exit` 均为 1，保留原失败。错误分类可重试，不据此断言具体限流原因或套餐永久不支持。

driver 读取 `use_proxy=false` 后没有传入显式全局代理，但保留默认 `system_proxy=true`，不能宣称完全绕过系统代理。读取超时临时设为 180 秒、总探测预算 200 秒；实际结果是 HTTP 429，不能当产品默认超时的验证。driver 没有写入持久配置、认证或禁用列表；仓库差异只含本计划与 ROADMAP，临时 crate 已回收，Cargo.lock 无差异。

证据在 `/tmp/pawork-env03-video-roundtrip/evidence`。除两组原命令、stdout 与 exit 外，指挥通过执行线程 `01a12f98-7fa2-7b61-8b85-d4f15d1c3414` 的真实工具记录补存 `driver-add.rs`、`driver-fix.diff`、`driver-Cargo.toml` 和 `coordinator-source-provenance.json`；`driver-used.rs` 仅按记录中的一行修正还原，不伪称历史文件摘要。stdout 的 credential masked 字段已脱敏为 `[redacted]`，未保存明文 Secret。

独立 review：`gpt-6.1-sol / max`，线程 `01a12faf-9326-7621-8128-c2d3c8967860`，2026-10-12 PASS；核原日志、两次 exit 1、driver 原始回执、受审差异与摘要，阻断／非阻断问题均无。PASS 不代表视频成功。

后续仅在供应商允许当前登记端点／模型的请求后复验真实视频理解结果；本轮不放宽视频 gate、不启用禁用模型，也不以模拟响应替代成功。

```text
Validated: cargo run --offline -p env03-video-probe --bin env03-video-probe（exit 1）；直接执行该临时二进制（exit 1）；两次正式目录与视频请求，doc-links / git diff --check
Targeted regressions: none（无生产代码改动；两次请求均非测试通过）
Full workspace gate: NOT RUN（当前未设置全量门禁）
```
