# pawork-providers

模型通道适配器：net + registry + 六通道。依赖 `pawork-domain`。

## 职责

把 canonical `ModelProvider` 接到具体 HTTP/SSE：内置模型目录、定价、用量归一、能力协商、reasoning 保护。六条首发通道登记在 `channels/registry.rs` 的 `CHANNEL_REGISTRY`（行不带 cfg；`is_enabled` 是唯一 feature 求值点）。Engine **不得**按 Provider 名分支；本包也不得在 core 模块依赖 `net`（模块纪律测试护航）。

## 模块树

```
src/
  lib.rs
  registry.rs  pricing.rs  usage.rs  negotiate.rs  reasoning.rs
  provider.rs  request.rs  stream.rs  error.rs  error_table.rs
  responses.rs  responses_reasoning.rs  memory_protector.rs
  net/{http,sse,retry}.rs
  channels/
    registry.rs          # CHANNEL_REGISTRY
    anthropic/           # feature anthropic
    chatgpt.rs           # chatgpt-oauth
    xai.rs               # xai-oauth
    api_key.rs           # glm-coding | opencode-go | qwen-token-plan | deepseek
```

## 对外入口/API 面

`pub mod` 覆盖 net / registry / channels 等。crate 根再导出通道登记与若干适配器。

**`CHANNEL_REGISTRY` 六行（产品序，id 即 `provider_id`）：**

| id | kind | feature | 默认 base_url 形态 |
| --- | --- | --- | --- |
| `chatgpt` | ChatGptOAuth | `chatgpt-oauth` | ChatGPT backend Codex |
| `xai` | XaiOAuth | `xai-oauth` | xAI v1 |
| `glm-coding` | ApiKey | `glm-coding` | Z.AI coding paas v4 |
| `opencode-go` | ApiKey | `opencode-go` | OpenCode Go |
| `qwen-token-plan` | ApiKey | `qwen-token-plan` | 阿里云 token-plan compatible |
| `deepseek` | ApiKey | `deepseek` | DeepSeek |

登记辅助：`channel_preset` / `is_enabled` / `ChannelKind` / `ChannelPreset` / `OAuthFlow` / `OAuthPreset`。

其它要点：`ModelRegistry`、`CapabilityNegotiator`、`ReasoningProtector` / `InMemoryReasoningProtector`、`OpenAiCompatibleProvider`、feature 门控的 `AnthropicProvider` / `ChatGptProvider` / `XaiProvider` / `ApiKeyChannelProvider`。OAuth client 参数在 preset 里，**不**在 adapter 内硬编码第二份。

默认 feature 仅 `anthropic`；宿主 `pawork-app` 打开全部六通道 feature。

## 依赖与被依赖

- **依赖**：`pawork-domain`。`reqwest` / `futures` / `bytes`。不依赖 auth/storage（凭证由宿主注入；PWB1 实现在 app）。
- **被依赖**：`pawork-app`（生产，开全通道）；`pawork-engine` **仅 dev**（`CHANNEL_REGISTRY` 供 `no_provider_branch` 守护）。

## 红线与注意事项

- 新增通道 = `CHANNEL_REGISTRY` 加一行 + feature；禁止在 app/engine 再写 id 表。
- `ApiKeyChannelConfig` 不得按渠道名猜测；未启用 feature → fail-closed。
- `ReasoningProtector` 不解释明文、不按 Provider 名分支；持久化实现在 `pawork-app`。
- HTTP 错误路径不得把 request body 片段拷进 `ProviderError.message`（可能回显 token）。
- `registry`/`pricing`/`usage`/`negotiate`/`reasoning`/`error` 不得引用 `net`。
- Anthropic 未声明能力在写 wire 或 HTTP 前显式拒绝（K-10），不静默丢弃。

## 相关文档

- [docs/design.md](../../docs/design.md) §2 / §4 S6
- [plan/R5-provider-neutrality.md](../../plan/R5-provider-neutrality.md)
- [AGENTS.md](../../AGENTS.md) §2（Engine 无 Provider 名称特例）
- [代码地图总索引](../../docs/code-map/README.md)
