# R9–R11 — UI 修复、全功能测试与收尾

> 状态：⚪ 未开始
> 前置：R8（UI 终局比对与优化文档）已产出 `docs/ui-optimization.md`。2026-08-31 用户重排：原 R11（比对）前提到 R8；新增 R9 修复 R8 发现的问题；原 R8（模拟操作全功能验收）与原 R10（关键回归与真实环境验证）并入本阶段线 R10 测试；原 R9（一致性与代码债务）顺移为 R11 收尾。历史阶段与已交付细节统一见 [docs/history.md](../docs/history.md)。

## R9 — 修复 R8 发现的 UI 问题

### 输入与纪律

- 唯一输入是 R8 产出的 `docs/ui-optimization.md`（分区差异清单 + 主流样式对照 + 修复任务草案）；文档未登记的缺口先回写文档再实施。
- 修复顺序：结构对齐优先，其次组件样式，最后美观度打磨；不靠阴影、渐变或动画掩盖结构问题。
- 写入集最小；涉及 design 基准变更（如 State C 底色归一）须先取得用户批准；不扩张 wire/能力，不借机重构无关代码。
- 生产 UI 不写死 fixture 文案；测试数据仍经 Host、协议与实际 projection 进入 Desktop。

### 执行波次

按优化文档的缺口族划分 Wave（一 Wave 一缺口族，数小时内可完成）。每个 Wave：

- 先补可观察回归（结构/几何断言），再改视觉与样式。
- 修复后用同一 fixture 复拍受影响区域，运行写入集定向测试与受影响区域的 U2/截图复验。
- 同批更新 `docs/gui-design.md`、`design/README.md`、Desktop 相关 Spec，并把优化文档对应条目标记为已修复。

### R9 退出标准

- [ ] `docs/ui-optimization.md` 登记的缺口族全部修复，或经用户确认降级/移交。
- [ ] 每个修复 Wave 有定向测试与受影响区域复验证据。
- [ ] 优化文档条目状态已回写；无文档外施工。

## R10 — 测试

本阶段合并原 R8（模拟操作全功能验收）与原 R10（关键回归与真实环境验证）的全部合同；R2–R6 各波「移交 R8」条款全部由本阶段承接，中间态记录值不得追认为通过。

### 1. 前置：重采集准备

- **fixture 演示数据重塑**（R3 拍板 c 移交）：`fixtures/ui/seed.json` 数据形状对齐定稿图演示形状（标题长度/时间分布/会话数），同步既有 golden 与约 18 处断言引用，估算 0.5–1 天。
- **State C reference 底色归一**：定稿图中位 RGB (0,9,17) 比冻结 token base `0x07121a` 更暗；是否按冻结 token 归一属设计基准变更，重采集前必须由用户拍板，不得把当前漂移追认为新基准。
- 完成上述前置后重新采集 State A/B/C 的 reference/current，再进入视觉终局门禁；不得沿用 R2–R6 中间态记录值。

### 2. UI 全功能验收

#### 2.1 全量场景矩阵

| 领域 | 必须模拟的操作与状态 |
| --- | --- |
| 启动与连接 | 无 Host、连接、失败重试、断连、重连、window close/reopen；区分 persisted/connected/executing/blocked |
| TaskRail | Timeline/Projects、scope、project 展开、全局/定向新建、task 切换、selection/scroll/focus 恢复、Unread/Needs input |
| Composer | click/type、多行、IME、paste、model/reasoning/workspace/context/`@`、send、cancel、草稿与不可用态 |
| Timeline | stream、tool 全状态、展开/收起、approval allow/deny、error/retry、cancel、completion、follow-scroll 与千级事件 |
| Changes | 空态、真实多文件 diff、Files/Summary/DiffView、长行横滚、scope 与只读动作 |
| Terminal | create、input/output、resize、stop、失败、task/workspace 切换、重连与 Policy 拒绝 |
| Resources | 空/可用/失败、resource 打开、Add tool/capability 缺失的诚实状态 |
| Inspector/Activity | tab、二级 tab、折叠/恢复、右上 Popover、dismiss、焦点/滚动/session 保持 |
| 浮层与快捷键 | grouping/scope/model/reasoning/`@` 菜单，command palette，Tab/方向键/Enter/Esc，窗口边界与 outside click |
| 响应式/AX | State A/B/C 的 1440 图、1080 窄窗、字号放大、纯键盘、VoiceOver/AX、状态非纯颜色 |
| 生命周期 | Run 中关闭窗口、Host 仍运行、重开恢复、approval 等待恢复、完成通知与后台状态真实性 |

每一行至少覆盖成功、失败/拒绝和恢复路径；所有 manifest 组件及可达状态必须能反查到场景 ID。单条 happy path、只测 renderer 或人工随意点击均不构成“全功能”。

#### 2.2 执行策略

- 使用 R1 固定 seed 与隔离数据；每场景独立 reset，允许按标签重跑。
- 语义定位优先：AX identifier/role/name + 明确状态等待；坐标只用于几何验证，固定 sleep 只允许有界兼容并需记录原因。
- U0/U1 先行，U2 真进程覆盖所有用户动作，U3 只对稳定终态采图；真实 Provider 不属于 UI fixture。
- PR/本地默认运行 U0/U1 与小型稳定视觉集；macOS 定时门禁运行完整 XCUITest/视觉集；本阶段收口再串行执行 U0–U3、真实 IME/VoiceOver 与性能，避免并发争抢 Cargo/主线程窗口资源。
- flaky 测试不可“重跑即绿”后隐藏：记录首次失败、重试结果、随机种子和根因；同一场景连续不稳定即阻塞签字。
- failure bundle 至少含 action trace、AX tree/当前焦点、Host/event log 与协议 sequence、窗口尺寸、current/reference/overlay/diff/mask、AE/PDC/RMSE/SSIM 指标、fixture manifest、seed、OS/Xcode/GPU/scale/locale/input source/font、时间与源码状态；若使用 XCTest，同批保留失败 `xcresult`/attachments。

#### 2.3 视觉终局门禁

- State A/B/C 的可见区域、组件、顺序、展开/折叠和选中状态必须 100% 对齐。
- TaskRail、Header、Timeline、Composer、Inspector/Popover、StatusBar 各区域动态遮罩后 SSIM `≥0.99`；结构一票否决优先于数值。
- **R2 移交（2026-08-27 拍板 a）**：R2 只以壳层结构门禁退出，不把内容区未落地组件的分区像素差记为 R2 失败。本阶段必须在 F-03–F-12 落地后重新采集 State A/B/C current，再跑分区 SSIM；不得沿用 R2 Wave A 的 0.65–0.81 中间态报告作为终局通过。
- **R3 移交（2026-08-28 拍板 c）**：R3 以 TaskRail 结构门禁退出（Wave A State A/C 结构断言全 PASS；State B 与 State A 同 Timeline 模式，未单独采 TaskRail 分区图），三状态分区 SSIM ≥0.99 不在 R3 判定。本阶段重采集 current 前必须先完成 **fixture 演示数据重塑**（见 §1 前置）；并就是否按冻结 token 归一 State C reference 底色另行取得用户批准（设计基准变更）。天花板量化分解：State A ≈100% 内容形状（0.6941，tone 校正上限 0.7490）；State C = tone ≈50% + 形状 ≈50%（0.3543，tone 校正后 0.6885）。遮罩侧无合规余量（已用 16.6%/14.9%，上限 35%），不得靠放宽 UI_Review §0.1 遮罩合同制造通过。细节见 [../docs/history.md](../docs/history.md#r3--taskrail-与任务导航2026-08-2728)。
- **R4 移交（2026-08-28 拍板 1）**：R4 以 Header/Timeline 结构门禁与 U2 九场景退出，State A/B 分区 SSIM ≥0.99 不在 R4 判定。本阶段重采集 current 时一并覆盖 Header / Timeline / 相关 Workspace 分区；不得沿用 Wave A 记录值（timeline 0.665 / header-left 0.940 / header-right 0.883 / global 0.648）作为终局通过。主因与 R3 相同：fixture 演示内容形状差，重塑已在拍板 c 移交，本条不另开数据任务。细节见 [../docs/history.md](../docs/history.md#r4--workspacetimeline-与-agent-状态2026-08-28)。
- **R5 移交（2026-08-29 用户确认）**：R5 以 Composer 几何结构门禁、定向测试与 U2 九场景退出，State A/B Composer 分区 SSIM ≥0.99 不在 R5 判定。本阶段必须用重塑后的同一 fixture 重采 current 并覆盖 idle/running Composer；不得沿用 R5 Wave A 记录值 0.423 / 0.619 作为终局通过。详见 [../docs/history.md](../docs/history.md#r5--composer-与运行控制2026-08-2829)。
- **R6 移交（2026-08-30 用户确认）**：R6 以 Inspector/Activity 结构门禁、定向测试与审查后最终二进制 U2 九场景/19 断言退出，State A/B Inspector/Activity 分区 SSIM ≥0.99 不在 R6 判定。State A 中间态 Inspector 记录值为 0.614/0.800；State B 原 `current.png` 在 Popover 打开前采集，不能证明 Popover 视觉，已用正确的 `shot-activity-popover.png` 归一补录为 0.712/0.860。本阶段必须在 fixture 演示数据重塑后，以真正打开的 ActivityPopover 重采 current 并覆盖 Inspector/Popover；上述记录值均不得作为终局通过。详见 [../docs/ui-review/r6-wave-a/notes.md](../docs/ui-review/r6-wave-a/notes.md)。
- 所有 P0/P1 Review 项关闭；无白 titlebar、缺失 Header、错位 Popover、超高 Composer、假数据、遮挡、截断或布局跳动。
- 由用户在同尺寸 reference/current/overlay 上完成最终视觉签字；自动门禁通过不能代替签字。

### 3. 关键回归与真实环境

#### 3.1 关键契约

- K-01：`.pawork/config.toml` 在 git 根、git 子目录和非 git 目录三态的发现/合并行为闭环。
- 安全红线：路径越界、symlink、`.git` 写、审批 deny、Sandbox fail-closed/可观察降级、Secret 脱敏与外部 Secret 拒绝。
- 持久化与重放：envelope、schema 升级、lineage/compaction、PWB1、checkpoint、export/import、projection、CommandLedger 崩溃/重试。
- 协议与解析：GUI frame、headless JSON、ACP、MCP、registry fail-closed、config 矩阵和 usage dedup。

#### 3.2 真实通道与客户端

- 低消耗矩阵四通道各一轮 chat；`gui serve` + Desktop probe-smoke/真窗口、Zed ACP、headless json-stdio、typed client 与 `pawork doctor --json`。
- ChatGPT/xAI 在自然临期 token 上验证 refresh → retry → success 与 `invalid_grant` 清理。
- 真实 Anthropic/GLM Anthropic 端点、fork/compact 与其它仍缺真实证据的主路径逐项执行或明确登记阻塞。

#### 3.3 人工/平台挂账

- kill -9、ACP 双连接交错、Seatbelt 真机探针、Windows SCM/Job 等不能由 mock 代替的非 UI 项目。
- Linux/Windows 缺平台项分别记录真实验证、仅编译证明或未验证；不得把 macOS UI 门禁写成三平台发布证明。

UI 全功能验收未通过的项退回 R9 修复，不在本阶段重复登记或降级放行。

### R10 退出标准

- [ ] manifest 的组件 × 状态 × 输入方式覆盖率 100%，所有场景可独立重放并有明确断言。
- [ ] U0/U1/U2/U3 全部通过；跨进程、断连、恢复、后台 Run 与审批恢复有真实证据。
- [ ] 三张定稿图的结构门禁、分区 SSIM 与人工 overlay 全通过，无 P0/P1 遗留。
- [ ] 用户完成视觉签字；已知 P2/P3 只可在不破坏 99% 与全功能的前提下明确接受并登记。
- [ ] 三类关键回归全绿，K-01 闭环。
- [ ] 四通道与计划内客户端实际通过或以可复现外部阻塞明确登记。
- [ ] OAuth refresh、历史人工项与平台证据逐项有结论；无虚构“已验证”。
- [ ] 失败证据、性能基线、AX 结果与实际命令归档；形成收口摘要；ROADMAP 指针移至 R11。仍不执行发布级 workspace full gate。

## R11 — 收尾

一致性与代码债务收口（原 R9）。本阶段不涉及发布准备（[ROADMAP §5](../ROADMAP.md) 候选，须用户另行授权），也不新增门禁测试。

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
- 清理 UI 主线未顺带关闭的低风险残项：heartbeat pump 可观察测试、极窄窗口 client 状态竞态、`mcp_list` 死分支等；BackToBottom、窗口 metrics、Terminal AlwaysAsk 测试应优先在 R2/R4/R6/R10 对应阶段关闭，不得拖到本阶段。
- 复查上游重复版本、usage 哨兵、shell wrapper 与 probe flake；超出小任务或涉及 wire/schema 时登记候选或先立 ADR。

### R11 退出标准

- [ ] 常设文档、Spec、ADR、断言与源码一致，无旧阶段任务死链。
- [ ] 列出的剩余债务已修复并运行各写入集定向测试，或因明确前置移入候选且说明证据。
- [ ] 已完成细节移入 history，ROADMAP 只保留下一未完成指针。
