# 产品候选与激活规格

> 基线日期：2026-09-01。这里汇总未进入活动线的产品候选和转正规则。产品规则见 [Settings Spec](settings.md)；工程约定见 [AGENTS.md](../../AGENTS.md)。本页不等于排期。

## 1. 候选转正闸门

候选只有同时满足以下条件才进入下一产品线：

1. 用户选择真实目标用户、场景和成功指标，并明确版本/阶段编号；
2. 在本文件登记状态、前置、写入集、复活资产和停止条件；
3. 任务书写清需求/非目标、用户流程、现状证据、契约/迁移、安全/Secret/Policy、GUI/a11y、验证与人工证据；
4. 改冻结契约或架构红线时先起草 ADR，Accepted 后才能实施；
5. 先证明消费面，再复活 `v2-final` 资产；禁止把归档代码整包复制回主干库存；
6. 发布类候选必须先解决 License，并单独定义三平台/供应链/安装/回滚门禁。

小功能直接以任务书承载，只有跨任务共享且内容足够大时才使用 [Feature Spec 模板](feature-template.md)。不会用 P20 或批量空任务书预占未来进度。

## 2. 已确认：多账户与缓存功能族

用户已确认 G1–G6 的方向。G1 同 kind 多账户的 Settings 产品面与 G2 有权威来源后的额度切换已进入 UI-6；其余仍未排期完整产品面。G7/F6 明确维持不内建。

| ID | 功能 | 优先级 | 状态/激活要求 |
| --- | --- | --- | --- |
| G1 | 同 Provider 多账户池与订阅 plan 凭证 | P1 | 部分进入 [UI-6b](../review/roadmap-ui-2026-09-09.md#8-ui-6--providers供应商目录多账号)：Settings 同供应商多 OAuth / 多 API key 与账号切换。account factory 与 `pawork accounts` CLI 完整产品面仍未排期。 |
| G2 | 额度窗口跟踪与预算 gate | P1 | 部分进入 [UI-6b](../review/roadmap-ui-2026-09-09.md#8-ui-6--providers供应商目录多账号)：有权威 QuotaSnapshot 才接按额度切换，无来源不画数字。完整预算 gate 仍未排期。 |
| G3 | 缓存感知的会话—账户亲和路由 | P1 | 已确认未排期；默认 sticky、新会话再平衡、分类错误 rebind。 |
| G4 | 子 Agent 声明式 provider/model/account 绑定 | P1 | 已确认未排期；需 RouteContext 与 budget gate 接线。 |
| G5 | canonical 输入缓存策略控制 | P1 | 已确认未排期；会扩展 canonical request/usage，必须 golden 先行。 |
| G6 | 账户/端点配置导入 | P2 | 已确认未排期；Secret 必须直接进入 auth backend，不经中间文件。 |
| G7 | 对外账户池网关 | P3 | 明确不内建；近期用 openai-compatible 上游连接外部网关。 |

设计与决议全文见 [design §3](../design.md#3-已确认扩展功能族g1g7) 和 [references 附录 C（决策 D1–D8）](../references.md#附录-c-决策记录-d1d8-与并入约定原-researchmulti-account-quota-plan-mergemd)。

## 3. 功能对照候选池（28 项）

下表是未排期候选索引；实际合计 **28 项：P1 5、P2 17、P3 6**。红线排除项见 [design §4](../design.md#4-架构红线排除项不实现)。

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

## 6. Settings 线收口后的候选纪律

Settings 活动线已实现并通过本机真窗口验收（2026-09-05，证据见 [desktop.md §8](desktop.md#8-gui-收尾验收记录2026-09-05)）。OPT 活动线已关闭，全文见 [roadmap-opt-2026-09-05.md](../review/roadmap-opt-2026-09-05.md)。当前活动线是 [GUI 第二阶段](../ROADMAP.md)；UX-01～UX-09 证据见 [历史记录](../review/roadmap-ux-2026-09-10.md)，未闭合验收由当前路线图承接。Desktop 模块重设计 UI 线（2026-09-07 真窗口走查 #01–#05）已实现，全文见 [roadmap-ui-2026-09-09.md](../review/roadmap-ui-2026-09-09.md)；未创建发布版本。UI-6 吸收 G1 同 kind 多账户的 Settings 产品面，以及 G2 在有权威 QuotaSnapshot 后的额度切换。下列候选仍不自动并入：

1. **本机多账户与成本效率**：G3–G6，以及 G1/G2 超出 UI-6 的部分（account factory、`pawork accounts` CLI、完整预算 gate）；
2. **Desktop 完整编码工作台**：Git 写入、`@` 补全、规则可见性等；
3. **分发与集成**：IDE/CI/Web/安装发布等 D 类候选。

如用户选择其中一类，按 §1 闸门创建独立 Feature Spec/任务切片，明确不做项、成功指标与证据预算。发布继续维持 BK-RELEASE-01 未授权状态。

## 7. 已登记系列规划

| 系列 | 规划文档 | 状态 |
| --- | --- | --- |
| MOCK-1～MOCK-8 本地 Provider 模拟仿真 | [mock-simulation-plan.md](../mock-simulation-plan.md) | 已实施完成（2026-09-10）：MOCK-2～MOCK-8 已实现并通过 gate 与 review，端到端证据见规划 §7；MOCK-0b 保持候选未获批。 |

## 8. 实施过程登记的产品缺陷

以下缺陷在 mock 系列端到端验收（MOCK-6，2026-09-10）中发现并已定位根因，均未在 mock 写入集内修复，按最小方案另立任务处理。

### BUG-OAUTH-01：OAuth 请求前刷新被静默跳过（FileBackend 路径）

- 现象：持有已过期 access token + 有效 refresh token 的 OAuth 凭证发起对话时，`refresh_oauth_credential_with`（[oauth.rs](../../crates/auth/src/oauth.rs)）在锁内 reload 后比较整个 `StoredCredential`，`metadata_changed` 恒为真，直接 `return Ok(false)`，既不刷新也不报错；请求带过期 token 发出后被 Provider 401 拒绝。
- 根因：`load_account` 会用账户索引里的 `display_name`（默认 "Default OAuth"）覆盖凭证 meta，而 `stored_from_meta` 侧硬编码 "default oauth"；两处大小写不一致使 `metadata_changed` 在每次 reload 后恒真，提前短路刷新分支。
- 判别实验（已做）：把索引里的 `display_name` 改成与硬编码一致的 "default oauth" 后，refresh 立即触发并向 token 端点轮转成功——证明短路条件就在该比较。
- 建议修法方向：刷新前的 metadata 比较排除 `display_name`（展示字段不应参与变更判定），或统一两处命名常量；修复需补一条「过期凭证 → 自动 refresh → 轮转落盘」的定向回归。
- 修复后复验配方（mock 环境）：`scripts/mock/run-instance.sh start` 后用 `seed_auth.py` 注入 `expires_at_ms` 已过期的 OAuth 凭证，发起对话触发请求前刷新，断言 mock `/token` 端点收到 refresh 请求且 auth.json 落盘新 token；当前版本此配方会复现 401。

### BUG-USAGE-01：usage ledger request-id 跨进程撞车

- 现象：同一 data dir 内第二次起 Host 进程跑对话，usage ledger 记录报 `usage record id conflict: rec-run-...`（host.log warn），第二次运行的用量不落账。
- 根因：[services/run.rs](../../crates/app/src/services/run.rs) 的 usage record id 用进程内计数器拼 `req-{n}`，而 control-plane 侧按 (tenant, account, request_id, attempt) 去重；新进程计数器从 0 重来，与上一进程同 data dir 的记录撞 id。
- 建议修法方向：request_id 引入进程级随机前缀或持久单调序列；修复后用「同 data dir 连续两个 Host 进程各跑一次对话」回归。

### BUG-GUI-01：GUI Run 事件路径静默断连

- 现象（MOCK-6 真窗口验收中 6 次复现）：任意 Run 启动后约 1.5–10 秒，Desktop 连接被静默关闭（状态栏「已断开 · connection is closed」），慢流期间同样触发；断连后 Cancel 按钮因「需要活连接」被禁用，GUI 内 Reconnect 按钮多次点击无状态变化；Host 进程仍存活并接受新协议连接（quota 探针正常），Run 在 Host 侧继续执行直至自然完成；重启 Desktop 后完整重放恢复且 Run 不受影响。
- 已排除项：客户端 stderr 无错误输出；host.log 无连接错误；mock server 未断开 HTTP 流（Run 正常完成）。
- 影响面：GUI 取消主路径被阻断（CLI 取消已验证不受影响）；断连期间设置页刷新与额度查询同样不可达。
- 建议修法方向：定位 Run 事件（artifact/tool/usage 流）经 GUI 连接分发时的 panic 或主动 close 路径，补「Run 流式期间连接保持 + 断连可经 Reconnect 恢复」的回归；修复后用 `MOCK:SLOW_STREAM` 场景复验取消。
