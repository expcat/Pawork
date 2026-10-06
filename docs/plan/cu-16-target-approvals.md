# CU-16 · 应用与网站范围的 Host 审批

> 状态：等待人工验收（P1）。前置：[CU-02](cu-02-target-contracts.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

在现有 Policy 和显式审批上补目标范围，生产原生操作在本任务落地后才接入。

## 当前依据与读取范围

当前 computer 工具是 requires_approval 的 ExternalPlugin，拒绝 untrusted / ReadOnly，Approve for run 已接线；尚无本机应用或网站授权范围。

[policy Spec](../spec/crates/policy.md)、[tools Spec](../spec/crates/tools.md)、[app Spec](../spec/crates/app.md)、[安全规格](../spec/security.md)、[Settings Spec](../spec/settings.md)。

## 实施范围

- 将应用/网站身份与 workspace/run 审批绑定，捕获、读树、启动和输入均在实际后端访问前裁决。
- 区分 OS Screen Recording / Accessibility / Automation 权限与 Pawork 目标授权；不能代用户批准系统权限。
- 复用现有一次/本 Run 审批与拒绝语义，自动 resolver 不能冒充显式用户批准；持久允许仅按已裁决设置口径实现，并可撤销。
- 授权变化与 CU-09 占用、观测和取消联动，跨目标和失效授权不能继续派发。
- 防止 Agent 通过自身审批界面或系统权限界面构成自我批准；必要安全/持久契约变化先裁决并做 golden。

## 完成条件与验证

- 拒绝、未授权、ReadOnly 和不可信 workspace 在连接、捕获或输入前零后端访问。
- 授权只覆盖用户选定应用/网站，撤销影响后续动作且已执行历史诚实保留。
- 对应 Policy / 显式审批 / Secret 定向回归通过，不另造审批框架。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

2026-10-06 实现完成，等待指挥方契约评审（人工验收）。写入集：`crates/computer-use/src/approval.rs`（新增审批契约模块）、`crates/computer-use/src/target.rs`（授权闸接入 + `TargetError` 两变体 + `Scope`/`AppIdentity`/`AppFamily` 派生 Hash + 测试）、`crates/computer-use/src/lib.rs`（一行 `pub mod approval;`）、[computer-use Spec](../spec/crates/computer-use.md)、[安全规格](../spec/security.md)、[design.md](../design.md) 各一段同步、本任务文档与 ROADMAP。policy / tools / app 无代码改动：授权与 Policy / 显式审批的接线随生产原生操作（CU-03+ 探针与工具）落地，故对应 Spec 不变。

契约要点（`pawork_computer_use::approval` 与 `target` 的授权闸）：

- **目标身份**：`TargetIdentity`（serde `tag="kind"` snake_case）两变体——`application`（整应用，bundle id + 族）与 `website`（浏览器内单源网站；构造要求 browser 族，origin 归一化为小写 `http(s)://host[:port]`，host 限域名字符且含字母数字，端口限 1..=65535 并规范化前导零，拒路径 / 查询 / 片段 / userinfo / 空 host / 非法端口，IPv6 字面量 fail-closed）。`TargetIdentity::validate` 在授权入口（`grant / check / spend` 与 `bind_window`）统一复核，公开枚举字段与派生反序列化不能绕过构造校验。授权只覆盖被批准目标：整应用授权不覆盖其内任何网站，单源网站授权不覆盖浏览器与其它源；网站目标在绑定与每次 live 校验时经 `NativeProbe::current_origin` 复核窗口当前 origin（归一化比较），同窗口跨源导航或 origin 不可得 → `SiteChanged`。`bind_window` 改收 `TargetIdentity`，`ValidatedWindow.app` 更名 `target`。
- **审批语义复用**：`GrantKind::{Once, ForRun}` 对应宿主 `ApprovedOnce` / `ApprovedForRun` 决定，拒绝与取消即不签发；授权只能由宿主经 `TargetAuthorizer::grant` 签发，模型侧只能触发检查。自动 resolver 不能满足 Policy AskUser（既有不变量），因此永远产生不了目标授权；不另造审批框架。逐目标持久授权不存在——持久允许仅走已裁决的 ApprovalMode / workspace trust 设置口径（可撤销），授权台账仅存内存，随 run（`revoke_scope`）或 Host 退出消亡。
- **裁决先于 probe**：校验链在 StaleHandle 之后、进程实例 probe 之前插入授权检查——绑定、观测签发（捕获 / 读树）、窗口校验与动作消费（输入）全部先查 `(Scope, TargetIdentity)` 台账授权，未授权 / 已撤销 / 跨目标 → `NotAuthorized`，受保护 bundle → `ForbiddenTarget`；台账授权拒绝零 probe 调用（回归以计数钉住）。网站目标的 `SiteChanged` 裁决须经 `current_origin` 接缝复核窗口当前 origin（不触碰进程实例与捕获后端），同窗口跨源导航或 origin 不可得即拒；宿主捕获前预检 `require_authorized` 同序执行——先查台账、再复核 origin（非消耗），跨源或 origin 不可得时预检即 `SiteChanged`，宿主不得连接 / 捕获 / 读取。证据边界如实收敛：契约层证明台账拒绝先于 probe、网站拒绝先于捕获 / 派发；ReadOnly / 未信任 workspace 的 Policy 拒绝属工具接线语义，随 CU-03+ 生产路径落地后验证——当前「ReadOnly / 未信任零后端访问」的回归证据仅覆盖隔离桌面链路。
- **一次授权的消耗时机**：once 只在其余校验全部通过、动作真正放行时才消耗——绑定需要有效授权但不消耗；陈旧目标（ProcessRestarted 等）或拒绝路径不烧掉单次批准。一次观测或一次输入各为一个动作。
- **撤销与历史**：`revoke` 阻断后续全部 gated 操作（含已签发未消费的在途租约，先于 probe 拒绝）；重新授权后在途租约可放行；已消费动作是历史，撤销不回写。
- **防自我批准**：`MINIMUM_PROTECTED_BUNDLES` 把 Pawork 自身 UI（`dev.pawork.desktop`）与 macOS 权限界面（systempreferences / securityagent / usernotificationcenter / coreservicesuiagent）内置为受保护目标——默认构造即生效、大小写不敏感、宿主只能加不能减（`TargetAuthorizer` 移除派生 `Default`，`new` 永远并上内置集）；受保护目标永不授权、永不绑定，Agent 无法经 Pawork 审批界面或系统权限界面自我批准。契约不存在任何变更 OS Screen Recording / Accessibility / Automation 权限的 API，系统权限缺失由 CU-03+ 探针如实报告，Pawork 授权不蕴含系统权限。

验证：`bash scripts/test.sh computer-use`（31 过：CU-02 既有 16 + CU-16 契约 15，含授权零 probe 访问、once 消耗时机、撤销阻断派发、受保护目标、跨目标 / 跨 scope 拒绝、默认构造内置保护、手工构造身份入口校验、网站跨源导航在捕获前预检 / 观测 / 窗口校验 / 派发四路 `SiteChanged`）；`bash scripts/test.sh tools`（80 过，tools 依赖本包故重跑）；policy（73 过）与 app（278 过，含 computer 审批持久化 / resume 不重执行与 SET-6b 审批回归）无本轮代码影响，沿用前轮结果。日志 /tmp/cu16-fix2-cu.log、/tmp/cu16-fix2-tools.log、/tmp/cu16-fix-policy-tools.log、/tmp/cu16-app.log。无新编译警告（复审轮三条 `unused_doc_comments` 已随 `///` → `//` 消除，仅余第三方 `block v0.1.6` future-incompat 既有提示）。

冻结契约：无需演进——未触碰 GUI wire、domain 事件、持久格式、包布局与依赖边，无新增依赖；`TargetIdentity` serde 形状为包内新契约（无生产消费者），由内联测试钉住。若未来需要逐目标持久授权（跨 Host 重启记住「允许此应用」），须先过 ADR 与 golden，已在上方登记为显式非目标。

审查修复（2026-10-06，6.1 sol 契约评审 ISSUES 轮，四项全部闭环）：

1. **网站授权跨源导航（P1）**：`NativeProbe` 增 `current_origin` 页面身份接缝；`bind_window` 对网站目标钉住当前 origin，每次 live 校验（`validate_window` / `consume_observation` / `consume_element`）复核窗口当前 origin（归一化比较，尾斜杠 / 大小写 / 端口拼写不能绕行），同窗口跨源导航或 origin 不可得一律 `SiteChanged` 先于派发拒绝（fail-closed），导航回已授权源后在途租约恢复可用。回归 `website_target_rejects_same_window_navigation_before_dispatch`。
2. **自我批准防护收紧（P1）**：`MINIMUM_PROTECTED_BUNDLES` 内置 Pawork 自身 UI 与 macOS 权限界面（`com.apple.systempreferences` / `com.apple.securityagent` / `com.apple.usernotificationcenter` / `com.apple.coreservicesuiagent`）；`TargetAuthorizer` 移除派生 `Default`，默认构造与 `new` 永远并上内置集——宿主只能加不能减，大小写不敏感。回归 `default_authorizer_protects_pawork_ui_and_os_permission_surfaces` 与 `default_registry_protects_pawork_ui_and_permission_surfaces`。
3. **文档过度结论收敛（P2）**：security.md、computer-use Spec 与本文件把「零后端访问」结论收敛到契约层证据边界——拒绝先于 probe 由回归钉住；捕获前预检与 ReadOnly / 未信任 workspace 的 Policy 拒绝属宿主 / 工具接线语义，随 CU-03+ 验证；ReadOnly / 未信任零后端证据当前仅覆盖隔离桌面链路。`TargetError` 计数改述十三变体。
4. **origin 校验加强（评审建议）**：host 限 `[a-z0-9.-]` 且含字母数字，端口 u16 且 1..=65535（前导零归一），拒空 host / 多冒号 / IPv6 字面量；`TargetIdentity::validate` 在 `grant / check / spend / bind_window` 入口统一调用，堵住公开枚举字段与派生反序列化的构造绕行。回归 `manually_built_identities_are_validated_at_authorization_entry` 及 origin 用例扩充。

复审修复（2026-10-06，复审 ISSUES 轮，两项全部闭环）：

1. **捕获前预检跨源放行（P1）**：`require_authorized` 此前只查台账授权键——窗口从已授权源导航到异源或 origin 变 None 后预检仍放行，而宿主按「先捕获再登记观测」顺序会在 `begin_observation` 拒绝前已读取屏幕。修法：预检在台账检查之后对网站目标复核窗口当前 origin（非消耗，不烧 once），跨源 / 不可得即 `SiteChanged`，liveness 复核仍归 gated 操作。导航回归扩充：绑定后预检通过、跨源与 None 两路 `SiteChanged`、导航回源预检恢复。
2. **文档口径与编译警告（P2）**：「零 probe」结论限定为台账授权拒绝（NotAuthorized、ForbiddenTarget）——`SiteChanged` 裁决必须调 `current_origin` 接缝（不触碰进程实例与捕获后端），security.md、computer-use Spec、design.md 与本文件同步口径；`MINIMUM_PROTECTED_BUNDLES` 数组元素三条 `///` 改 `//`，`unused_doc_comments` 警告消除，「无新编译警告」恢复为实。

遗留缺口：`NativeProbe` 的 macOS 实现、动作派发与工具 / 宿主接线未做（CU-03+），届时宿主装配只须把审批决定映射为 `GrantKind`（受保护内置集已由 `TargetAuthorizer` 默认生效，无需宿主登记）；OS 权限状态的真实报告随 CU-03 探针落地；CU-09 联动遗留——目标占用、用户接管后的派发暂停与授权撤销的跨 Run 广播未实现，本任务只落地授权失效 / 跨目标不能派发的契约层部分（撤销即时生效于后续 gated 调用），占用冲突与用户接管语义归 CU-09；人工验收即指挥方对审批契约形状与拒绝语义的评审。
