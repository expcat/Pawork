# S6：首发 Provider 与认证

> 阶段 S6 · 首发 Provider 与认证 · 状态：🔵进行中 · 依赖：S2（Anthropic 最小版在位）· 规模：大 ·（与 S5/S7 可并行）

## 目标（本阶段结束时用户能做什么）

把 S0–S5 的两条开发测试通道扩展为六条首发产品通道：ChatGPT OAuth、xAI Grok OAuth、Z.AI GLM Coding Plan API key、OpenCode Go API key、Qwen Token Plan API key、DeepSeek API key。`pawork models` 聚合已配置通道；API key 与 OAuth 凭证进入 OS Keychain；环境变量只作 headless/CI fallback；全局脱敏确保 secret 不入日志。

Google/Gemini、Moonshot/Kimi、OpenAI API key、xAI API key、Qwen 按量计费、智谱中国区标准计费端点，以及 Anthropic 的 S6 完整化均延期，只有后续需求明确纳入时再做。S0 的 generic OpenAI-compatible 与 S2 的 Anthropic 基线继续保留，不算本阶段新增厂商。

## 首发范围冻结

| 通道 | 凭证 kind | Wire transport | 默认 Base URL / 约束 |
| --- | --- | --- | --- |
| ChatGPT | `OAuthBearer` | Responses | 当前后端预设 `https://chatgpt.com/backend-api/codex`，可覆盖；必须提供 account id；登录/刷新由 `pawork-auth` 注入，不在 adapter 内硬编码 OAuth client secret |
| xAI Grok | `OAuthBearer` | 模型 capability 声明 Responses 或 Chat Completions | `https://api.x.ai/v1`，未知模型保守走 Chat；本期不接受 xAI API key |
| Z.AI GLM Coding Plan | `ApiKey` | 默认 Chat Completions，可逐模型声明 Responses | `https://api.z.ai/api/coding/paas/v4`；`provider_id` 沿用 `glm-coding` 兼容既有配置 |
| OpenCode Go | `ApiKey` | 默认 Chat Completions，可逐模型声明 Responses | `https://opencode.ai/zen/go/v1` |
| Qwen Token Plan | `ApiKey` | 默认 Chat Completions，可逐模型声明 Responses | `https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1` |
| DeepSeek | `ApiKey` | 默认 Chat Completions，可逐模型声明 Responses | `https://api.deepseek.com` |

所有 transport 选择都来自 adapter 配置或 `ModelCapabilities`，Agent Engine 不按 Provider 名称分支。未声明的 hosted tools/extensions fail-closed；本波只承诺文本、图片输入、客户端 function tools、SSE、usage/stop 与 reasoning continuation 基线。

## 涉及包与 V1 资产

| V2 包（目录） | 本阶段动作 | 来源与方式 |
| --- | --- | --- |
| `pawork-providers` | 增强：六条首发 adapter；ChatGPT/xAI 共用 Responses transport；四条 API-key 通道复用 OpenAI-compatible Chat transport并可逐模型切 Responses；凭证 kind 构造期 fail-closed；首发范围错误码归一表 | 复用 V1 Responses/adapter 语义并按 V2 canonical 重组；不迁移延期厂商 |
| `pawork-auth`（providers/auth） | 激活：OS Keychain、OAuth PKCE/Device/refresh/callback、credential、masked；凭证解析链为 Keychain → env fallback → 无凭证 | 波 B 迁移 V1 `auth-service`，并只接六条首发通道 |
| `pawork-diagnostics`（foundation/diagnostics） | 激活：全局脱敏 tracing layer；与 `ResolvedCredential`、`pawork-auth::masked` 对齐 | 波 B 迁移并修复 V1 未全局挂载缺口 |
| `pawork-config` | 凭证引用接 `pawork-auth`；六通道配置与模型目录联动 | 波 C 接线 |
| `pawork-cli` / `pawork-app` | `pawork auth`、`/model`、`/provider`，以及六通道装配 | 波 C 接线 |

## 波次状态

- [x] **波 A — adapter**：六通道 adapter、共享 Responses、错误归一、wiremock 契约已实现；未使用真实凭证。
- [x] **波 B — auth/diagnostics**：OAuth 获取与刷新、Keychain、masked、全局 tracing 脱敏。
- [ ] **波 C — config/cli/app/smoke**：正式装配、模型聚合、切换、日志扫描与真实冒烟。

## 关键任务

1. **首发 adapter 契约**：每个渠道只接受冻结的 credential kind；默认 endpoint 可覆盖；Chat/Responses 由模型数据选择；共享 Responses 组装器覆盖文本、工具、reasoning、usage 与结束原因。
2. **auth 链路**：API key 的 `set-key` 与 ChatGPT/xAI OAuth 登录/刷新都落 Keychain；env fallback 只在 Keychain 无条目时启用，并在 `auth list` 标注来源。
3. **脱敏三线对齐**：`ResolvedCredential` Debug、`auth::masked`、diagnostics layer 一致；日志全链扫描断言。
4. **切换体验**：会话中途 `/provider`、`/model` 切换后续轮走新模型，事件流记录变更。

## 真实测试与评估（阶段冒烟清单）

- [ ] ChatGPT 与 xAI：浏览器登录/回调或 device flow → Keychain → token refresh → `pawork chat`；无 OAuth 凭证时 fail-closed。
- [ ] 四条 API-key 通道：`pawork auth set-key <provider>` 后清除对应 env，分别完成一次流式工具任务。
- [ ] `pawork auth list` 只显示掩码与来源，不显示 token/key/account secret。
- [ ] `pawork models` 聚合六条首发通道；混合协议模型按 registry transport 路由。
- [ ] REPL `/provider` + `/model` 切换后续聊正常，`sessions show` 可见模型切换记录。
- [ ] 最详细日志级别完成任务后，日志与终端输出扫描不到任何凭证片段。

## 定向自动化测试

- `cargo test -p pawork-providers`。
- `cargo test -p pawork-providers --all-features`：六通道 credential/path/header/body、Chat/Responses 路由、Responses SSE/reasoning、错误码归一。
- `cargo test -p pawork-auth`：Keychain、OAuth PKCE/Device/refresh/callback、masked。
- `cargo test -p pawork-diagnostics`：全局脱敏规则与 token tracing 断言。
- config/app 的凭证优先级与六通道装配定向测试。

## 退出标准

- [ ] 六条首发通道完成正式装配与真实冒烟；延期厂商没有伪 feature、空 adapter 或预埋分支。
- [ ] Keychain 为主、env 为显式 fallback；OAuth refresh 与 credential kind fail-closed 回归通过。
- [ ] 共享 Responses 与四条 API-key 通道契约通过，transport 选择不进入 Engine。
- [ ] diagnostics layer 全局挂载，Secret 不入日志回归通过。

## 为后续阶段预留 / 明确不做

- 预留：Provider 账号池/租约/路由属于 S11；远端配额监控继续冻结候审。
- 延期：除首发六通道外的厂商/认证方式；provider-hosted tools/extensions；没有公开稳定契约或真实账号的 OAuth 冒烟必须登记，不能用 mock 冒充完成。

## 并行拆分建议

- 波 A：adapter（已完成自动化基线）。
- 波 B（并行 ×2）：`pawork-auth`；`pawork-diagnostics`。
- 波 C（串行）：config/cli/app 接线 + 模型聚合 + 全链日志扫描 + 真实冒烟。

## 参考

- [Z.AI GLM Coding Plan](https://docs.z.ai/devpack/quick-start) · [OpenCode Go](https://dev.opencode.ai/docs/go/) · [Qwen Token Plan](https://help.aliyun.com/zh/model-studio/token-plan-personal-quick-start) · [DeepSeek API](https://api-docs.deepseek.com/)
- [OpenAI Codex authentication](https://learn.chatgpt.com/docs/auth) · [xAI REST API](https://docs.x.ai/developers/rest-api-reference/inference)
- [../docs/design.md](../docs/design.md) §4 · [../docs/task-guide.md](../docs/task-guide.md) §5
