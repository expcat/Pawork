# R9–R11 — UI 后一致性、真实回归与设计稿终局比对

> 状态：⚪ 未开始
> 前置：R8 已完成全功能 UI suite、99% 三图门禁和用户签字。本任务书只保留尚未完成的工作；历史阶段与已交付细节统一见 [docs/history.md](../docs/history.md)。

## R9 — 一致性与代码债务收口

### Wave A：事实源与断言

- 核对 README、AGENTS、ROADMAP、architecture/design/gui-design、产品与包级 Spec、flows、ADR 与 history 的状态、链接和 21 包布局。
- 抽查包级 Spec 的模块树、公开 API、feature、依赖边与红线；冲突以源码为准并同批回写。
- 复核 desktop deny-list、engine domain-only、rmcp 隔离、policy 成环与副作用 `Result` 不静默等断言仍覆盖当前结构。

### Wave B：小型剩余债务

- 修复 usage record id 多轮冲突并补幂等回归。
- 清理 policy/workflow/orchestration 已确认的死依赖、过期描述和注释；不借机重构无关代码。
- 到期且兼容窗口满足后移除 `StoredCredential` serde alias。
- 合并 protocol 重复测试箱，评估并移除不再需要的 client dev-dep。
- 将 resources 残余路径判断统一到 policy `canonical_within` / 路径内核。
- 复查 Claude import 五项 P3：多 text part 分隔、缺失 id 对的 fail-closed、首行嗅探上界、部分损坏/unknown_fields 可见性、扫描根 symlink；只有真实影响成立才立窄修复。
- 清理 UI 主线未顺带关闭的低风险残项：heartbeat pump 可观察测试、极窄窗口 client 状态竞态、`mcp_list` 死分支等；BackToBottom、窗口 metrics、Terminal AlwaysAsk 测试应优先在 R2/R4/R6/R8 原阶段关闭，不得拖到本阶段。
- 复查上游重复版本、usage 哨兵、shell wrapper 与 probe flake；超出小任务或涉及 wire/schema 时登记候选或先立 ADR。

### R9 退出标准

- [ ] 常设文档、Spec、ADR、断言与源码一致，无旧阶段任务死链。
- [ ] 列出的剩余债务已修复并运行各写入集定向测试，或因明确前置移入候选且说明证据。
- [ ] 已完成细节移入 history，ROADMAP 只保留下一未完成指针。

## R10 — 关键回归与真实环境验证

### Wave A：关键契约

- K-01：`.pawork/config.toml` 在 git 根、git 子目录和非 git 目录三态的发现/合并行为闭环。
- 安全红线：路径越界、symlink、`.git` 写、审批 deny、Sandbox fail-closed/可观察降级、Secret 脱敏与外部 Secret 拒绝。
- 持久化与重放：envelope、schema 升级、lineage/compaction、PWB1、checkpoint、export/import、projection、CommandLedger 崩溃/重试。
- 协议与解析：GUI frame、headless JSON、ACP、MCP、registry fail-closed、config 矩阵和 usage dedup。

### Wave B：真实通道与客户端

- 低消耗矩阵四通道各一轮 chat；`gui serve` + Desktop probe-smoke/真窗口、Zed ACP、headless json-stdio、typed client 与 `pawork doctor --json`。
- ChatGPT/xAI 在自然临期 token 上验证 refresh → retry → success 与 `invalid_grant` 清理。
- 真实 Anthropic/GLM Anthropic 端点、fork/compact 与其它仍缺真实证据的主路径逐项执行或明确登记阻塞。

### Wave C：人工/平台挂账

- kill -9、ACP 双连接交错、Seatbelt 真机探针、Windows SCM/Job 等不能由 mock 代替的非 UI 项目。
- Linux/Windows 缺平台项分别记录真实验证、仅编译证明或未验证；不得把 macOS UI 门禁写成三平台发布证明。

R10 不接收未通过的 Desktop UI 项；出现此类缺口即退回 R8，不在本阶段重复登记或降级放行。

### R10 退出标准

- [ ] 三类关键回归全绿，K-01 闭环。
- [ ] 四通道与计划内客户端实际通过或以可复现外部阻塞明确登记。
- [ ] OAuth refresh、历史人工项与平台证据逐项有结论；无虚构“已验证”。
- [ ] 形成收口摘要，仍不执行发布级 workspace full gate。

## R11 — 设计稿与实际 UI 终局比对

R11 是文档任务。对照 [design/](../design/README.md) 三张 v3 定稿图与已归档的实际 UI 证据，把仍不符合的显示效果归纳为下一阶段完善任务。本阶段**不查询、不修改任何代码**；不启动 Desktop、不重跑 cargo、不重拍 current、不改 design。发布准备已移出本编号，见 [ROADMAP §5](../ROADMAP.md)。

R11 不替代 [R8 退出标准](R7-R8-ui-quality-gates.md#4-r8-退出标准) 的 99% 门禁与全功能 suite；也不在本阶段修复差异。

### 比对输入（只读）

- `design/`：Timeline（Inspector 展开）、Timeline（Inspector 折叠）、Projects 三张 v3 定稿图。
- [docs/gui-design.md](../docs/gui-design.md) 的信息架构与交互规则（用于判断缺状态、缺分区，而不只看像素）。
- [docs/UI_Review.md](../docs/UI_Review.md) 的分区、容差与结构一票否决（用于区分合同内误差与需完善项）。
- R8 归档的三状态 `reference` / `current` / overlay / diff / mask / checklist，以及 R2–R7 分波证据作分区线索。不得打开 `apps/desktop` 或其它 crate 源码定位组件。

### Wave A：逐区对照

- 按 State A/B/C 与 UI Review 分区（header、TaskRail、timeline、composer、inspector、statusbar 等）对照 design 与 current。
- 只登记**显示效果**：布局、色/字/间距、图标、文案可见性、组件有无、状态外观。截图上看不出的交互缺口标「截图无法判定」，不查代码补证。
- 容差内、已遮罩、或 R8 已明确接受的项不重复立项；结构未对齐或仍刺眼的可见差异必须登记。
- 每条写：区域、design 期望、当前现象、证据路径（design 资产 + current/diff）、建议优先级。禁止指向源码路径或「应改某函数」。

### Wave B：归纳下一阶段完善任务

- 将差异条收敛为下一阶段可独立执行的完善任务（一任务一缺口族，数小时内可完成）。
- 回写 [ROADMAP.md](../ROADMAP.md) 下一指针，并起草后续任务书；编号在该阶段开启时确定。
- 本阶段 git 差异仅文档。不得把 License、安装器、供应链或全量门禁塞进本阶段。

### R11 退出标准

- [ ] 三张定稿图与对应 current 证据已逐区对照，形成差异清单。
- [ ] 不符合的显示效果已归纳为下一阶段完善任务（含证据链接与优先级）。
- [ ] 本阶段未查询、未修改代码；design 像素未改。
