# ENV-07 · 网关媒体供应商与账单环境验收

> 状态：当前核查完成，独立 review PASS；成功往返与实际费用仍待验。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#缺环境与外部条件)

## 目标

验收真实图像、视频、搜索供应商、套餐权限和账单来源。

## 当前依据与读取范围

本轮重核当前合法账号、正式目录、启用态与按源码选择的端点，用必要最小请求核图像、原生视频和 Hosted Search；费用来源与生成结果分别记录。既有模拟消费者验收可复用其同版链路边界，不当作真实供应商扣费证据。

[gateway Spec](../spec/crates/gateway.md)、[模型网关](../spec/model-gateway.md)、[control-plane Spec](../spec/crates/control-plane.md)。

## 验收范围

- 分别准备真实供应商与套餐能力，执行实际图像 / 视频 / 搜索往返。
- 核对账单的权威来源、请求身份和账本关联，模拟上游不作为真实扣费证据。

本轮用临时隔离的正式 Gateway HTTP → AppCore → Provider → SQLite 链。只装配本项现行账号，凭证在进程内进入 MemoryBackend，保留供应商当前代理设置；客户端连接临时 loopback listener。现有用户 Gateway、认证与设置文件不作为本项写入集。

本轮选择 `qwen-token-plan/wan2.7-image`、`qwen-token-plan/happyhorse-1.1-t2v` 与 `xai/grok-4.7`。发请求前核正式目录、启用态与能力证据；图像仅发一次生成请求，视频仅提交一次，既有正式 Provider 固定 720P / 16:9 / 5 秒。有合法视频任务 ID 才有界查询原任务，不自动补交；搜索带 `web_search: true` 并通过能力闸门。

## 费用来源与证据边界

Gateway 调用记录的 Token、生成图像张数与 `planned_video_seconds` 分别表示用量或计划时长。`complete_record` 只补终态、时间和错误，不能提供实际费用；Gateway 当前没有供应商账单接线，`cost` 保持未知。Native Run 的模型定价计算是 estimated，现行 Go 额度读口也不能代替 Qwen / xAI 的 actual 费用来源。

本项分别对齐 HTTP call ID、认证 client、task / group / subtask 与 SQLite 记录，保留失败、可空 Token / cost 和 `unknown_cost_calls`。没有权威费用来源时保持未知，不把失败请求推断为零扣费。账单产品接线缺口与供应商套餐、授权或网络条件分开登记。

[AC-07](accept-gateway-consumers.md)、[AC-08](accept-momai-task-usage.md)、[AC-09](accept-yingmai-task-usage.md) 的同版消费者模拟证据只证明请求和记录关联；[ENV-04](env-04-glm-search.md) 的真实 MCP 搜索不代替本项 Gateway Hosted Search。

## 完成条件

- 每类能力与套餐结果有真实供应商证据，拒绝与失败如实记录。
- 账单来源明确，不用推算或 mock 数字冒充实际扣费。

## 完成记录

2026-10-12，以 `main` / `fa459dfdaef2521acb44fd43d23ed0a0ce44224b` 为基线，Grok 4.7 xhigh 执行对话 `01a13002-2e05-7e42-ae91-d7ab714afe23` 完成本轮核查。证据目录为 `/tmp/pawork-env07-media-billing/evidence`；SQLite 位于本项临时 instance。本轮没有生产源码改动。

### 账号、目录与通道

qwen 使用现行 legacy API key，配置 base 为 `https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1`、`use_proxy=false`。xAI 使用现行已选 OAuth。原配置的 6 个 provider 条目在临时配置中收窄为 qwen 与 xAI，MemoryBackend 只载入这两类凭证；其它官方通道因缺凭证在本地拒绝。保留当前代理配置，不由 `use_proxy=false` 推断系统代理完全关闭。

正式 Gateway 图像、搜索与视频目录均返回 200，分别有 2、5、1 个条目。图像条目 `wan2.7-image` 的 `image_output=true` / `text=false`，搜索条目 `grok-4.7` 的 `web_search=true`，三个所选模型均未被对应 denylist 禁用。视频目录只表示客户端配置可用，不证明套餐允许生成。

`prep.json` 中 xAI 的 `api.x.ai/v1` 是配置默认 base；按 [XaiProvider](../../crates/providers/src/channels/xai.rs) 的 OAuth 分支，它改为 `https://cli-chat-proxy.grok.com/v1`，目录 `/models` 与 Responses 共用该 base 和订阅头。远端目录的 Responses transport 才声明 Hosted WebSearch。此处依据源码和目录回执，没有捕获实际出站 URL，不把配置元数据当抓包证据。

### 三次请求与持久记录

| 操作 | 正式 Gateway 请求 | Gateway 回执 | call ID |
| --- | --- | --- | --- |
| 图像 `qwen-token-plan/wan2.7-image` | `POST /v1/chat/completions`，简单红色方块提示，一次 | 429 / `rate_limit_exceeded` | `gateway-27112f59c23ac984f5a404db7880bd69` |
| 视频 `qwen-token-plan/happyhorse-1.1-t2v` | `POST /v1/video/tasks`，同一媒体提示，一次 | 429 / `rate_limit_exceeded` | `gateway-62931445d58acc30d9b9869d18d27216` |
| Hosted Search `xai/grok-4.7` | `POST /v1/chat/completions`，官方搜索工具名称问题，`web_search=true`，一次 | 429 / `rate_limit_exceeded` | `gateway-a6e7003af90037131bfa7d49d54fa105` |

三次均未返回成功媒体或搜索正文；视频没有上游任务 ID，查询 0 次，生成请求均未重发。429 是已捕获的 Gateway 状态：[GatewayError](../../crates/gateway/src/backend.rs) 将 Provider 的 `RateLimited` 与 `QuotaExceeded` 归为相同错误码，未捕获上游原始 HTTP 状态，不能据此确定 429 根因、套餐权限或实际扣费。

`POST /v1/usage/query` 返回 200。HTTP 的 `error.call_id` / `pawork_usage.call_id` 与 SQLite `task_usage_calls` 的三条 payload、查询记录逐字段一致：client=`env07`、task=`env07-media`、group=`env07-group`，subtask 分别为 `env07-image` / `env07-video` / `env07-search`；operation 分别为 image / video / text，provider / model 保持原路由，均 `status=failed` / `tokens=null` / `cost=null`。

汇总 `records=generation_calls=failed=3`、`unknown_token_calls=unknown_cost_calls=3`；`currencies=[]`，费用表 `usage_records` 为 0 行。已知 Token 聚合显示的 0 不代表这三次用量已知；视频的 `planned_video_seconds=5` 也不代表已生成 5 秒。指挥只读 SQLite 的独立核对见 `coordinator-sqlite-comparison.json`。

### 验证、回收与待验

外部临时包首次编译因依赖解析与仓库不同而主动终止，`build-offline.exit=-15`；这是构建过程回执，不是供应商失败。改为临时 workspace 包后，`env RUSTC_WRAPPER=kache cargo build -p env07-driver --offline` exit 0，`run-driver` exit 0。运行退出码只证明 driver 正常结束，三次供应商尝试仍按上述失败登记。原命令、stdout 与退出码分别保留在同名 `.command.json` / `.stdout` / `.exit`；实际运行源码为 `sources/main.rs`，workspace 与外部 manifest 分别为 `sources/Cargo.workspace.toml` / `sources/Cargo.external.toml`。

临时 `crates/env07-driver` 已回收，`Cargo.lock` 回到基线，当前只剩本项两份文档差异。12:46:54 UTC 的持久摘要观察与执行收尾相比，`auth.json`、全局 `config.toml`、`desktop.json` 一致，`mcp-auth.json` 仍不存在；凭证内容没有写入证据。临时 Gateway 随 driver 退出收尾。

已实现与同版模拟消费者验收继续复用；本轮完成真实 Gateway 失败尝试与账本关联核实。成功图像 / 视频 / Hosted Search、套餐与权威费用仍未验收。实际账单产品接线缺口单独保留，不笼统归为环境缺失，本项不标 ✅。

```text
Validated: env RUSTC_WRAPPER=kache cargo build -p env07-driver --offline（exit 0）；隔离 driver 一次（exit 0，三次 Gateway 429）；指挥只读 HTTP / SQLite 逐字段核对通过
Targeted regressions: none（无生产源码变化；复用 AC-07～09 同版消费者模拟证据）
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

独立 review：gpt-6.1-sol max 对话 `01a13032-a8a1-7063-8329-269390901402`，turn `01a13032-af66-7be2-a78a-04f99e00ba22`，实际终态 completed / error=null，结论 PASS；阻断与非阻断问题均无。受审快照 `FINAL-review.diff` 为 10222 字节，SHA256 `0b5eb03991081b8c41ca301775636a7642d3130dc26bb7c923df953fcac5ac78`；指挥核完整 diff 和两份文件摘要一致后，仅同步审查元数据。原回执保留于证据目录 `REVIEW-PASS.json`。

