# S6：多 Provider 与认证

> 阶段 S6 · Provider 扩容与认证 · 状态：⚪未开始 · 依赖：S2（anthropic 最小版在位）· 规模：大 ·（与 S5/S7 可并行）

## 目标（本阶段结束时用户能做什么）

从「两条测试通道」升级为完整多厂商支持：八厂商适配器（openai / anthropic / google / xai / zhipu / qwen / moonshot / openai-compatible base）按 feature 齐备，`pawork models` 聚合全部已配置 Provider；API key 存入 OS Keychain（`pawork auth` 子命令），环境变量降级为 headless/CI fallback；OAuth（PKCE/Device/refresh/callback）能力就绪；全局脱敏 tracing layer 上线，secret 不入日志有系统性保障。

## 涉及包与 V1 资产

| V2 包（目录） | 本阶段动作 | V1 来源与方式 |
| --- | --- | --- |
| `pawork-providers` | 增强：迁移全部厂商 adapter 完整版——`provider-openai`（含 Responses 协议）、`provider-anthropic` 完整化（prompt cache、thinking 配置，补齐 S2 最小版）、`provider-google`、`provider-xai`、`provider-qwen`、`provider-moonshot`、`provider-zhipu`；**openai/xai 两份约 1.3k 行 Responses 流组装器下沉为共享模块，合并后 golden 无差异**；厂商错误码归一数据表；每厂商 `builtin_models()` 目录并入 registry；**zhipu adapter 增加 Coding Plan 端点预设**（`/api/coding/paas/v4` 与 `/api/anthropic`，V1 默认值 `/api/paas/v4` 是标准计费端点，保留但不再是 coding 场景默认） | 直接迁移（[archive/M2](archive/M2-providers.md) pawork-providers 节全文适用） |
| `pawork-auth`（providers/auth） | 激活：V1 `auth-service` 整包——`backend`（OS Keychain）、`oauth`（PKCE/Device/refresh/callback）、`credential`、`masked`（脱敏）；凭证解析链：Keychain → env fallback（V2 新增语义，显式登记）→ 无凭证（openai-compatible 免认证场景） | 直接迁移（[archive/M2](archive/M2-providers.md) pawork-auth 节） |
| `pawork-diagnostics`（foundation/diagnostics） | 激活：V1 `diagnostics` 迁移，脱敏 tracing layer 在 `pawork-app` 装配时全局挂载（修复 V1 仅 resource-loader 消费的缺口）；脱敏规则与 `pawork-auth::masked` 语义对齐 | 直接迁移（[archive/M0](archive/M0-skeleton-foundation.md) pawork-diagnostics 节） |
| `pawork-config` | 增强：凭证引用解析接 `pawork-auth`（配置仍无 api_key 字段）；`default_provider`/`default_model` 与多 Provider 目录联动 | 接线 |
| `pawork-cli` | 增强：`pawork auth set-key <provider>`（交互式读入、存 Keychain、回显掩码）、`auth list`（掩码显示来源：keychain/env）、`auth remove`；REPL 内 `/model` `/provider` 切换命令 | 新写 |

## 关键任务

1. **厂商迁移与 golden**：每厂商 1–2 条 contract golden（V1 随迁）；feature per vendor，默认 feature 只含 `openai-compatible` + `anthropic`（两条真实通道所需），全家桶经 `--all-features`。
2. **Responses 组装器下沉**：合并前先迁 V1 双方 golden，合并后 diff 为零（旧 M2 退出硬指标，原样保留）。
3. **auth 链路**：`set-key` → Keychain 存取 → 构造 `ResolvedCredential`；env fallback 只在 Keychain 无此条目时启用并在 `auth list` 标注来源；OAuth 全流程回归（V1 测试随迁），真实 OAuth 厂商接入待有账号时冒烟（不阻塞退出）。
4. **脱敏三线对齐**：`ResolvedCredential` Debug 脱敏（S0 起）、`auth::masked`、diagnostics layer 规则一致；日志全链扫描断言。
5. **切换体验**：会话中途 `/model` 切换后续轮走新模型，事件流记录模型变更（`ProviderRequestStarted` 携带）。

## 真实测试与评估（冒烟清单）

- [ ] `pawork auth set-key glm-coding` + `set-key opencode-go` → 删除两个环境变量 → `pawork chat` 正常工作（Keychain 生效）。
- [ ] `pawork auth list`：两条目掩码显示、来源=keychain；再设 env 变量后新增第三个 provider 未存 Keychain → 来源=env。
- [ ] `pawork models`：聚合显示两通道全部模型（含 context window/定价）；`--all-features` 构建下八厂商 feature 编译通过。
- [ ] REPL `/provider opencode-go` + `/model kimi-k2.x` 切换后续聊正常；`sessions show` 可见模型切换记录。
- [ ] GLM Anthropic 端点在 anthropic adapter 完整版下复测 S2 工具任务（prompt cache 开启前后差异记录）。
- [ ] 日志红线：开启最详细日志级别跑完整任务 → 日志文件与终端输出 grep 不到任何 key 片段（自动化 + 人工双查）。

## 定向自动化测试

- `cargo test -p pawork-providers`（default 与 `--all-features` 两档）：每厂商 golden、Responses 合并零差异、错误码归一表。
- `cargo test -p pawork-auth`：Keychain 后端（Windows Credential Manager 实测）、OAuth PKCE/Device/refresh/callback 回归、masked 脱敏。
- `cargo test -p pawork-diagnostics`：脱敏 layer 规则覆盖已知 secret 字段模式；含 token 输入的 tracing 输出断言已脱敏。
- 凭证解析优先级矩阵（keychain/env/none）单测。

## 退出标准

- [ ] 冒烟全项通过；Keychain 为主、env 为显式 fallback 且有回归测试（S0 过渡机制正式收编）。
- [ ] 每厂商 golden 通过；Responses 组装器合并 golden 无差异；错误码归一数据表就位。
- [ ] Secret 不入日志：diagnostics layer 全局挂载 + 三线脱敏对齐断言全绿。
- [ ] zhipu Coding Plan 端点预设可用（配置一个 id 即得正确端点）。

## 为后续阶段预留 / 明确不做

- 预留：OAuth 能力就绪但真实厂商 OAuth 冒烟按 key 可得性推迟登记；`negotiate`/capability 协商为 S9 多客户端能力声明铺路。
- 不做：Provider 账号池/租约/路由（S11 provider-control）、远端配额监控（冻结候审）。

## 并行拆分建议

- 波 A（并行 ×3）：厂商 adapter 迁移按厂商分组（openai+xai 一组因 Responses 合并；anthropic 完整化一组；google+qwen+moonshot+zhipu 一组——注意 google 是 Gemini 专有协议、工作量最大，qwen/moonshot/zhipu 均为 openai-compatible 薄封装）。
- 波 B（并行 ×2）：`pawork-auth`；`pawork-diagnostics`。
- 波 C（串行）：config/cli/app 接线 + 全链日志扫描 + 冒烟。

## 参考

- [../docs/design.md](../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../docs/references.md)（参照项目手册）
- [../docs/task-guide.md](../docs/task-guide.md) §5（通道端点与 key 约定）
- [archive/M2-providers.md](archive/M2-providers.md)（providers/auth 迁移细则——本阶段主文档）
- [archive/M0-skeleton-foundation.md](archive/M0-skeleton-foundation.md)（diagnostics 细则）
