# R9–R11 — UI 后一致性、真实回归与发布准备

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

## R11 — 发布准备（条件阶段）

R11 只有用户再次明确授权发布后才启动：

1. 先确认 License 与 crates.io 占名策略；
2. 冻结供应链、签名、安装/卸载、自更新、升级/回滚和 Secret 迁移方案；
3. 定义 macOS/Linux/Windows 的发布矩阵、全量门禁、安装包冒烟和回滚演练；
4. 经用户确认后执行提交、推送和发布动作。

未获授权时保持 ⚪，不得把“排入 ROADMAP”解释为发布许可。
