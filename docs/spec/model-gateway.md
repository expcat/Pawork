# 本机模型网关

- 最近更新：2026-10-03
- 范围：第三方本机程序复用 Pawork 已连接模型的 HTTP API；MoMai 是消费者之一，接口不包含 MoMai 专属字段。
- 架构：[architecture](../architecture.md)；实现：[gateway](crates/gateway.md)、[app](crates/app.md)、[cli](crates/cli.md)；阶段状态：[ROADMAP](../ROADMAP.md)。

## 决策：GW-1 通用本机模型接口

本次用户明确要求 Pawork 提供通用、可复用的外部模型访问接口。采用 OpenAI Chat Completions 兼容子集，独立于 GUI/core-api/ACP，不变更既有协议版本、Agent 事件或数据库 schema。仍由唯一 `pawork` 二进制托管。2026-09-27 按用户授权的包边界调整，将 HTTP/SSE 与客户端 token 提取到 `pawork-gateway`；AppCore 实现 `GatewayBackend`，继续负责模型路由、账户快照、租约与用量，CLI 负责进程装配。HTTP 沿用 hyper / hyper-util / http-body-util，无新增第三方依赖。

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

providers 从已配置的 `/compatible-mode/v1` 基地址推导同源原生根，Chat 与视频共用渠道基地址 / 代理装配；在 SecretBackend 同一事务中冻结凭证与账号身份。提交与查询均取得绑定账号的并发租约，并在完成、失败、取消或超时后释放；后台执行任务由网关退出流程等待收尾。domain 的 `VideoGenerationModel` / `VideoGenerationTask` / `VideoTaskStatus` 是共享类型，宿主必须明确实现三个视频端口，无无类型 JSON 或默认兼容实现。原生视频按条 / 秒计费，未写成 token 账本，不估造费用。本轮模拟回归与真实 Host + mock 验收仅证明本机协议、媒体下载与资源收尾，不能替代真实供应商套餐权限或消费者 GUI 验收。

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

## 验证边界

定向回归通过真实本地 HTTP 和模拟上游检查目录、鉴权、token 撤销、非流式/SSE/usage、请求模型路由、结构化参数保留、Host/Origin 拒绝，以及流式超时错误、断开后取消和部分用量落账。token 测试检查无秘密落盘、文件权限和路径拒绝。实例锁回归检查与 GUI 共存、网关独占和不清扫 Agent 会话。MoMai 实际客户端与实际网关跨进程验证目录与撤销错误，普通/SSE 补全连接本地模拟上游；CLI 另验签发/list/revoke/serve/status/shutdown。真实供应商、Windows 系统行为、系统服务安装不由这些回归证明，状态见 [ROADMAP](../ROADMAP.md)。

### 2026-10-03 验证记录

受影响包运行 `bash scripts/test.sh --log /tmp/pawork-gateway-root-20261003-tests.log domain providers gateway app`。初次在旧 `api_key_channels` 生图 mock 把 JSON 契约当 SSE 时失败，已同步 mock 并补跑该目标及其后尚未运行的目标：

```bash
cargo test -p pawork-providers --offline \
  --test api_key_channels --test chatgpt --test contract --test responses --test xai \
  --features anthropic,chatgpt-oauth,xai-oauth,glm-coding,opencode-go,qwen-token-plan,deepseek,kimi-platform,kimi-code
cargo test -p pawork-app --offline --lib --features ui-fixture gateway_
bash scripts/test.sh --log /tmp/pawork-gateway-root-20261003-host.log --host
bash scripts/mock/gate.sh --level 0,2
```

所选目标共 522 个不同测试通过；最后 App 网关专项 4 项通过，包含视频等待上游时断连与 Host 退出后的租约释放。当前 `pawork` 正式构建及真实 Host 子进程 3 项通过；mock L0/L2 通过，server smoke 为 70/70。全 workspace 门禁未运行。

代理通过当前 `target/debug/pawork` 和本地 mock 模拟消费者操作，27/27 项通过：CLI 生命周期、三类目录、普通 / SSE 及 fixture 权威用量、生图 JSON 到消费者 SSE 和 PNG 下载解码、视频 PENDING → RUNNING → 重启 Host → SUCCEEDED 与 MP4 下载解码、失败脱敏、无效参数 / 鉴权 / Origin 拒绝、token 撤销。mock 视频由临时 1 秒 MP4 fixture 回放，下载后逐帧解码并核对摘要；固定 720P / 16:9 / 5 秒请求参数由定向回归校验，不据回放文件推定真实生成质量。

验收准备阶段保留了三次失败：首次 CLI 启动超过脚本 15 秒预算，后续预算改为 60 秒；第二次端点放在 Workspace 层而被产品 Global-only 规则剥离，改用现有 `run_instance.py` 的独占备份 / 恢复流程；第三次空 fixture 兜底无 usage，改为现有 `opencode-go/chat_text.sse` 并精确核对 9 输入 / 3 输出 token。最终原 Global 配置字节和权限均已恢复，全部验收进程退出。本机证据为 `/tmp/pawork-gateway-root-20261003-acceptance-final-pass.log` 与 `/tmp/pw-gateway-e3-ycteecza/results.json`；临时脚本和媒体不入仓库。这是代理消费者 HTTP 模拟，真实媒体供应商、消费者 GUI 和用户人工签字未由本轮证明。
