# 本机模型网关

- 最近更新：2026-10-04
- 范围：第三方本机程序复用 Pawork 已连接模型的 HTTP API；MoMai 是消费者之一，接口不包含 MoMai 专属字段。
- 架构：[architecture](../architecture.md)；实现：[gateway](crates/gateway.md)、[app](crates/app.md)、[cli](crates/cli.md)；阶段状态：[ROADMAP](../ROADMAP.md)。

## 决策：GW-1 通用本机模型接口

Pawork 提供通用、可复用的外部模型访问接口，采用 OpenAI Chat Completions 兼容子集，由唯一 `pawork` 二进制托管。HTTP Gateway 独立于 GUI/core-api/ACP；初版未改变 Agent 事件或数据库 schema，2026-10-04 的任务日志扩展见下文。2026-09-27 按用户授权的包边界调整，将 HTTP/SSE 与客户端 token 提取到 `pawork-gateway`；AppCore 实现 `GatewayBackend`，继续负责模型路由、账户快照、租约与用量，CLI 负责进程装配。HTTP 沿用 hyper / hyper-util / http-body-util，无新增第三方依赖。

后端调用保持「prepare → run」两阶段：参数、模型与凭证准备失败先返回正常 HTTP 错误，再开始补全或发送 SSE 成功响应头。宿主的 `Completion` 为不透明快照，接入层不读取凭证或重做模型路由。

与 backlog G7/F6 的边界：保留“不内建对外账户池网关”的排除项；此次开放本机模型调用，不开放远程服务、厂商凭证导出、账户池管理或第三方账户选择接口。

## 决策：ADR-064 模型能力用途筛选（2026-10-01）

第三方（如 MoMai）与 Pawork 自身的选择面需要按用途过滤模型：识别图片、生成图片、识别视频、网络搜索。用途不是第二套能力体系——domain 新增 canonical 词汇 `ModelPurpose`（`text` / `image_input` / `image_output` / `video_input` / `web_search`，snake_case wire 名），映射既有 `ModelCapabilities` 能力位；Host `model_list` 响应条目增 `text` / `image_output`（GUI API 1.25，additive，旧 Host 缺字段按 text=true / image_output=false 解读）；Gateway `/v1/models` 条目增 `capabilities` 对象并支持 `?purpose=<name>`（可重复取交集，未知值 400）。

口径与红线：

- **缺省 = v1 兼容**：不带 purpose 的 `/v1/models` 只返回 text 模型，现有 OpenAI 客户端看到的仍是「可聊天」集合；图像生成模型须显式按用途查询。
- **宣告 = 授权 = 实现**：图像生成模型进目录依据 2026-09-23 qwen compatible-mode 实测（`default_image_output` 表）；同表 `default_text` 把它们收窄为 `text=false`。视频输入沿用既有 wire + 端点证据，无实测不新增默认表。
- **会话 fail-closed**：图像生成模型不能被设为会话模型 / 角色默认 / 子代理规则（Host 结构化错误 `model_not_text`）；CLI 直接指定模型与恢复会话同样在 Run 入口拒绝非 text 模型，自动命名执行前也复核该能力；Engine 不消费 `ImageOutput` 流事件（闸门保证不可达），Gateway 直连消费透传。
- **多模态输入**：v1.1 起 `chat/completions` 消息 content 接受数组（text / image_url / video_url part）。URL 只做语法边界校验（长度、无空白控制符、拒绝内嵌凭证），Host 不下载、不解码、不转存；data: URL 仅 `image/*;base64` 形状，受请求体 2 MiB 上限约束；远程图片 URL 单独限 8 KiB。media_type 取自前缀，https 图像 URL 按扩展名推断（未知回落 image/png）。带图片 / 视频的请求走既有 `capability_gate`，模型证据不支持即 400。
- **图像输出**：Chat 通道 wire 解析 image content part（qwen `{"type":"image","image":"<url>"}` 与 OpenAI `image_url` 形状）归一为 `ProviderStreamEvent::ImageOutput`；已声明的纯生图模型以 content parts 数组发送文本提示。非流式响应在 `choices[].message` 增 `images: [{"url": ...}]`（additive，文本模型恒空数组）；流式以 `choices[].delta.images` chunk 透传，均计入输出字节上限。
- **web_search 请求位**：`web_search: true`（顶层 additive）映射 hosted WebSearch 工具要求并经 `capability_gate` 校验；模型未声明 WebSearch 即 400，不静默降级。

## 原生视频任务扩展（2026-10-03）

Qwen Token Plan 原生视频走异步任务 API。compatible `/models` 没有视频型号，不据此判断套餐无权限；依据[阿里云 Token Plan 多模态文档](https://help.aliyun.com/zh/model-studio/token-plan-multimodal-gen)的端点契约，独立提供视频目录与任务路由，保持 Chat / canonical 用途字段不变。

| 方法与路径 | 行为 |
| --- | --- |
| `GET /v1/video/models` | 已连接且启用 Qwen Token Plan 时列出 `qwen-token-plan/happyhorse-1.1-t2v`；目录表示客户端配置可用，套餐权限仍以实际提交结果为准 |
| `POST /v1/video/tasks` | 仅 `{"model":"qwen-token-plan/happyhorse-1.1-t2v","prompt":"提示词"}`；原生异步提交一次，固定 720P、16:9、5 秒，返回 HTTP 202 |
| `GET /v1/video/tasks/{id}` | 按任务 ID 绑定的提交账号查询，不受当前账号选择影响；不生成新任务 |

任务响应为 `{"id":"default-api-key.供应商任务ID","model":"完整模型 ID","status":"PENDING","url":null,"error_code":null}`。状态限 `PENDING` / `RUNNING` / `SUCCEEDED` / `FAILED` / `CANCELED` / `UNKNOWN`；成功必须有 HTTP(S) 输出 URL。客户端立即保存 ID，成功后完整下载并归档，临时 URL 不作为唯一资产。原始错误 message、提示词及凭证不回显，失败码只保留固定白名单。

提示词限 1–8000 字。供应商 ID 最多 128 字节，只含 ASCII 字母、数字、连字符、下划线；网关返回账号限定的不透明 ID（最多 256 字节，允许点分隔），客户端须原样保存并查询，不能用供应商裸 ID 替代。路由拒绝查询参数和未知字段，复用既有认证 / Host / Origin / 请求体限制。提交和查询各 60 秒上限，供应商 JSON 各 1 MiB 上限，不自动轮询或重试。取消 / 超时只停止本地网络；上游可能已接受任务，不能承诺远端取消，也不能自动重交。任务查询始终读取提交账号，不修改当前账号选择；凭证被删除、替换或环境凭证失效后，查询可能失败。禁用模型会阻止新提交，已有任务仍可查询。

providers 从已配置的 `/compatible-mode/v1` 基地址推导同源原生根，Chat 与视频共用渠道基地址 / 代理装配；在 SecretBackend 同一事务中冻结凭证与账号身份。提交与查询均取得绑定账号的并发租约，并在完成、失败、取消或超时后释放；后台执行任务由网关退出流程等待收尾。domain 的 `VideoGenerationModel` / `VideoGenerationTask` / `VideoTaskStatus` 是共享类型，宿主必须明确实现三个视频端口，无无类型 JSON 或默认兼容实现。原生视频按条 / 秒计费，未写成 token 账本，不估造费用。本机模拟验证不能替代真实供应商套餐权限或消费者 GUI 验收。

## 启动和接入

先在 Pawork 中连接供应商、启用目标模型，再执行：

```bash
pawork gateway token issue --client momai
pawork gateway serve
```

第一条命令只在签发时打印 token；复制至 MoMai 设置中的网关令牌框。默认 API Base URL 为 `http://127.0.0.1:17432/v1`。其他应用或 OpenAI 客户端库填写同样的 Base URL、各自签发的 token，并从目录选择完整模型 ID。无需安装 Pawork SDK。

```bash
pawork gateway token issue --client another-editor
pawork gateway token list
pawork gateway token revoke <token-id>
pawork gateway serve --port 17433
pawork gateway status
pawork gateway shutdown
```

`--instance <name>` 是全局参数，签发、运行、查询、撤销必须指向同一实例。每实例有独立 `gateway.lock`、PID 和 token 目录；与 GUI 服务可并存。当前以前台命令运行，Ctrl-C / SIGTERM 会取消请求、等待用量落账与释放租约。现有 `service install`、顶层 `status/shutdown` 仍服务 GUI，不宣称已提供网关系统服务安装。

网关启动读取配置快照；修改启用集、端点等配置后重启网关。账号选择和认证修订在每次补全前读取，沿用已有选择/耗尽切换策略；token 撤销对下一次请求立即生效，已接收请求不会追溯中断。网关启动不打开会话、不启动 MCP、不执行 Agent loop 或工具，不清扫其他宿主的 Run。

## HTTP 契约 v1

所有请求均需 `Authorization: Bearer <gateway-token>`。仅监听 `127.0.0.1`，Host 必须匹配 `127.0.0.1:<实际端口>` 或 `localhost:<实际端口>`；如有 Origin，只接受与 Host 对应的 HTTP 同源。无跨域 CORS 授权。

| 方法与路径 | 行为 |
| --- | --- |
| `GET /v1/models` | 返回 `{"object":"list","data":[...]}`，仅包含已连接且启用的可选模型。条目含 `id`、`object:"model"`、`owned_by`、`display_name`、`context_window` 与 `capabilities`（`text` / `image_input` / `image_output` / `video_input` / `web_search` 布尔）。未知窗口为 0。v1.1 起支持 `?purpose=<name>`（可重复、取交集、未知值 400）；缺省仅返回 text 模型（v1 兼容）。 |
| `POST /v1/chat/completions` | 文本多轮上下文、普通响应或 SSE；无服务器会话状态。v1.1 起消息 content 可为数组（text / image_url / video_url part）并支持顶层 `web_search: true`；图像生成模型经 `message.images` / 流式 `delta.images` 返回生成图 URL。 |

模型 ID 为 `<provider>/<model>`，只按第一个 `/` 拆分，上游模型 ID 可以继续包含 `/`。不会把同名模型路由到其他供应商，也不回落到默认模型。

OpenCode Go 的上游 `x-opencode-session` 由网关在每次请求中使用同一个租约 / canonical 请求身份生成，客户端无需提供。普通 JSON 与 SSE 均适用，连续请求身份互不相同；它不表示服务器持久会话。

已声明的纯生图模型使用非流式上游请求；Token Plan 的普通 JSON `output.choices` 被适配器归一为图片输出与完成事件。客户端仍使用本文的 Chat Completions API，不新增 `/images/generations`；HTTP 200 的错误正文不算成功；普通 JSON 直接解析，复用与 SSE 相同的 content part 映射。缺完成标记、非成功终态、缺图片或带凭证 / 非 HTTP(S) 图片 URL 均拒绝；JSON 正文最多 1 MiB，等待正文期间可取消。

最小请求：

```json
{
  "model": "opencode-go/glm-5.3-flash",
  "messages": [{"role":"user","content":"用一句话介绍自己。"}],
  "stream": true,
  "stream_options": {"include_usage": true}
}
```

支持字段：

- `messages`：非空数组，最多 1024 项；每项 `role` 为 system/user/assistant，`content` 为字符串或 part 数组（`{"type":"text","text":...}` / `{"type":"image_url","image_url":{"url":...}}` / `{"type":"video_url","video_url":{"url":...}}`；空数组、未知 part 类型、非法 URL 400）。
- `stream`：缺省 false；`stream_options.include_usage` 仅用于流式请求。
- `max_tokens` 或 `max_completion_tokens`：正整数，二者不能同时出现。
- `temperature`：0–2。
- `response_format`：text / json_object / json_schema。Chat 与 Responses 路径保留 schema、strict 等格式字段；当前 Messages 路径只接受 text，结构化输出返回 `unsupported_response_format`，不静默变成提示词约束。上游仍可拒绝不支持的选项。
- `web_search`（v1.1）：缺省 false；true 时注入 hosted WebSearch 工具要求，模型证据不支持即 400。

未知顶层字段明确返回 400。v1 不支持工具调用、embeddings、Responses 外部端点或 Agent 操作；图片 / 视频输入与 hosted 搜索按 ADR-064（v1.1）以 content part 数组、`web_search` 请求位开放，音频输入与其余未支持字段仍显式拒绝。

普通响应遵循 `chat.completion` 的 `choices[].message` / `finish_reason` / `usage`。SSE 为 `data: <chat.completion.chunk JSON>`，文本在 `choices[].delta.content`；包含终止 chunk，按需发送 `choices:[]` 的 usage chunk，最后 `data: [DONE]`。请求已开始流式返回后的上游失败以 `data: {"error":...}` 结束，客户端必须检查 error，不能仅凭 HTTP 200 判断完成。

usage 使用 prompt_tokens / completion_tokens / total_tokens，缓存读取量在 prompt_tokens_details.cached_tokens。错误体统一 `{"error":{"message":...,"type":...,"code":...,"param":null}}`；只发送固定脱敏文案，不回显上游错误正文、请求内容或密钥。

| 状态码 | 含义 |
| --- | --- |
| 400 / 404 | 不支持的参数 / 未知或禁用模型、未知路由 |
| 401 / 403 | 网关 token 无效或撤销 / Host、Origin 拒绝 |
| 405 / 413 / 415 | 方法错误 / 请求体超限 / 非 JSON |
| 429 | 凭证租约并发限制或上游配额/限流 |
| 502 / 503 / 504 | 上游失败或认证失效 / 本机服务依赖不可用 / 超时 |

请求体上限 2 MiB；输出正文上限 16 MiB；最多 64 个 HTTP 连接；请求头/正文各 15 秒，模型准备 30 秒，补全最多 600 秒。流式队列有界，慢读有背压；客户端断开或服务停止会取消模型请求。

## 凭证与用量

网关 token 是独立的随机凭证，不等于 `gui.token` 或厂商 key。随机 ID + 256 位随机秘密；只落 BLAKE3 摘要和非秘密的客户端/ID/签发时间。Unix token 目录 0700、文件创建时即 0600；摘要常数时间比较。`list` 不返回秘密，`revoke` 以 token ID 操作。

每请求分配随机唯一 request ID，使用既有 CredentialPool 租约，按 `thirdparty/<client>` 租户与实际 provider/credential 归属写 UsageLedger。取消或失败时仅记已收到的 usage，不估造缺失用量；上游完全未回传 usage（如图像生成模型经 chat 兼容端点）时零记录如实跳过账本，不伪造 token，也不让已成功响应因记账校验回 500（ADR-064）。第三方记录独立于 `local/default`，当前不并入 GUI 的个人 quota 投影；同一厂商账号的真实远端额度仍共同消耗。池的并发限制沿用进程内、租户/账号作用域，不宣称跨进程全局限流。未提供价格时费用记录为 unknown/unpriced。

## 任务日志与统计（2026-10-04）

持久调用日志及查询复用既有供应商执行、鉴权和账本连接。完整作品是一级 `task`，小说章节 / 视频分段是可选二级 `subtask`，分部是正交的可选 `group`；消耗用途单独用 `operation`。客户端不需要把小说和视频改成同一种页面。相同作品跨客户端的关联只由明确提交的 task ID 决定，不按时间 / 名称猜测。

### 生成时提交关联

`POST /v1/chat/completions` 可追加以下字段，旧客户端省略时仍记录为未关联任务。文字 / 分镜操作要求 text 能力，图片操作要求 image_output 能力；省略 operation 上下文时由已解析模型能力判断 text 或 image。

```json
{
  "model": "glm-coding/glm-5.2",
  "messages": [{"role": "user", "content": "续写本章"}],
  "pawork_usage": {
    "task": {"id": "novel:42", "title": "纸船的午后"},
    "group": {"id": "part:1", "title": "第一部"},
    "subtask": {"id": "chapter:3", "title": "第三章"},
    "operation": "text",
    "retry_of": null
  }
}
```

视频 `POST /v1/video/tasks` 追加相同上下文，把 subtask 换成分段、operation 设为 `video`；仍仅接受已有 model / prompt 生成参数。group / subtask / retry_of 可省略或为 null。ID 为稳定非空字符串，最多 256 字节；标题最多 512 字节，是调用时的快照，后续改名仍按 ID 关联；均拒绝控制字符。不能把 bearer token、提示词或正文放入这些字段。`retry_of` 是同一 client、同一作品的原 call ID，明确补交生成新记录，保留原记录。

普通响应及每个 SSE chunk 追加 `"pawork_usage":{"call_id":"gateway-…"}`；视频响应另有 `related_call_id`（提交时 null，查询时为原生成 call ID）。call ID 与 OpenAI completion ID、账号限定的视频任务 id 各司其职，客户端应分别保存。已经进入执行阶段的错误尽可能返回 `error.call_id`；准备阶段拒绝没有已执行记录。

### 查询记录与全量汇总

`POST /v1/usage/query` 接受 JSON `TaskUsageQuery`，复用 Gateway bearer 鉴权：

```json
{
  "task_id": "novel:42",
  "group_id": "part:1",
  "group_by": "subtask",
  "limit": 100
}
```

| 字段 | 语义 |
| --- | --- |
| client | 可省略；Host 强制使用 token 所属 client，传其它 client 返回 403 |
| task_id / group_id / subtask_id | 完整作品 / 分部 / 章节或分段精确筛选，可组合 |
| operation | text / storyboard / image / video / coding / query / download / import / export / edit |
| provider / model / status | 精确筛选；status 为 running / submitted / succeeded / failed / cancelled / unknown |
| started_after_ms / started_before_ms | Unix 毫秒半开区间 `[start,end)`；不是归档时间或媒体时长 |
| group_by | task（缺省）/ group / subtask / operation / model / client |
| limit / cursor | limit 缺省 100，1–200；cursor 原样回传 `next_cursor`，不解析或自行生成 |

响应为 `TaskUsageReport { totals, groups, records, next_cursor }`。totals 与 groups 总计全部匹配记录，limit / cursor 只分页 records；不要把每页 totals 再相加。groups 每项携带 key / title / totals / filter，filter 提供保留已有筛选的准确下钻条件；未关联维度的 filter 为 null。章节 / 分部 key 包含父作品，客户端不得解析 key。records 按 `(started_at_ms,client,id)` 倒序，末页 next_cursor 为 null。数据仍可能在轮询后更新，刷新重新从首页查询。

记录包含 call ID、认证 client、来源（gateway / native_run / client_report）、任务上下文、操作、供应商 / 模型、开始 / 完成毫秒、状态、可空 Token / cost / output_images / planned_video_seconds、上游任务 ID / 状态、related_call_id 和脱敏 error_code。没有提示词、正文、媒体 URL 或供应商 Secret。

totals 分别给出 records、generation_calls、failed / cancelled / unfinished、已知 tokens（输入 / 输出 / 缓存读写）、unknown_token_calls / unknown_cost_calls、output_images、planned_video_seconds、currencies。每币种区分 actual_micros / estimated_micros 与各自 records 数；没有对应记录时不展示虚构的实际 0，不跨币种合计。未知条数只覆盖生成和原生 Run，查询 / 本地操作不伪造未知生成费用。缓存读写保留 canonical 定义，不再加到输入 / 输出 Token 上。

### 上报关联的本地操作

`POST /v1/usage/operations` 只记录下载 / 导入 / 导出 / 编辑，不能补报生成 Token 或费用：

```json
{
  "report_id": "download:stable-operation-id",
  "context": {
    "task": {"id": "film:42", "title": "纸船的午后"},
    "subtask": {"id": "segment:3", "title": "第三段"},
    "operation": "download"
  },
  "status": "succeeded",
  "started_at_ms": 1791000000000,
  "finished_at_ms": 1791000001200,
  "related_call_id": "gateway-original-generation-id"
}
```

report_id 最多 128 字节，Host 生成 `client-<report_id>`，同 client 的相同报告重放幂等，内容冲突 409；终态只接受 succeeded / failed / cancelled，结束时间不得早于开始时间。关联原 call 必须存在于该 client。响应为 TaskUsageRecord，明确 source=client_report；多余字段（包括 Token、金额）400。这些记录不增加生成次数 / 图像 / 视频用量。

### 记录与显示边界

鉴权、请求与模型 / 凭证准备成功后，执行尝试在租约 / 上游前持久化；正常、失败、取消都有日志，尚未执行的准备拒绝不冒充已执行调用。费用账本仍只追加非零用量，日志能记录无 usage 的成功图片和视频。Token 只有明确 UsageUpdated 或非零 summary 才确认，缺失为 null；标准 OpenAI usage 里的兼容零值不能作为账单证据，应以日志的可空 tokens 为准。

Gateway 当前没有供应商账单来源，cost 为 null；原生 Pawork Run 只在既有 pricing 可用时提供 estimated。视频秒数是固定参数的计划值（当前每次提交 5 秒），不能当实测媒体时长、credits 或实际扣费。提交超时 / 取消 / 回执丢失可能已被上游接受，记 unknown，明确补交单独保留。查询作为 query 关联原生成并更新确认终态，保持提交耗时，不重复生成统计；已确认终态不会被迟到并发轮询倒退。

新视频 handle 的 client owner 持久化，跨 client 查询返回 404；升级前未有日志的旧 handle 沿用原查询能力，查询记录未关联，不回填猜测 owner。历史缺失调用和账单不能从成片 / 字数反推。running 也表示结果尚未确认，进程异常退出不能伪造成功或零扣费。

相同 data_dir / instance 的 Gateway 与 GUI Host 读取同一 `usage-ledger.sqlite3`（SQLite v4 纯新增表，保留 v2/v3 历史费用与去重）；GUI API 1.26 的 task_usage 可汇总该实例全部 client。Desktop 入口与显示见 [任务消耗](task-usage-ui.md#7-生产界面)；消费者应按各自 token 查询，在 MoMai 用作品 / 分部 / 章节，在 YingMai 用作品 / 分段 / 操作类型。

## 验证边界

定向回归覆盖本地 HTTP 与模拟上游的目录、鉴权 / 撤销、普通 / SSE 补全、模型路由、参数和请求边界、断开取消、用量落账、视频账号 / client 归属及跨 Host 重启查询。token 回归覆盖秘密不落盘、文件权限与路径拒绝；实例锁回归覆盖 GUI 共存、网关独占及 Agent 会话不受启动清扫影响。任务统计回归覆盖全量汇总、游标分页、客户端隔离和本地报告幂等。

上述回归不证明真实媒体供应商、套餐权限、账单、消费者 GUI、Windows 系统行为或网关系统服务安装；未完成验收见 [ROADMAP](../ROADMAP.md)。验证入口与证据要求见 [验证规格](verification.md)。
