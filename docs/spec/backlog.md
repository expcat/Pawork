# 产品候选与激活规格

> 基线日期：2026-08-25。这里汇总产品候选和转正规则；已排期的剩余债务见 [ROADMAP §4](../../ROADMAP.md#4-ui-之后的剩余任务)，开放决策与候选见 [ROADMAP §5](../../ROADMAP.md#5-开放决策与候选池)。本页没有创建下一版本，也不等于排期。

## 1. 候选转正闸门

候选只有同时满足以下条件才进入下一产品线：

1. 用户选择真实目标用户、场景和成功指标，并明确版本/阶段编号；
2. 在 ROADMAP 登记状态、前置、写入集、复活资产和停止条件；
3. 任务书写清需求/非目标、用户流程、现状证据、契约/迁移、安全/Secret/Policy、GUI/a11y、验证与人工证据；
4. 改冻结契约或架构红线时先起草 ADR，Accepted 后才能实施；
5. 先证明消费面，再复活 `v2-final` 资产；禁止把归档代码整包复制回主干库存；
6. 发布类候选必须先解决 License，并单独定义三平台/供应链/安装/回滚门禁。

小功能直接以任务书承载，只有跨任务共享且内容足够大时才使用 [Feature Spec 模板](feature-template.md)。不会用 P20 或批量空任务书预占未来进度。

## 2. 已确认、未排期：多账户与缓存功能族

用户已确认 G1–G6 的方向，但尚未立项和实现完整产品面；G7/F6 明确维持不内建。

| ID | 功能 | 优先级 | 状态/激活要求 |
| --- | --- | --- | --- |
| G1 | 同 Provider 多账户池与订阅 plan 凭证 | P1 | 已确认未排期；需重写 account factory 装配和 `pawork accounts` 产品面。 |
| G2 | 额度窗口跟踪与预算 gate | P1 | 已确认未排期；需定义 QuotaSnapshot 来源、错误/响应头归一和远端适配边界。 |
| G3 | 缓存感知的会话—账户亲和路由 | P1 | 已确认未排期；默认 sticky、新会话再平衡、分类错误 rebind。 |
| G4 | 子 Agent 声明式 provider/model/account 绑定 | P1 | 已确认未排期；需 RouteContext 与 budget gate 接线。 |
| G5 | canonical 输入缓存策略控制 | P1 | 已确认未排期；会扩展 canonical request/usage，必须 golden 先行。 |
| G6 | 账户/端点配置导入 | P2 | 已确认未排期；Secret 必须直接进入 auth backend，不经中间文件。 |
| G7 | 对外账户池网关 | P3 | 明确不内建；近期用 openai-compatible 上游连接外部网关。 |

设计与决议全文见 [design §3](../design.md#3-已确认扩展功能族多账户额度切换子-agent-路由与输入缓存g1g7) 和 [references 附录 C（决策 D1–D8）](../references.md#附录-c-决策记录-d1d8-与并入约定原-researchmulti-account-quota-plan-mergemd)。

## 3. 功能对照候选池（28 项）

下表是 [design §4](../design.md#4-候选功能对照未排期对照-opencode--pi--codex--deepseek-harness) 的索引；实际合计 **28 项：P1 5、P2 17、P3 6**。

| ID | 候选 | 优先级 |
| --- | --- | --- |
| A1 | 自定义 slash 命令 / Prompt Templates | P1 |
| A2 | `pawork init` AGENTS.md 生成器 | P1 |
| A3 | Turn 级 undo/redo | P2 |
| A4 | 写后自动格式化 | P2 |
| B1 | webfetch + websearch 内置工具 | P1 |
| B2 | question 结构化问答工具 | P2 |
| B3 | todowrite 轻量任务清单工具 | P2 |
| B4 | 工作区外 References | P2 |
| B5 | 图片输入与多模态 | P1 |
| B6 | 图片生成工具 | P3 |
| B7 | Pawork 作为 MCP Server | P2 |
| B8 | Code Mode / 单轮组合多步工具 | P2 |
| B9 | 会话级 Goals | P2 |
| C1 | 能力包打包与 git 分发 | P2 |
| C2 | 用户级 memories | P2 |
| C3 | Connector directory | P2 |
| C4 | LSP 自动安装矩阵 + diagnostics | P3 |
| D1 | 第一方 IDE 扩展 | P1 |
| D2 | GitHub/GitLab CI bot | P2 |
| D3 | 会话公开分享 | P2 |
| D4 | Web UI 浏览器客户端 | P2 |
| D5 | 自更新与多渠道安装器 | P2 |
| D6 | Cloud 执行环境 | P3 |
| D7 | Slack/Linear 等 chat 平台集成 | P3 |
| D8 | 更多订阅 plan 登录 | P2 |
| E1 | Enterprise SSO + 组织配置 | P3 |
| E2 | Bedrock/Vertex 模型源 | P2 |
| F1 | 版本自检 + 可选遥测 + 离线模式 | P3 |

## 4. 其它产品候选/归档复活面

| ID | 候选 | 当前状态 | 激活条件 |
| --- | --- | --- | --- |
| BK-REMOTE-01 | 远程 GUI transport | 归档/候选 | 按当时 API 版本重评 TLS、认证、授权与远程威胁模型。 |
| BK-WORK-01 | teams / goal / automation / monitor | reducer 已归档，事件保留 | 先定义真实产品面与持久化/调度语义，再按 `v2-final` 考古。 |
| BK-GIT-01 | GUI Branch/Stash/Conflict/History/Commit | 归档/候选 | 产品定义 + host protocol + Policy；不得让 Desktop 直连 Git。 |
| BK-GIT-02 | Desktop stage/unstage/hunk | 候选 | 新增双向 wire、审批/回滚语义和 ADR；当前 Changes 保持只读。 |
| BK-EXT-01 | WASM 插件/市场/Hooks/LSP 生态 | 候选 | 只允许纯 Rust/WASM 路线；不引入 Node/Bun/JS Runtime。 |
| BK-EGRESS-01 | egress broker + 域名白名单 | 候选 | 另立 ADR/任务，代理与 Sandbox 两层 enforcement；当前网络 allow-hosts 不存在。 |
| BK-ART-01 | GUI ArtifactStreaming | 候选 | registry/实现/授权/背压/恢复同时落地后才宣告 capability。 |
| BK-TERM-01 | 终端命令级交互审批 | ADR 候选 | 泛化审批事件/命令关联并补 Desktop 渲染；当前 AskUser 对 terminal_create 为 Deny。 |
| BK-AT-01 | Composer `@` 模糊补全 | 候选 | 新增受控 file-index query（gui.available）与 Desktop 浮层。 |
| BK-RES-01 | Resources 已加载规则分区 | 候选 | host 暴露实际加载的 AGENTS.md/Skills query 后再渲染。 |
| BK-RESP-01 | 1080–1279 窄窗自适应 | 已接受延期 | TaskRail 240px/Inspector 默认折叠需单独 UI 任务和截图验收。 |
| BK-RELEASE-01 | 发布、全量门禁、三平台矩阵 | 未授权 | License 确定 + 用户明确授权后另立任务。 |

## 5. 架构排除项

以下不进入路线图：

- 交互式全屏 TUI；Pawork 采用 CLI 交互 + GPUI Desktop。
- Node/Bun/V8/嵌入式 JS/TS 插件运行时及 hot reload。
- npm 生态作为 Pawork SDK/插件的必需运行时或传输层。
- 当前阶段内建对外账户池网关（G7/F6）；可连接兼容的外部网关。

若要推翻排除项，必须先处理纯 Rust/无 TUI 等架构红线并由用户通过 ADR；普通 Feature Spec 无权覆盖。

## 6. 下一产品线入口

当前没有活动的“P20”或下一版本 Roadmap。立项时应由用户从以下三类中选择一个主目标，避免把候选池全部并入同一版本：

1. **本机多账户与成本效率**：G1–G6；
2. **Desktop 完整编码工作台**：Git 写入、`@` 补全、规则可见性、可访问性/响应式；
3. **分发与集成**：IDE/CI/Web/安装发布等 D 类候选。

选择后再创建产品 brief、下一版本 ROADMAP 和开启编排，明确不做项、成功指标、阶段数量与证据预算。未选择前，本页只作为决策输入。
