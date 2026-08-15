# S6：首发 Provider 与认证

> 阶段 S6 · 首发 Provider 与认证 · 状态：🔵进行中 · 依赖：S2（Anthropic 最小版在位）· 规模：大 ·（与 S5/S8 可并行；S7 GUI 建议本阶段先行但设计波可不阻塞）

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
- [x] **波 C — config/cli/app/smoke**（2026-08-15）：六通道正式装配（channels 表 + 目录兜底装配）；`pawork models` 跨通道聚合；`pawork auth list/set-key/login/logout`；REPL `/model` `/provider` + `model.switched` 事件；宿主全局挂载 RedactingFmtLayer（stderr）。真实冒烟：glm-coding / opencode-go 完成 set-key → 清 env → Keychain 流式工具任务；GLM 双协议通道完成 `/model` `/provider` 切换 + `sessions show` 记录；trace 级日志 + 终端扫描 0 泄漏（24 文件 233 行）。登记项：ChatGPT/xAI OAuth 浏览器登录与 Qwen/DeepSeek 凭证未冒烟（fail-closed 已验）；macOS 未签名 dev 构建重编后 Keychain 条目 ACL 不匹配会弹授权框（详见冒烟登记）。

## 关键任务

1. **首发 adapter 契约**：每个渠道只接受冻结的 credential kind；默认 endpoint 可覆盖；Chat/Responses 由模型数据选择；共享 Responses 组装器覆盖文本、工具、reasoning、usage 与结束原因。
2. **auth 链路**：API key 的 `set-key` 与 ChatGPT/xAI OAuth 登录/刷新都落 Keychain；env fallback 只在 Keychain 无条目时启用，并在 `auth list` 标注来源。
3. **脱敏三线对齐**：`ResolvedCredential` Debug、`auth::masked`、diagnostics layer 一致；日志全链扫描断言。
4. **切换体验**：会话中途 `/provider`、`/model` 切换后续轮走新模型，事件流记录变更。

## 真实测试与评估（阶段冒烟清单）

- [x] ChatGPT：浏览器 OAuth 登录/回调 → auth 文件（`~/.pawork/auth.json`，0600，文件后端替代 Keychain）→ `pawork chat` 直通。（2026-08-15 完成：回调参数对齐上游（`/oauth/authorize`、`/auth/callback`、scope 含 connectors、`codex_cli_simplified_flow`）；根因为 `/models` 按 `minimal_client_version` 过滤——client_version 提至 0.147.0 后目录含 gpt-5.4/5.5/5.6-sol/terra/luna，隐藏模型（codex-auto-review）按 visibility 过滤；`gpt-5.4` 与 `gpt-5.6-luna` 各完成一次真实流式对话；token refresh 冒烟仍待后续）
- [x] xAI：device flow → auth 文件 → `pawork chat`；无 OAuth 凭证时 fail-closed。（2026-08-15 完成：参照上游 grok CLI / cc-switch 接入 auth.x.ai 公开 Device Flow 预设（`/oauth2/device/code` + `/oauth2/token` + grok-cli 公共 client + 官方 Agentic CLI scope 组），`pawork auth login xai` 走设备码引导，`[oauth.xai]` 可覆盖且优先；wiremock 契约覆盖 begin→pending→success 轮询→auth 文件→XaiProvider 构造链路与 grok-4 Responses 流式事件；fail-closed 已验。真实凭证冒烟：设备码浏览器授权 → auth 文件（`auth list` file 来源 + 掩码）→ `grok-4` Responses 流式对话（thinking + 用量 1256/141）与 `grok-3` Chat Completions 流式对话（cache read 192）双传输各通过一次，输出无 token 片段。附带修复：零配置下 `auth login` 不再因缺 default_model 拒绝启动（目录/凭证命令退化为 CatalogOnly 装配，chat/run 保持 fail-closed，回归测试锁定））
- [x] 四条 API-key 通道：`pawork auth set-key <provider>` 后清除对应 env，分别完成一次流式工具任务。（2026-08-15 完成：glm-coding / opencode-go 此前已通过；qwen-token-plan / deepseek 从 `Pawork_v2/.env` 注入后 `set-key` → 清 env → `auth list` file 来源；`qwen3.8-max` 完成 `read_file` 工具任务（cache read 2048）与纯文本流式对话；`deepseek-chat` 完成 `read_file` 工具任务（cache read 2560）与纯文本流式对话。Token Plan 线上目录已换代，静态条目由过期的 `qwen3-coder-plus` 更新为 `qwen3.8-max`）
- [x] `pawork auth list` 只显示掩码与来源，不显示 token/key/account secret。（keychain 命中显示掩码，env/none 不显示值）
- [x] `pawork models` 聚合六条首发通道；混合协议模型按 registry transport 路由。（chatgpt 登录后运行期探测 6 模型；glm-coding 运行期探测合并 9 个模型）
- [x] REPL `/provider` + `/model` 切换后续聊正常，`sessions show` 可见模型切换记录。（glm-anthropic↔glm-openai 双协议通道实测；模型全局归属校验拒绝跨 provider 复用 id 符合设计）
- [x] 最详细日志级别完成任务后，日志与终端输出扫描不到任何凭证片段。（RUST_LOG=trace；24 个输出/日志文件 233 行对两把真实 key 全值 grep，0 命中）

### 冒烟登记（不伪完成）

- macOS 未签名 dev 构建每次重编后二进制 cdhash 变化，读取此前创建的 Keychain 条目会触发 SecurityServer GUI 授权框并在 headless 下阻塞；签名发布构建不受影响。冒烟中创建的 `pawork.glm-coding` / `pawork.opencode-go` 两个 default 条目已于 2026-08-15 收口波用 `security delete-generic-password` 删除并验证：`auth list` 恢复 env 来源、无 env 时 fail-closed，且经 env 直通完成一次真实流式对话。
- 修复：wave B 的 keyring 依赖未启用平台后端（默认 mock 存储，store 后 get 即 NotFound）；本波按 macOS/Linux/Windows target 分别启用 `apple-native` / `sync-secret-service` / `windows-native`，进程内与跨进程 Keychain 读写删已实测。
- 用户决策（2026-08-15）：不使用 macOS Keychain 及任何系统机密存储，凭证统一走文件后端 `$PAWORK_HOME/auth.json` / `~/.pawork/auth.json`（JSON v1、0600、临时文件+rename 原子写、损坏 fail-closed，形态对齐 Codex CLI auth.json）；keyring 依赖已整体移除，旧 Keychain 条目已清理（验证 0 残留）。
- 修复（2026-08-15）：ChatGPT `/models` 空列表与 `/responses` 400 同根因——后端按 client_version 过滤目录且退役模型不可调用；client_version 对齐上游 0.147.0、`visibility != list` 的模型不进目录后，`gpt-5.4` 与 `gpt-5.6-luna` 均真实直通。

## 定向自动化测试

- `cargo test -p pawork-providers`。
- `cargo test -p pawork-providers --all-features`：六通道 credential/path/header/body、Chat/Responses 路由、Responses SSE/reasoning、错误码归一。
- `cargo test -p pawork-auth`：Keychain、OAuth PKCE/Device/refresh/callback、masked。
- `cargo test -p pawork-diagnostics`：全局脱敏规则与 token tracing 断言。
- config/app 的凭证优先级与六通道装配定向测试。

## 退出标准

- [x] 六条首发通道完成正式装配与真实冒烟；延期厂商没有伪 feature、空 adapter 或预埋分支。（2026-08-15 完成：ChatGPT / xAI / glm-coding / opencode-go / qwen-token-plan / deepseek 均完成真实凭证冒烟；ChatGPT token refresh 仍待 token 临期自然触发）
- [x] auth 文件为主、env 为显式 fallback；OAuth refresh 与 credential kind fail-closed 回归通过。（auth 50+ 测试 + app 目录兜底/优先级测试；文件后端按用户决策替代 Keychain）
- [x] 共享 Responses 与四条 API-key 通道契约通过，transport 选择不进入 Engine。（`cargo test -p pawork-providers --all-features` 全绿；Engine 仅消费 trait）
- [x] diagnostics layer 全局挂载，Secret 不入日志回归通过。（宿主 EnvFilter + RedactingFmtLayer；trace 级冒烟 0 泄漏）

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
