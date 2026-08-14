# S0：最小可对话 CLI

> 阶段 S0 · 最小可对话 · 状态：⚪未开始 · 依赖：无（起点）· 规模：中

## 目标（本阶段结束时用户能做什么）

在 Windows 上 `cargo run -p pawork` 得到一个可用的 CLI：配置好 GLM Coding Plan 或 OpenCode Go 的 base_url 与 key 后，`pawork chat` 进行流式多轮对话，`pawork models` 列出可用模型，Ctrl-C 取消当前回答而不退出进程，错误（401/429/超时/断网）以可读方式呈现。**这是 V2 的第一个真实可测交付物，也是后续一切阶段的宿主。**

## 涉及包与 V1 资产

| V2 包（目录） | 本阶段动作 | V1 来源与方式 |
| --- | --- | --- |
| workspace 根（`Pawork_v2/Cargo.toml`） | 新建：resolver 2、members 按域 glob、`[workspace.package]` 统一元数据、默认 `publish = false` | 无（参考 [archive/M0](archive/M0-skeleton-foundation.md) 前置节） |
| `pawork-domain`（foundation/domain） | 激活：V1 `agent-domain` 本体整包迁移（**不含** events，events 在 S1 并入）。serde 形状不变；类型暂时闲置无害（ToolDescriptor 等为 S2 铺路） | 直接迁移 |
| `pawork-api`（foundation/api） | 激活：`provider` feature。V1 `provider-api` **整包迁移、不裁剪**：`ModelProvider`/`ProviderEventSink`/`CanonicalModelRequest`/`ProviderStreamEvent`/`ModelResponseSummary`/`ResolvedCredential`/`ProviderError` | 直接迁移 |
| `pawork-net`（net/net） | 激活：V1 `provider-runtime` 的 `http`/`retry`/`sse`/`jsonl`/`partial_json`；feature `parsers`（默认，零重依赖）/`http`（reqwest）；proptest 种子随迁 | 直接迁移（[archive/M0](archive/M0-skeleton-foundation.md) pawork-net 节全文适用） |
| `pawork-providers`（providers/adapters） | 激活：V1 `provider-openai-compatible` 迁移（`POST {base}/chat/completions` 固定 `stream:true` + `stream_options.include_usage`、SSE 流解析、`GET {base}/models`、`provider_id` 可配置、`credential=None` 时免认证头）。若其引用 `provider-runtime::stream_assembly`，把该模块最小迁入 `providers/core`（提前激活，仅此模块） | 直接迁移 |
| `pawork-config`（foundation/config） | 激活（最小三层）：TOML schema 与文件位置**照抄 V1**（全局 `config.toml` + workspace `.pawork/config.toml` 向上查找；`PaworkConfig`/`ProviderConfig{id, base_url}`/`ModelConfig`，**无 api_key 字段**）；实现 Builtin<Global<Workspace 三层合并；Profile/Session/Run 层 S8 补齐 | 迁移或按 V1 schema 重写（执行者选，schema 不得偏离） |
| `pawork-engine`（engine/engine） | 激活（最小）：`run_turn(request) -> 事件流`——组装 `CanonicalModelRequest`、调 `provider.stream`、转发 `ProviderStreamEvent`、透传 `CancellationToken`。不落库、不做工具循环 | 新写（语义对齐 V1 `provider_loop` 的单轮子集） |
| `pawork-app`（host/app） | 激活（最小装配）：读配置 → env 解析 key → 构造 `ResolvedCredential` + provider 实例 → 暴露 `AppCore` 门面（`chat_turn`/`list_models`） | 新写（薄） |
| `pawork-cli`（host/cli） | 激活：`chat` REPL（流式渲染，`TextDelta` 与 `ThinkingDelta` 区分展示；Ctrl-C 取消当轮）、`models` 子命令、`--provider`/`--model` 覆盖、`ProviderError.kind` 分类的可读错误输出 | 新写（V1 `cli-renderer` 可参考） |
| `pawork`（apps/pawork） | 激活：composition root，`main` → 组装 → `pawork-cli` 入口 | 新写（薄） |
| `fixtures/config/` | 产出 `config.example.toml`（GLM Coding Plan OpenAI 端点、GLM Anthropic 端点占位、OpenCode Go 三条目） | 新写 |

## 关键任务

1. **workspace 根骨架**：`cargo metadata` 可解析；统一 edition 2021 / rust-version 1.85 / version 0.1.0。
2. **契约迁移（单一 owner 串行）**：`pawork-domain`（除 events）→ `pawork-api`（provider feature）。红线：canonical 纯净（依赖树无 reqwest/rusqlite/keychain/git/GUI/具体厂商）。
3. **net 迁移**：解析器 golden + proptest 种子先行迁移并通过；`parsers` 默认 feature 零重依赖（`cargo tree` 无 reqwest）。
4. **openai-compatible 适配器迁移**：依赖收敛到 `pawork-api` + `pawork-net`；保留 V1 请求组装与流解析测试。
5. **config 最小实现**：三层合并 + env key 解析（`PAWORK_API_KEY_<PROVIDER_ID>`，`-`→`_` 大写）→ `ResolvedCredential::new(ApiKey, …)`；key 在任何 Debug/Display/日志输出中脱敏（沿用 V1 `[REDACTED]` 语义）。
6. **engine 最小 run_turn** + **app 装配** + **cli REPL**：多轮对话历史在内存中维护（`Vec<Message>`），流式渲染，Ctrl-C → `CancellationToken` 取消当轮。
7. **配置样例与 README 片段**：`config.example.toml` + 两通道接入步骤写入本文件冒烟节（作为操作手册）。

## 真实测试与评估（冒烟清单，两把 key 各跑一遍）

配置（以 GLM Coding Plan 为例；OpenCode Go 同理，base_url 换 `https://opencode.ai/zen/go/v1`）：

```toml
# %APPDATA%\dev\pawork\pawork\config.toml（或 workspace .pawork/config.toml）
default_provider = "glm-coding"
default_model = "glm-5.2"

[[providers]]
id = "glm-coding"
base_url = "https://open.bigmodel.cn/api/coding/paas/v4"   # 注意：Coding Plan 专属端点，不是 /api/paas/v4

[[providers]]
id = "opencode-go"
base_url = "https://opencode.ai/zen/go/v1"
```

```powershell
$env:PAWORK_API_KEY_GLM_CODING = "<coding plan key>"
$env:PAWORK_API_KEY_OPENCODE_GO = "<opencode go key>"
```

- [ ] `pawork models`：GLM 通道列出 glm 系模型；OpenCode Go 通道列出 `deepseek-v4-pro` 等目录。
- [ ] `pawork chat` ≥3 轮连续对话：流式逐 token 输出、上下文连贯（第二轮引用第一轮内容）。
- [ ] 中文与英文各一轮；一轮包含代码块输出，渲染不乱。
- [ ] 长回答中 Ctrl-C：当轮终止、提示已取消、进程存活、可继续下一轮。
- [ ] 错误路径：错误 key → 401 可读提示；错误 base_url → 连接错误可读提示；均不 panic、退出码非零（单次模式）。
- [ ] `--provider opencode-go --model deepseek-v4-pro` 覆盖默认值生效。
- [ ] **模型评估记录**：两通道各记录首 token 延迟体感、中文/代码回答质量、流稳定性（为后续阶段选默认测试模型提供依据）。

## 定向自动化测试

- `cargo test -p pawork-net`：SSE/JSONL/partial-JSON golden + proptest 种子（V1 原样迁移）全绿。
- `cargo test -p pawork-providers`：请求组装（含 `stream_options.include_usage`）与流解析单测（V1 随迁）。
- `cargo test -p pawork-config`：三层合并；env key → credential；**key 不出现在配置序列化与 Debug 输出**断言。
- `cargo test -p pawork-engine`：进程内 mock `ModelProvider`（tests 模块）驱动 run_turn 事件转发与取消。
- env 门控真实 API 测试（`--ignored`）：`PAWORK_SMOKE_*` 驱动一次最小流式请求断言收到 `TextDelta` 与 `ResponseCompleted`。

## 退出标准

- [ ] workspace 根 + 10 个激活目录编译通过（`cargo check` 逐包）。
- [ ] 冒烟清单全项通过（两通道），评估记录留档。
- [ ] `provider-api` 契约整包迁移、零裁剪；`pawork-domain`/`pawork-api` 依赖树 canonical 纯净。
- [ ] net 解析 golden + 种子全绿；`parsers` 默认 feature 零重依赖。
- [ ] key 安全红线断言通过（不入配置文件、不入 Debug/日志输出）。
- [ ] `fixtures/config/config.example.toml` 产出并与本文件冒烟节一致。

## 为后续阶段预留 / 明确不做

- 预留：`ProviderStreamEvent` 全部 13 变体的转发路径（未消费的变体原样透传/忽略，不删）；`ThinkingDelta` 渲染为后续 reasoning 展示铺路；config schema 中 `profiles`/`trust_workspaces` 字段解析但暂不消费。
- 不做：会话持久化与 `--resume`（S1）、`--json`（S1）、工具调用（S2）、Anthropic 协议（S2）、审批（S3）、Keychain（S6）。

## 并行拆分建议（可派发子代理）

- 波 A（串行，单一 owner）：workspace 根 → `pawork-domain` → `pawork-api`（契约文件不并行）。
- 波 B（并行 ×2）：`pawork-net`、`pawork-config`。
- 波 C（并行 ×2，依赖波 A/B）：`pawork-providers`（openai-compatible）、`pawork-engine`。
- 波 D（串行收口）：`pawork-app` + `pawork-cli` + `apps/pawork` + fixtures + 真实冒烟（建议主代理执行，涉及真实 key）。

## 参考

- [../docs/design.md](../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../docs/references.md)（参照项目手册）
- [../docs/task-guide.md](../docs/task-guide.md) §5（测试通道与 key 约定）· [../docs/design.md](../docs/design.md) §3.2（冻结契约表）
- [archive/M0-skeleton-foundation.md](archive/M0-skeleton-foundation.md)（workspace 根、pawork-net、pawork-domain 迁移细则）
- [archive/M2-providers.md](archive/M2-providers.md)（provider 域拆分背景）
