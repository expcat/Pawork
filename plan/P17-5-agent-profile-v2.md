# P17-5：Agent Profile v2（智能体配置档案 v2）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P8-5、P8-3、P9-6、P4-9、P17-1、P3-6、P15-7、P15-8、P16-4、P16-7、P11-1

**最终目的**：升级 Agent Profile 为 v2，统一描述一个可复用 Agent 的完整配置：prompt（system / instructions）、model、effort / reasoning 强度、tools（含显式 denied 清单）、skills、MCP、permissions、hooks、memory、max turns、background、isolation。让 Agent 可被一键实例化、复用、共享，且所有维度可被 policy 与运行时校验。v1 profile 可平滑迁移。

**涉及范围**：扩展 `agent-domain`（profile v2 类型）、`resource-loader`（加载 / 校验）；复用各被引用子系统

## 细分步骤

1. **Profile v2 schema** —— 目的：在 `agent-domain` 定义 v2 profile 类型，涵盖 prompt / model / effort / tools(denied) / skills / mcp / permissions / hooks / memory / max-turns / background / isolation；提供 v1→v2 迁移路径。
2. **tools 与 denied** —— 目的：声明允许工具清单与显式 denied 清单（deny 优先），与 tool-runtime + `policy-engine` 协作，确保 denied 不可被任何方式绕过。
3. **skills / MCP / permissions / hooks 引用** —— 目的：profile 通过引用（id + version / pin）挂载 skills（[P8-3](P8-3-skills.md)）、MCP（[P9-6](P9-6-mcp-config.md)）、permissions（[P4-9](P4-9-policy-engine.md)）、hooks（[P17-1](P17-1-user-hooks.md)），引用解析失败或越权时降级 / 报错。
4. **memory / max turns / canonical effort** —— 目的：memory（接入 [P16-7](P16-7-long-term-memory.md) 长期记忆）、max turns（接入 [P3-6](P3-6-budget-control.md) 预算控制）。effort / reasoning 改为**canonical 一等字段**：`AgentProfile.effort` 经 `ReasoningConfig` → [P15-8](P15-8-capability-discovery.md) `CapabilityNegotiator` → Provider Adapter 翻译，不再经 P6-9 `provider_options`；`ReasoningEffort { None, Low, Medium, High, XHigh, Max }` 由 P15-8 定义为 canonical 枚举。**Profile 不得包含 Provider-specific reasoning 字段**；Provider-specific 剩余特殊配置仍可经 extension/options 旁路，但 canonical effort 必须是一等字段，Agent Core 不按 Provider 名分支。
5. **background / isolation** —— 目的：background（接入 [P16-4](P16-4-background-task-manager.md) 后台任务）声明该 agent 可后台运行；isolation（接入 P11 sandbox）声明运行隔离等级（none / restricted / container）。
6. **校验与迁移** —— 目的：加载时做完整性 / 越权 / 冲突校验（与 `policy-engine` 协作），v1→v2 自动迁移；profile 本身不携带明文 secret。
7. **定向 / Mock 测试** —— 目的：v2 profile 加载与校验、denied 生效、引用解析失败降级、v1 迁移、isolation / background 正确传递。仅定向 + Mock。

## 主要产出物

- `agent-domain` profile v2 类型
- `resource-loader` 的 v2 加载 / 校验 / 迁移
- 定向测试

## 验收标准

- [x] v2 profile 覆盖 prompt / model / effort / tools(denied) / skills / mcp / permissions / hooks / memory / max-turns / background / isolation 全部维度
- [x] `effort` 为 canonical 一等字段（`ReasoningEffort`），经 P15-8 协商翻译，不经 `provider_options`；Profile 不含 Provider-specific reasoning 字段（`no_provider_branch` 断言）
- [x] denied 工具不可被任何方式绕过
- [x] 引用解析失败 / 越权时安全降级或报错
- [x] v1 profile 可迁移到 v2；profile 不含明文 secret
- [x] **（P16-10 延期接线）生产长期记忆**：当前选择验收条款中的安全降级路径：保留 provider-neutral contract、保持 default-off，并在 profile 显式标注 unavailable；RunStart 对 `enabled + unavailable` fail-closed，不虚假可用。真实 `EmbeddingProvider` + SQLite 持久化 + context 消费仍由 P16-10 后续接线。见 [p16-review §1/§3.5](../docs/review/p16-review.md) 与 [plan/README Phase 16 登记](README.md)。

## 实现（2026-08-12）

- `agent-domain` 新增 `profile.rs`：`AgentProfileV2` 全维度类型 + `ProfileToolRules`
  （deny 优先裁决 `policy()`）+ `ProfileRef`（id + version pin）+ `ProfilePrompt` /
  `ProfileModel` / `ProfileMemory`（default-off + 显式 `unavailable`，availability()
  fail-closed）/ `ProfileIsolation`（none / restricted / container）。`ReasoningEffort`
  由 `provider-api` 移入 `agent-domain::reasoning`（canonical 一等字段），`provider-api`
  重导出并保留 `clamp_effort_to_thinking_level` 自由函数；`agent-domain` 仍零依赖。
- `resource-loader`：profile 文件解析支持 v1/v2（`schema` 显式或按字段形态推断），
  v1 自动迁移 v2（instructions→prompt.system、default_provider/model→model.*）；
  加载校验 fail-closed：deny 优先、工具名/引用 id 标识符校验、重复拒绝、version
  pin（semver / `*` / `latest`）、max_turns≥1、明文 secret 结构（deny_unknown_fields）
  + 值级高信号模式扫描；memory 默认 off，`enabled=true` 且生产记忆不可用
  （`ResourceLoaderOptions.memory_available=false`，P16-10 前默认）时显式标注
  `Unavailable` + warning，绝不虚假可用；跨类引用解析（skills 按 id+semver 需求、
  hooks 按 id，bundle 内解析失败即报错并移除该 profile；mcp/permissions 由消费方
  解析，本层只做格式校验）。`ResourceBundle` 新增 `profiles_v2:
  Vec<LoadedAgentProfileV2>`（v1 兼容视图 `profiles` 保留）。
- 定向测试 14 项：全维度往返、v1 迁移、deny 优先、重复/非法工具名、引用解析
  成功/失败/版本不匹配、version pin 非法、memory 三态、明文 secret（结构+内容）、
  schema/max_turns/prompt 校验、诊断去重。`no_provider_branch` 守护回归通过
  （`agent-domain/src/reasoning.rs` 无 Provider 名，`ReasoningEffort` 仍被守护）。

```text
Validation Level: L1
Affected crates: agent-domain、provider-api、provider-runtime、resource-loader
Validated: cargo test -p resource-loader -p agent-domain -p provider-api -p provider-runtime
           cargo clippy -p resource-loader -p agent-domain -p provider-api -p provider-runtime --all-targets -- -D warnings（仅剩 P17-1 hooks.rs 并行改动的 2 项既有 lint）
           cargo fmt -- --check
           cargo test -p agent-engine --test no_provider_branch
           cargo check -p app-service -p cli-host -p core-runtime -p gui-protocol -p core-api
Targeted regressions: no_provider_branch 守护、provider 适配 crate 编译
Full workspace gate: NOT RUN（未命中升级条件；library 层定向验证）
```

**相关文档**：[P8-5 Profiles v1](P8-5-profiles.md) · [skills](../docs/features/skills.md) · [mcp](../docs/features/mcp.md) · [policy](../docs/features/policy.md) · [sandbox](../docs/features/sandbox.md) · [P16-7 Long-term Memory](P16-7-long-term-memory.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；扩展 `agent-domain` + `resource-loader`，依赖方向不变。profile v2 类型只引用各子系统 domain 类型，不依赖 infra，保持 `agent-domain` 纯净。

## 实现（2026-08-13）· RunStart 主链接线（app-service）

> 状态：**Built**（待主验收）。library 层与 RunStart 主链均已落地并自动门禁通过；
> 剩余唯一未验收项为 P16-10 延期接线（生产长期记忆，见上「验收标准」未勾选项），
> 维持显式 unavailable + fail-closed，不虚假可用。

- `core-api`：`AppCommand::RunStart` 新增可选 `profile: Option<String>`（schema
  typegen 同步），`AppCommand.d.ts` / `AppResponse.d.ts` 由 `schema-typegen` 校验一致。
- `app-service`：`CommandRouter::handle_run_start` 主链接线——
  - **解析**：`resolve_run_profile` 把 profile 名经 `RunProfileResolver`（trait，
    宿主用生产 ResourceLoader 装配注入）解析为 loader 已校验的不可变
    `AgentProfileV2`；未知 / 跨 workspace / 引用不可用 / 未注入解析器 / 无
    workspace 绑定一律结构化 fail-closed，绝不静默回退默认模型或默认 profile。
  - **模型与显式覆盖政策**：显式命令 `model` 优先（caller 权威）→ 否则
    profile.model canonical 解析（provider 必须已注册，fail-closed）→ 否则默认。
  - **reasoning**：`profile.effort`（canonical `ReasoningEffort`）经
    `provider_api::ReasoningConfig` 流入 ProviderLoop（不经 `provider_options`）。
  - **max_turns**：`profile.max_turns`（默认 16）成为 ProviderLoop `max_iterations`
    硬上限。
  - **deny-first tools**：`ProfileToolRules`（agent-domain，deny 优先裁决）随 run
    携带到权威 `pre_tool` 位点：allowed 非空时白名单过滤，随后 denied 一律移除，
    同名 allowed+denied 按 denied 处理，不可绕过。
  - **background TaskManager**：`background=true` 经 `TaskManager` 注册/启动
    `TaskKind::Agent`，终态在 run 任务内收尾；未注入 TaskManager 时 fail-closed。
  - **retry 保留**：retry 沿用同一不可变 `ResolvedRunProfile`（tool_rules /
    isolation / background 继续生效，重新注册 Agent 任务）。
  - **isolation fail-closed**：`IsolationCapability`（生产
    `SandboxIsolationCapability`）在 RunStart 判定 `satisfiable`；主 run 链
    当前无真实隔离执行器接线（见下「主审修复」），Restricted / Container
    一律拒绝，绝不静默降级。
  - **refs / memory**：引用解析失败由 resolver 报 `ReferenceUnavailable`；
    `memory.enabled + unavailable` 拒绝 run（fail-closed，不虚假可用）。

### 验证（2026-08-13）

```text
Validation Level: L1
Affected crates: agent-domain、resource-loader、core-api、app-service
Validated: cargo check -p app-service
           cargo test -p app-service --test p17_5_run_profile   # 9/9 通过
           cargo test -p app-service                            # 81 单测 + 集成全绿
           cargo clippy -p app-service --all-targets -- -D warnings
           cargo test -p agent-domain -p resource-loader        # 22 + 77 通过
           cargo run -p schema-typegen -- --check               # d.ts 一致
Targeted regressions: p17_5_run_profile 定向（解析/模型覆盖/reasoning/max_turns/
                      deny-first/background/retry/isolation/memory/refs）
Full workspace gate: NOT RUN（未命中升级条件；P17-5 主链定向验证）
```

## 实现（2026-08-13）· 模型覆盖授权（ModelOverridePolicy，主审缺口修复）

> 状态：**Built**（待主验收）。修复主审缺口：此前 RunStart 对
> profile + 显式 model 直接信任 caller（`(Some(model), _) => 显式模型优先`），
> 违反 “only policy permitting”。现于 app-service P17-5 边界新增可注入、
> 结构化的 `ModelOverridePolicy`，显式模型与 profile canonical 落点不同即
> override，须经策略授权（resolve 后 / record_run 前）；同模型不误拒。

- **类型（`app-service::profile_resolver`，P17-5 边界）**：
  `ModelLanding`（provider + model canonical 落点对）、`ModelOverrideRequest`
  （source + identity + workspace + profile_name + from/to）、
  `ModelOverrideDecision::{Allow, Deny}`、`ModelOverridePolicy` trait。
- **缺省 fail-closed**：`DenyAllModelOverridePolicy` 为未注入时的唯一行为，
  一律拒绝——绝不直接信任 caller。
- **生产策略**：`ProductionModelOverridePolicy`（pawork 正式宿主显式注入）——
  最多允许 LocalCli / LocalGui + LocalUser 覆盖；Remote / Automation / Plugin
  / MCP 一律拒绝；**System 默认拒绝**（理由：System 用于内部 / 无人值守服务
  动作，模型覆盖应走显式 profile / 配置而非隐式 caller 权威；确需放行时宿主
  注入自定义策略显式授权）。
- **router 接线**：`handle_run_start` 拆分显式模型分支——有 profile 时先求
  canonical 落点（`profile_canonical_landing`：profile 未声明 provider / model
  名返回 `None`，显式模型仅为补全、不构成 override；provider 声明但未注册
  fail-closed），再与显式模型解析落点比较；不同则 `authorize_model_override`
  以 source + identity + workspace + profile/from/to 提交策略，Deny 返回
  `AppServiceError::Authorization`（record_run 之前）。同模型经别名 / 大小写
  归一后落点相同，不触发授权（不误拒）。
- **宿主注入**：`apps/pawork/src/main.rs` P17-5 装配块显式注入
  `ProductionModelOverridePolicy`（覆盖 CLI / GUI / ACP / headless 全部入口）。
- 定向测试：默认拒（未注入 DenyAll，LocalCli+LocalUser 也拒）、远程拒
  （RemoteGui 生产策略拒）、显式 allow gate 放行（LocalCli+LocalUser 生产
  策略放行）、同模型不误拒（DenyAll 下别名同落点放行）、profile 无模型时
  显式模型补全不误拒；`profile_resolver` 单测覆盖 DenyAll / Production 各
  source+identity 组合。

### 验证（2026-08-13）

```text
Validation Level: L1
Affected crates: app-service、pawork（宿主注入）
Validated: cargo test -p app-service --test p17_5_run_profile   # 14/14（含新增 5 项）
           cargo test -p app-service                            # 全绿
           cargo clippy -p app-service --all-targets -- -D warnings
           cargo check -p pawork
           git diff --check
Targeted regressions: p17_5_run_profile 模型覆盖四向 + 补全非覆盖、policy 单测
Full workspace gate: NOT RUN（未命中升级条件；app-service 定向 + 全 crate 验证）
```

## 实现（2026-08-13）· 主审修复（source/identity 权威重写 + 隔离 fail-closed + deny-first 主链证据）

> 状态：**Built**（待主验收）。修复主审四项缺口：

1. **Host 权威重写 source/identity（wire 不可伪造）**：此前 GUI / headless
   线上信封的 `source` / `identity` 原样进入 app-service（客户端可伪造
   LocalGui/LocalUser 绕过策略）。现改为宿主侧权威盖戳，wire 值一律丢弃：
   - **headless**（`cli-host`）：command 与 query 信封固定重写为
     `CommandSource::Automation` + `ActorIdentity::Automation { name: "headless" }`；
   - **GUI**（`gui-server`）：按连接层事实 `ConnectionLocality` +
     服务端分配的 client_id / connection_id 盖戳——`Local` / `InProcess`
     → `LocalGui { client_id }` + `LocalUser { actor_id: client_id }`；
     `Remote` → `RemoteGui { client_id, connection_id }` +
     `AuthenticatedClient { actor_id: client_id, subject: connection_id }`
     （GUI 协议尚无 per-user 身份，远程动作归属到已验证连接；任何授权策略
     均不把 RemoteGui 当本机来源，fail-closed 语义不受影响）。query 同理。
   - 授权策略（ModelOverridePolicy / quota）看到的一律是服务端盖戳值，
     wire 伪造值不进入 app-service（defense-in-depth：源统计与身份统计
     同样只记录盖戳值）。
2. **Restricted / Container 无真实隔离执行器 fail-closed**：主 run 链工具
   执行为 P13-1 no-op runtime，`AppLoopContext` 只把 isolation 作为约束
   上下文传播、不强制——`SandboxIsolationCapability.soft_isolation_available()`
   由 `true` 改为 `false`，Restricted 与 Container 一样在 RunStart fail-closed，
   绝不虚假可用。真实执行器（sandbox-runtime NativeRestricted / 平台硬隔离
   后端）接入工具执行路径后按能力翻转。
3. **deny-first 主链证据**：新增主链测试——provider 同一轮提出 allowed
   （read_file）+ denied（shell）调用，权威 `pre_tool` 位点过滤：denied
   不执行（无 `ToolExecutionStarted`）、以拒绝结果回填（`is_error` 视图），
   allowed 正常执行，run 进入终态。
4. **工具目录如实边界**：deny-first 过滤在权威 pre_tool 位点生效（主链
   证据如上），但**真实工具注册表 / 工具执行尚未接线**——工具执行为
   P13-1 no-op runtime（返回占位成功结果），真实 tool registry /
   process-runtime 执行链路接入为后续任务；plan 不声称端到端工具隔离已
   达成。

### 验证（2026-08-13 · 主审修复）

```text
Validation Level: L1
Affected crates: cli-host、gui-server、app-service
Validated: cargo test -p app-service --test p17_5_run_profile   # 16/16 通过
           cargo test -p gui-server                             # 含线上伪造盖戳回归
           cargo test -p cli-host --lib                          # 含 headless 盖戳回归
           cargo clippy -p app-service -p gui-server -p cli-host --all-targets -- -D warnings
           git diff --check
Targeted regressions: 线上 source/identity 伪造四向（headless/GUI × command/query）、
                      Restricted 无执行器 fail-closed、deny-first 主链证据
Full workspace gate: NOT RUN（未命中升级条件；P17-5 主审定向验证）
```
