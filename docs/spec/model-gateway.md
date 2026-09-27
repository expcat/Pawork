# 本机模型网关

- 最近更新：2026-09-27
- 范围：第三方本机程序复用 Pawork 已连接模型的 HTTP API；MoMai 是消费者之一，接口不包含 MoMai 专属字段。
- 架构：[architecture](../architecture.md)；实现：[gateway](crates/gateway.md)、[app](crates/app.md)、[cli](crates/cli.md)；阶段状态：[ROADMAP](../ROADMAP.md)。

## 决策：GW-1 通用本机模型接口

本次用户明确要求 Pawork 提供通用、可复用的外部模型访问接口。采用 OpenAI Chat Completions 兼容子集，独立于 GUI/core-api/ACP，不变更既有协议版本、Agent 事件或数据库 schema。仍由唯一 `pawork` 二进制托管。2026-09-27 按用户授权的包边界调整，将 HTTP/SSE 与客户端 token 提取到 `pawork-gateway`；AppCore 实现 `GatewayBackend`，继续负责模型路由、账户快照、租约与用量，CLI 负责进程装配。HTTP 沿用 hyper / hyper-util / http-body-util，无新增第三方依赖。

后端调用保持「prepare → run」两阶段：参数、模型与凭证准备失败先返回正常 HTTP 错误，再开始补全或发送 SSE 成功响应头。宿主的 `Completion` 为不透明快照，接入层不读取凭证或重做模型路由。

与 backlog G7/F6 的边界：保留“不内建对外账户池网关”的排除项；此次开放本机模型调用，不开放远程服务、厂商凭证导出、账户池管理或第三方账户选择接口。

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
| `GET /v1/models` | 返回 `{"object":"list","data":[...]}`，仅包含已连接且启用的可选模型。条目含 `id`、`object:"model"`、`owned_by`、`display_name`、`context_window`。未知窗口为 0。 |
| `POST /v1/chat/completions` | 文本多轮上下文、普通响应或 SSE；无服务器会话状态。 |

模型 ID 为 `<provider>/<model>`，只按第一个 `/` 拆分，上游模型 ID 可以继续包含 `/`。不会把同名模型路由到其他供应商，也不回落到默认模型。

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

- `messages`：非空数组，最多 1024 项；每项 `role` 为 system/user/assistant，`content` 为字符串。
- `stream`：缺省 false；`stream_options.include_usage` 仅用于流式请求。
- `max_tokens` 或 `max_completion_tokens`：正整数，二者不能同时出现。
- `temperature`：0–2。
- `response_format`：text / json_object / json_schema。Chat 与 Responses 路径保留 schema、strict 等格式字段；当前 Messages 路径只接受 text，结构化输出返回 `unsupported_response_format`，不静默变成提示词约束。上游仍可拒绝不支持的选项。

未知顶层字段明确返回 400。v1 不支持工具调用、图片、embeddings、Responses 外部端点或 Agent 操作。

普通响应遵循 `chat.completion` 的 `choices[].message` / `finish_reason` / `usage`。SSE 为 `data: <chat.completion.chunk JSON>`，文本在 `choices[].delta.content`；包含终止 chunk，按需发送 `choices:[]` 的 usage chunk，最后 `data: [DONE]`。请求已开始流式返回后的上游失败以 `data: {"error":...}` 结束，客户端必须检查 error，不能仅凭 HTTP 200 判断完成。

usage 使用 prompt_tokens / completion_tokens / total_tokens，缓存读取量在 prompt_tokens_details.cached_tokens。错误体统一 `{"error":{"message":...,"type":...,"code":...,"param":null}}`；只发送固定脱敏文案，不回显上游错误正文、请求内容或密钥。

| 状态码 | 含义 |
| --- | --- |
| 400 / 404 | 不支持的参数 / 未知或禁用模型、未知路由 |
| 401 / 403 | 网关 token 无效或撤销 / Host、Origin 拒绝 |
| 405 / 413 / 415 | 方法错误 / 请求体超限 / 非 JSON |
| 429 | 凭证租约并发限制或上游配额/限流 |
| 502 / 503 / 504 | 上游失败或认证失效 / 本机服务依赖不可用 / 超时 |

请求体上限 2 MiB；输出正文上限 16 MiB；最多 64 个 HTTP 连接；请求头/正文各 15 秒，模型准备 30 秒，补全最多 300 秒。流式队列有界，慢读有背压；客户端断开或服务停止会取消模型请求。

## 凭证与用量

网关 token 是独立的随机凭证，不等于 `gui.token` 或厂商 key。随机 ID + 256 位随机秘密；只落 BLAKE3 摘要和非秘密的客户端/ID/签发时间。Unix token 目录 0700、文件创建时即 0600；摘要常数时间比较。`list` 不返回秘密，`revoke` 以 token ID 操作。

每请求分配随机唯一 request ID，使用既有 CredentialPool 租约，按 `thirdparty/<client>` 租户与实际 provider/credential 归属写 UsageLedger。取消或失败时仅记已收到的 usage，不估造缺失用量。第三方记录独立于 `local/default`，当前不并入 GUI 的个人 quota 投影；同一厂商账号的真实远端额度仍共同消耗。池的并发限制沿用进程内、租户/账号作用域，不宣称跨进程全局限流。未提供价格时费用记录为 unknown/unpriced。

## 验证边界

定向回归通过真实本地 HTTP 和模拟上游检查目录、鉴权、token 撤销、非流式/SSE/usage、请求模型路由、结构化参数保留、Host/Origin 拒绝，以及流式超时错误、断开后取消和部分用量落账。token 测试检查无秘密落盘、文件权限和路径拒绝。实例锁回归检查与 GUI 共存、网关独占和不清扫 Agent 会话。MoMai 实际客户端与实际网关跨进程验证目录与撤销错误，普通/SSE 补全连接本地模拟上游；CLI 另验签发/list/revoke/serve/status/shutdown。真实供应商、Windows 系统行为、系统服务安装不由这些回归证明，状态见 [ROADMAP](../ROADMAP.md)。
