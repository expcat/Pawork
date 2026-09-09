# 模型供应商目录核查（2026-09-08）

结论：八条正式通道的目录端点基本正确，但核查时的实现不能把“拿到模型 ID”可靠地转换成“账号已认证且 Pawork 可运行的模型”。已实测复现旧模型残留、公开目录被用于验证 key，以及非聊天模型进入目录；协议和元数据处理另有源码可证实的缺口。

状态：**本报告保留实施前核查证据；UI-6a 已实施，当前实现与验证见 [路线图](roadmap-ui-2026-09-09.md#8-ui-6--providers供应商目录多账号) 和 [ADR-058](../spec/settings.md#adr-058ui-6a-目录权威与凭证验证2026-09-08)。本报告本身不构成修复后验收。** 实现入口为 [ROADMAP UI-6](roadmap-ui-2026-09-09.md#8-ui-6--providers供应商目录多账号)。以下为实施前只读核查，原始观测不代表修复后的当前行为。

## 1. 逐供应商结果

请求使用本机已配置的凭证与代理，只做目录 GET；数量是当次响应快照，不是产品固定名单。“端点吻合”仅说明与官方资料一致，不代表当前账号已获授权或完成推理验证。

| 通道 | 当前目录请求 | 本次结果 | 判断 |
| --- | --- | --- | --- |
| DeepSeek | `GET https://api.deepseek.com/models`，Bearer，`data[].id` | HTTP 200，远端 3，Pawork CLI 5 | 端点、ID 解析正确；静态旧模型残留 |
| GLM Coding | `GET https://api.z.ai/api/coding/paas/v4/models`，Bearer，`data[].id` | HTTP 200，远端与 CLI 均 10 | 当前 ID 集合一致；通用解析器臆定窗口和工具能力 |
| OpenCode Go | `GET https://opencode.ai/zen/go/v1/models`，Bearer，`data[].id` | HTTP 200，远端与 CLI 均 35；无 key、错误 key 也均 200 / 35 | 端点正确；不能用于 key 验证；缺少逐模型协议信息 |
| Qwen Token Plan | `GET https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/models`，Bearer，`data[].id` | HTTP 200，远端与 CLI 均 12，包含图片、TTS、实时音频模型 | 中国区端点正确；未按 adapter 的可运行能力过滤 |
| Kimi Platform | `GET https://api.moonshot.ai/v1/models`，Bearer，`data[].id` | 本机无该通道凭证，未发请求 | 与官方 kimi-cli 一致；共享通用解析器缺口；真实认证待验证 |
| ChatGPT | `GET https://chatgpt.com/backend-api/codex/models?client_version=0.147.0`，Bearer + account/originator，`models[].slug` | 本机无该通道凭证，未发请求 | 与 Codex 端点吻合；固定版本过旧，错误响应形状会被当成空成功 |
| xAI | `GET https://api.x.ai/v1/language-models`，Bearer，`models[].id` | 当前凭证请求 HTTP 403，CLI 回退静态 | 端点与官方吻合；403 原因未确定；丢弃远端输入模态 |
| Kimi Code | `GET https://api.kimi.com/coding/v1/models`，OAuth Bearer，`data[].id` | 本机无该通道凭证，未发请求 | 与官方 kimi-cli 一致；未消费远端名称、窗口和能力字段 |
| Anthropic Messages（非八条正式通道之一） | 当前不发目录请求，只返回两条静态模型 | 源码核查 | 尚未实现官方 `GET /v1/models` 及分页 |
| 自定义 OpenAI-compatible | `GET {base_url}/models`，Bearer，`data[].id` | 源码核查，未对任意自定义服务实测 | 仅适用于提供该接口的服务；不能普遍视为认证或完整能力发现 |

端点依据：[DeepSeek Models](https://api-docs.deepseek.com/api/list-models)、[Z.ai Coding endpoint](https://docs.z.ai/devpack/tool/others)、[阿里云 Token Plan 接入](https://help.aliyun.com/en/model-studio/more-tools)、[Kimi 官方目录请求](https://github.com/MoonshotAI/kimi-cli/blob/main/src/kimi_cli/auth/platforms.py)、[Codex Models endpoint](https://github.com/openai/codex/blob/main/codex-rs/codex-api/src/endpoint/models.rs)、[xAI Models API](https://docs.x.ai/developers/rest-api-reference/inference/models)。OpenCode Go 和 Anthropic 的协议依据见下文对应发现。

## 2. 已确认的问题

### 2.1 公开目录不能验证 OpenCode Go API key

不带 Authorization、以及携带刻意构造的无效 Bearer，Go 目录都返回 HTTP 200 和相同的 35 个模型。[verify_api_key](../../crates/providers/src/channels/api_key.rs) 仅以 `list_models().await` 是否成功判定凭证，[auth_set_api_key](../../crates/app/src/gui_host/handlers/settings/auth.rs) 随后存储候选 key。因此 Connect / Replace 可以接受无效 key，直到真正请求模型才暴露认证问题。此结论由真实目录请求和写入调用链共同证明；本次没有执行无效 key 的保存。

修复应把目录发现与凭证验证分开，使用供应商确实校验凭证的接口；公开目录只能证明供应商公布了这些 ID，不能证明该账号有权限。若暂无可靠验证方法，应保留“未验证”的真实状态。

### 2.2 远端成功后仍保留静态旧模型

DeepSeek 实际返回 `deepseek-v4-flash`、`deepseek-v4-pro`、`deepseek-v4-flash-vision-exp`。同次 CLI 目录还包含静态 `deepseek-chat`、`deepseek-reasoner`，共 5 项。

[provider_assembly.rs](../../crates/app/src/provider_assembly.rs) 的 `model_catalog` 和 `models_overview` 先放入静态目录，探测成功只追加未知 ID；既不删除远端已无的 ID，也不更新已有条目的远端元数据。`resolve_provider_model` 又在静态命中时直接返回，因此仅修列表展示仍不足以保证选择有效性。

修复应由成功取得的供应商目录决定 ID 集合，再按 ID 补有证据的元数据；失败才用明确标记的静态回退。用户显式配置模型的语义须一并核对。不得按 `chat/reasoner` 黑名单隐藏问题。

### 2.3 目录缺少模态过滤和正确的请求协议

Qwen 远端及 CLI 同时列出 `wan2.7-image`、`wan2.7-image-pro`、`qwen-audio-3.0-tts-plus`、`qwen-audio-3.0-realtime-plus`。[通用解析器](../../crates/providers/src/provider.rs) 把所有 ID 标成 `text=true`、`tool_calls=true`；[GUI ModelList](../../crates/app/src/gui_host/handlers/query.rs) 只过滤供应商和启用状态，因此这些条目也会进入聊天选择目录。图片、音频需要对应接口，不能因出现在 Models 响应中就视为当前 Chat adapter 可用。[Token Plan 官方接入说明](https://www.alibabacloud.com/help/en/model-studio/token-plan-quick-start)

OpenCode Go 的目录响应仅含 ID 等基础字段，官方却为模型指定不同请求路径：Grok 4.6 / GPT 5.6 Luna 使用 `/responses`，GLM 使用 `/chat/completions`，Qwen 3.8 使用 `/messages`。[OpenCode Go endpoint 表](https://opencode.ai/docs/go/)

当前 [ApiKeyChannelProvider](../../crates/providers/src/channels/api_key.rs) 默认所有模型走 Chat Completions，仅手工 `model_transport_overrides` 能改路由，Messages 分支直接拒绝。目录中的模型不能据此全部宣称可运行。应补逐模型、有来源的协议元数据，并让目录过滤与实际 adapter 路由采用同一事实；暂未接通的协议应诚实标为不可运行。

### 2.4 能力信息既有凭空赋值，也有已取得却丢弃

通用解析器对未知模型固定写入上下文 128,000、最大输出 16,384 和工具支持，远端响应本身并未提供这些证据。此次 CLI 中新发现的 Go / GLM / DeepSeek 等模型确实显示这些固定值。未知应保留 unknown，不能用于精确上下文预算或承诺工具能力。

[Kimi Code 解析器](../../crates/providers/src/channels/kimi.rs) 只使用 ID，然后取静态元数据或文本基线；官方 kimi-cli 还读取 `display_name`、`context_length`、`supports_reasoning`、`supports_image_in`、`supports_video_in`。[官方解析与目录更新源码](https://github.com/MoonshotAI/kimi-cli/blob/main/src/kimi_cli/auth/platforms.py)

[xAI 解析器](../../crates/providers/src/channels/xai.rs) 已正确过滤非文本输出，但丢弃 `input_modalities`，未知 ID 的图片能力固定为 false；官方接口本身提供了该输入能力证据。`aliases` 也未消费，可能使别名对应的静态元数据无法匹配；没有枚举别名本身不代表 canonical ID 不可调用。[xAI Models API](https://docs.x.ai/developers/rest-api-reference/inference/models)

### 2.5 ChatGPT 的客户端版本会影响可见目录

[ChatGptConfig](../../crates/providers/src/channels/chatgpt.rs) 固定 `client_version=0.147.0`。当前 Codex 官方目录中 `gpt-6-astra` 为 `visibility=list`，最低客户端版本为 `0.153.0`；Pawork 自身注释也说明远端按该字段过滤。[Codex 官方模型元数据](https://github.com/openai/codex/blob/main/codex-rs/models-manager/models.json)

在账号获权且服务端执行版本过滤时，旧版本会遗漏新模型。当前缺少 ChatGPT 凭证，未声称已对该账号复现。修复需维护与实际兼容能力相符的客户端协议版本，并验证返回目录。

### 2.6 错误响应形状会被标为目录成功

[OpenAiCompatibleProvider](../../crates/providers/src/provider.rs) 对缺失或非数组的 `data` 使用 `unwrap_or_default()`；[ChatGPT 解析器](../../crates/providers/src/channels/chatgpt.rs) 对缺失或非数组的 `models` 同样产生空列表。HTTP 200 的 `{}` 因而被当成 `Ok([])`。[catalog_state](../../crates/app/src/gui_host/handlers/settings/catalog.rs) 对任意 `Ok` 都标为 `Remote`，无法触发真实格式错误的回退；API-key 验证也会随之误判。

这项为源码可构造缺陷，未声称本次上游出现畸形响应。应区分合法空数组与错误响应结构。ChatGPT 的 `supported_reasoning_levels` 也只检查键存在，空数组或 null 会误标 thinking；应按实际内容判断。

Qwen 当次响应含 `has_more/last_id`，通用解析器未处理分页。本次没有保留 `has_more` 值，不能据此断言已漏页；修复时按供应商实际分页契约核对，不给无分页证据的渠道统一加翻页逻辑。

### 2.7 Anthropic 只有静态基线

[AnthropicProvider::list_models](../../crates/providers/src/channels/anthropic/provider.rs) 始终返回 `claude-3-5-sonnet` 和 `claude-3-5-haiku`，完全不发网络请求。官方已经提供 `GET /v1/models`，使用 `x-api-key`、`anthropic-version`，并有 `has_more/last_id/after_id` 分页。[Anthropic List Models](https://platform.claude.com/docs/en/api/models/list)

这是现有 Messages 适配器的远端发现缺口。它不在八条正式通道注册表中，不应把补目录混同于新增正式供应商或账户流程。

## 3. 参照项目中适合采用的做法

| 参照 | 实际做法 | Pawork 应采用的部分 |
| --- | --- | --- |
| [OpenCode provider](https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/provider/provider.ts) | 基础目录来自 Models.dev；逐模型记录 API / SDK、能力、限制，并有少量专用动态发现逻辑 | 把 ID 发现与协议、能力元数据分开；不要假定所有供应商均靠一个 `/models` 完成全部工作 |
| [Pi 模型生成器](https://github.com/earendil-works/pi/blob/main/packages/ai/scripts/generate-models.ts) | 构建时生成目录；OpenCode 各模型按 provider SDK 选择 Responses / Messages / Chat 等协议 | 可用版本固定的元数据补充协议与能力，不把第三方目录当账号权限证明 |
| [Codex models endpoint](https://github.com/openai/codex/blob/main/codex-rs/codex-api/src/endpoint/models.rs) | 专用请求参数，按 ModelsResponse 反序列化 | ChatGPT 保留专用契约；结构错误必须显式失败 |
| [Kimi 官方目录管理](https://github.com/MoonshotAI/kimi-cli/blob/main/src/kimi_cli/auth/platforms.py) | 验证 `data` 数组，读取元数据，更新已有模型并移除远端消失项 | 采用有证据的字段和替换语义；不照搬其静默更换默认模型行为 |

实现顺序建议：先修目录与认证混用、成功后的 ID 替换和可运行过滤，再补协议及能力元数据，最后完善各专用解析器与 Anthropic 远端发现。原有 [Settings §3.4](../spec/settings.md#34-模型目录规则) 的默认失效提示、显式刷新和失败回退要求继续适用。UI-3 已记录的 Go 会话请求头缺口仍是独立的真实 Run 阻塞，目录修复不能代替它。

## 4. 验证与证据边界

- 真实 GET：DeepSeek、GLM Coding、OpenCode Go、Qwen Token Plan 成功；xAI 返回 403；ChatGPT / Kimi Platform / Kimi Code 缺本机凭证。未将请求失败解释为端点错误。
- 现有 CLI：`target/debug/pawork --instance ui3-followup --provider deepseek --json models`，与真实响应逐供应商比较 ID 集合。本轮未重新构建二进制；核心缺陷另有当前源码佐证。
- Go 认证反例：同一官方目录分别无认证和错误 key 请求，均返回 200 / 35；未调用保存凭证操作。
- 本机临时证据：`/tmp/pawork-provider-catalog-audit.json`、`/tmp/pawork-provider-catalog-oauth-audit.json`、`/tmp/pawork-provider-catalog-cli.json`、`/tmp/pawork-provider-catalog-cli.log`、`/tmp/pawork-catalog-auth-probe.json`。这些路径不是仓库交付物，可能随本机临时目录清理而失效；关键结果已记录于本文。
- 官方资料与参照源码为 2026-09-08 查询的当前分支快照；链接目标和远端模型数量会继续变化。

Validated: 真实目录 GET、现有 CLI 目录对比、本地源码与官方参照核查、文档链接和 `git diff --check`。

Targeted regressions: none（本次为核查与文档更正，无生产代码改动；真实请求只验证目录，不宣称推理回归通过）。

Full workspace gate: NOT RUN（当前未设置全量门禁）。
