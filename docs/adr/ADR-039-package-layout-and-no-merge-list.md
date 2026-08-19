# ADR-039:V3 包合并布局(37→21)与不合并清单

- **状态**:Accepted(用户 2026-08-19 确认)
- **日期**:2026-08-19

## 背景

V2 收官 39 成员([v2-summary.md](../v2-summary.md)),R0 裁决后 37 成员([ADR-038](ADR-038-inventory-and-product-shape.md))。V3 计划原则要求结构收敛至 **21 成员(19 库 + 2 应用)**([ROADMAP.md](../../ROADMAP.md) §1);终局包清单与来源映射见任务书 [plan/R1](../../plan/R1-package-consolidation.md) §1,结构性判定依据见其 §2。任务书证据经 2026-08-18 波 A 三路只读核查重验,漂移已回写任务书:

- api 消费者实为 **11 个**(原估 ~13),全部同时依赖 domain;api features 已收敛为空锚点 `plugin = []`,src 零门控,`async-trait` 已在用,`ReasoningEffort` 定义在 domain、api 仅重导出。
- diagnostics 对外仅 `Redactor`/`RedactingFmtLayer` 两活符号,唯一消费者 `apps/pawork`;两测试在位可随迁。
- 现有红线断言(desktop deny-list、rmcp 隔离、no_provider_branch)均不点名 api/diagnostics 包名;engine domain-only 断言尚不存在,波 E 建立。

参照:codex-rs 布局纪律(扁平 + 统一前缀 + 集中依赖声明,[references.md](../references.md) §7.1 R1 行)——**只抄纪律不抄粒度**(其 134 成员微 crate 增殖为反面教材,Pawork 方向相反)。

## 决策

### D1 — 目录布局:扁平 `crates/<短名>` + `apps/<name>`

19 个库平铺 `crates/`,目录用去 `pawork-` 前缀的短名(`crates/domain`、`crates/storage`、`crates/control-plane` …);包名保持 `pawork-` 前缀不变——`use` 路径零变更,仅 Cargo.toml 的 path 变化。2 个应用维持 `apps/pawork`、`apps/desktop`(现状即终局)。

- 否决支:保留 13 个功能域目录——21 成员规模下多数域只剩 1–2 包,域目录成纯噪音。
- **执行方式(对任务书 §3「顺势 git mv」字面的修正)**:`git mv` 目录迁移**集中在波 E 一次完成**,与 members glob 切换、design.md §2 重写同步;波 A–D 只做内容级合并(解散包内容平移进幸存包现有目录)。理由:集中迁移每条 path 引用只改一次,避免跨波反复抖动与「两 glob 并存」过渡态;波 A–D 写集合不含根 Cargo.toml,收口边界干净。`git log --follow` 抽查在波 E 收口执行。

### D2 — 不合并清单(固化,以下保持独立包)

`policy`、`exec`、`auth`、`git`、`engine`、`protocol`、`testkit`、`transport`、`orchestration`、`workflow`。

结构性理由:

- **policy**:tools→workspace→policy 依赖链存在,并入任何含 tools 的包即成环;`PolicyDecision`/`ApprovalMode` 是冻结契约与安全红线回归锚。
- **exec**:零内部依赖自含,5 个消费者,平台执行面独立演进(R7 沙箱)。
- **auth**:Secret 审计边界;mcp/app 两个异质消费者。
- **engine**:纯执行核,R1 收口后只依赖 domain,波 E 建立依赖断言。
- **protocol**:冻结帧契约 + typegen 链载体。
- **testkit**:dev-only,不进生产二进制闭包。
- **git / transport / orchestration / workflow**:各自独立消费面与 feature 组合(orchestration 的 `git` feature、transport 的 `protocol` feature),并入即拖泥带水。

### D3 — api→domain 的 GUI 侧口径

api 并入 domain 后,desktop 的编译闭包(经 client→domain)将包含 provider/tool 契约**类型**。**不违反「GUI 不加载 Core」红线**:红线指 GUI 进程不在运行时装配/执行 Core 服务、不直接访问 Provider/数据库/工具(运行语义);纯类型出现在编译闭包不构成运行时加载,且 domain 纯净红线(不依赖 GUI/SQLite/HTTP/Keychain/Git/具体 Provider)对迁入类型继续成立(依赖树可断言)。serde 形状(tag=`type`/content=`data`/snake_case)逐字节不变。

### D4 — 合并映射与解散清单

终局 21 包与来源映射以任务书 §1 表为准。解散 16 包:`pawork-api`、`pawork-net`、`pawork-provider-core`、`pawork-sqlite`、`pawork-session`、`pawork-blob-store`、`pawork-resources`、`pawork-config`、`pawork-compat`、`pawork-mcp`、`pawork-quota`、`pawork-provider-control`、`pawork-gui-server`、`pawork-channels`、`pawork-sdk`、`pawork-diagnostics`,外加 `apps/protocol-probe`(转 client 测试/example)。波 A 执行其中 api→domain 与 diagnostics→apps/pawork 两项;解散性质为**平移**(git 历史 + tag `v2-final` 兜底),非删除语义。

### D5 — 编译粒度代价(明示取舍)

providers/storage 单体化后,「改一个 adapter 重编整包」。接受该代价,换依赖治理、feature 收敛与纪律简化。**实测数据补录**:波 B/C 收口时以 touch-单文件 + `cargo check -p` 计时(合并前基线 vs 合并后),结果追加到本 ADR「落实记录」。

### D6 — 合并后包内纪律(原跨包纪律降级为模块纪律的清单)

- provider-core 不依赖 net → providers 包内模块纪律 + `core_modules_do_not_reference_net_module` 定向源扫描。
- rmcp 隔离断言 `public_sources_do_not_mention_rmcp` 随迁为 tools 包 `mcp/` 模块级测试。
- storage feature 分层:`sqlite` 基座常开(`pawork-storage` 中不设同名 feature),`session`/`blob` default-on;control-plane 以 `default-features = false` 只取 Actor 面(2026-08-19 落实修正:经核查 control-plane 对 sqlite Actor 零引用,死依赖直接移除,见波 B 落实记录)。
- engine domain-only、desktop client-only 断言、`cargo tree` 无环与闭包断言:波 E 建立/更新。

### D7 — 契约保护(golden 先行)

- 波 A 前置:`ProviderStreamEvent` 13 变体、`ProviderError`、`CanonicalModelRequest`、`ToolResult` 目前仅有内存 roundtrip、**无检入字节级 golden**;先在 `pawork-api` 补 fixture 跑绿,再整组平移 domain,迁移前后 JSON 零 diff。`ResolvedCredential` 无 serde(仅 Debug 脱敏测),随迁。
- 迁入 domain 的类型**不加** ts-rs derive(不进 typegen 导出集);`pawork-domain/typegen` feature 链与 `schemas/` 三产物保持。
- 全阶段 serde/磁盘/线上形状零 diff;合并不裁剪(整组平移,字段可闲置);各冻结契约 golden(信封/帧/DDL/PWB1/config/audit/MCP)随模块平移并保绿。

## 后果

- 波 A 后 members 37→35(撤 api、diagnostics);波 B/C/D 继续合并;波 E 定稿 21 并完成目录迁移、design.md §2 重写、README 与 AGENTS.md 成员数回写。
- 每波 `cargo tree` 无环 + `-p pawork` 闭包只减不增;Cargo.lock 逐波刷新。
- 微包间编译器强制纪律下沉为模块纪律 + 定向测试(D6),强制力减弱——为收敛 21 成员目标接受的代价。
- 目录迁移集中波 E 后,波 A–D 期间「包位置 ≠ 终局位置」为已知过渡态,文档(design.md §2)以 V2 实态为准直至波 E 重写。

## 落实记录

### 波 A(2026-08-19,api→domain + diagnostics 撤包)

- **golden 先行(D7)**:`ProviderStreamEvent` 13 变体 / `ProviderError` / `CanonicalModelRequest` / `ToolResult` 字节级夹具先于迁移在 `pawork-api` 建立并跑绿,随后随类型整组平移至 `foundation/domain/tests/{contract_golden.rs,fixtures/}`;迁移前后 JSON 逐字节零 diff(迁移后同 fixture 全绿)。
- **api→domain**:lib.rs(951 行)+ tool.rs(254 行)→ `provider_api`/`tool_api` 两模块,domain 根 re-export 保持原符号路径(`pawork_domain::CanonicalModelRequest` 等);domain 增 `thiserror` 依赖与 `plugin = []` F41 复活锚(ADR-038 D14 语义随迁);未给迁入类型加 ts-rs derive,`pawork-domain/typegen` 链不变。11 个消费者(74 文件)`use`/Cargo.toml 切至 `pawork-domain`(net 的 `http` feature 只留 `dep:pawork-domain`);`pawork_api_key*` 环境变量名不受影响。`foundation/api` 撤包。
- **diagnostics 撤包**:`Redactor`/`RedactingFmtLayer` 与两测试迁 `apps/pawork/src/redact.rs`(宿主增 `regex` 依赖);`foundation/diagnostics` 删除。
- **验证**:`cargo test -p` domain(含迁移后 golden 4+信封 golden 3)/cli(100)/mcp(59,含 rmcp 隔离断言)/quota/auth/engine/testkit/app/tools/providers/provider-core/net/pawork(bin,脱敏 2)全绿;protocol 含 typegen 全绿;desktop 与 protocol-probe `cargo check` 绿;members 37→35;`cargo tree -p pawork` 闭包 817→800 行,无环、无 api/diagnostics。
- **遗留**:D5 编译粒度实测数据待波 B/C(providers/storage 单体化)收口补录;design.md §2、README、AGENTS.md 成员数回写在波 E。

### 波 B(2026-08-19,storage / providers / workspace 三大合并)

- **storage**:`foundation/sqlite` 改名 `pawork-storage`,吸收 `storage/session`→`session/`、`storage/blob`→`blob/`;D6 feature 分层落地:`default = ["session","blob"]`,`compaction`/`checkpoint`/`protected` 保持 opt-in,sqlite 基座常开。control-plane 的 `pawork-sqlite` 经核查为死依赖(零源码引用,`SqliteUsageLedger` 自开 rusqlite 连接),prod optional 与 dev 两处一并移除——「只取 Actor 面」以「不依赖」达成,`sqlite = ["dep:rusqlite"]` feature 保留。
- **providers**:`providers/adapters` 吸收 `net/net`→`net/`(http/sse/retry 包内常开)、`providers/core`→`registry/pricing/usage/negotiate/reasoning/error` 六根模块;通道内聚 `channels/`(anthropic/、chatgpt、xai、api_key),cfg feature 门控与 `required-features` 集成测试原样;adapters 原 `usage.rs` 薄 shim 与 core `usage` 合一。D6「core 不依赖 net」降级为模块纪律 + 源扫描定向测试(`core_modules_do_not_reference_net_module`)。host 通道表本体(`host/app/src/channels.rs`)未迁入 providers,仅随装配缝替换其中 config 的 use 路径。
- **workspace**:`workspace/core` 吸收 `resources/`、`config/`、compat→`import/`(五来源 fixtures 随迁);mcp 改 `pawork_workspace::config`,tools 零改动。
- **host/app 装配缝**:主代理串行收口,7 个解散包依赖收敛为 `pawork-storage`(features compaction+checkpoint)/`pawork-workspace`/`pawork-providers` 三个方向,use 路径 12 文件机械替换。
- **根 Cargo.toml 最小触碰(对 D1 字面的偏离记录)**:`net/*`、`storage/*` 两 glob 在成员清空后被 Cargo 拒绝(空 glob 报错),先行移除这两条 glob 并注记;终局 members 定稿与目录迁移仍集中波 E。空目录 `net/`、`storage/` 已删。
- **验证**:storage 默认 86+5 绿,全 feature 133 + PWB1 golden 4(`pwb1_valid_hex_*`、`seal_for_test_matches_pwb1_golden_hex` 实际运行)+ read_range 5 绿,`--no-default-features` check 绿;providers 默认 134+8+16、全通道 140+ 各集成测试绿,模块纪律测试在位;workspace 112+12+14 绿(config 47 测含 `six_layer_default_model_matrix_and_profile_provenance`,compat 五来源 smoke 绿);app 93、mcp 64、tools 65、cli 30、client 6+7、control-plane 69、domain 44+contract_golden 4+events_golden 3、desktop 34(当时测试名 `desktop_direct_deps_stay_on_client_deny_list`)全绿;protocol-probe 9 场景绿;28 成员全体 `cargo check` 绿。
- **members 35→28**;`cargo tree -p pawork` 闭包 800→751 行;无环;解散七包名在闭包与 Cargo.lock 零残留。
- **D5 实测补录**:providers 包 touch-单文件增量 `cargo check -p`——合并前(HEAD 基线 worktree,独立 target)≈0.14–0.16s,合并后 ≈3.7–4.7s。编译粒度代价成立但绝对值仍在秒级,维持 D5 取舍。
- **偏差/遗留**:①三路验证期间因根 workspace 暂不可解析,子代理在 /tmp 隔离拷贝验证,收口后全部由主代理在仓库根复跑为上方结果;②protocol-probe `snapshot-reconnect` 一次批量下偶发 10s 帧超时(单跑 3/3 绿,纯路径改名不涉该链路),判定既有偶发,已登记 ROADMAP §4 待 R9 复跑;③design.md §2、README、v2-summary §4(信封 golden 位置表述)、gui-design.md、desktop deny-list 包名、AGENTS.md 成员数 —— 波 E 统一回写。

### 波 C(2026-08-19,mcp→tools ∥ quota+provider-control→control-plane)

- **tools(+mcp)**:mcp 八模块 + crate 根(McpError/McpPeer/re-export)整组平移为 `execution/tools/src/mcp/`(mod.rs + 八文件),64 测零裁剪;rmcp 隔离断言随迁为模块级测试(扫描根 `src/mcp`,仍只豁免 codec.rs);tools 依赖并集增 auth/reqwest/rmcp(=2.2.0 原 feature)/url/tracing,dev 增 wiremock/rmcp server/tokio test-util。F05 语义(SecretRef `pawork.mcp.*`、stdio env_clear、拒 `PAWORK_API_KEY_*`)随模块原样平移;`mcp-auth.json` 装配仍在 host/app。
- **control-plane(+quota+provider-control)**:quota 六文件平移为 `src/quota/`(100 测),provider-control 两文件平移为 `src/credential/`(活树实测 35 测;冻结形状 LeaseState 持久化字符串/LeaseEvent tag=kind/CredentialLease schema_version=2/禁 secret 字段不动,lease 状态机虽仅包内活按零裁剪整组保留);core 增 futures/url 与 dev proptest,`default=["sqlite"]` 原样。
- **下游收口**:orchestration 由 worker 切换(`pawork-control-plane`,保持 `default-features = false`,`cargo tree -p pawork-orchestration` 无 rusqlite);host/app 装配缝由主代理串行收口(删 mcp/quota/provider-control 三依赖行,use 切 `pawork_tools::mcp::` / `pawork_control_plane::{quota,credential}::`)。
- **撤包**:extensions/mcp、control-plane/quota、control-plane/provider-control 三源目录删除(平移非删除语义);根 `extensions/*` 空 glob 移除(波 B 先例);**members 28→25**。
- **验证**:tools 129(65+64)、control-plane 204(69+100+35)、app 93、cli 30、pawork 2、orchestration 85 全绿;desktop `cargo check` 绿(deny-list 未点名三包,desktop 依赖不变);audit JSONL golden 与 usage dedup_key 四锚定测试随迁面零触碰、保持绿;`cargo tree -p pawork` 闭包 751→724 行、无环、三包名在闭包与 Cargo.lock 零残留。
- **D5 实测补录**:touch-单文件增量 `cargo check -p`——tools 合并前 ≈10.4s、合并后 ≈11.5s;control-plane 合并前 ≈8.5s、合并后 ≈5.7s(后者在噪声内)。编译粒度代价成立但绝对值仍秒级,维持 D5 取舍。

### 波 D(2026-08-19,gui-server→app ∥ channels→cli ∥ sdk→client + probe→client)

- **app(+gui-server)**:GuiHost trait 随包平移为 `pawork_app::gui_server::GuiHost`(GuiServer/ConnectionManager/EventHub 同迁);app 终态依赖 = 原依赖 + transport(吸收 gui-server 获得);cli 四处 `use pawork_gui_server::GuiHost`(chat/gui/headless/adapter)改经 app。
- **cli(+channels)**:`acp/{adapter,command_host,host,wire}` 四公开模块 + `pub(crate) map.rs` 平移为 `pawork_cli::channels`;ACP golden fixtures 随迁;cli 顶层子命令实态 **21** 个(任务书「14」过时表述已回写)。
- **client(+sdk、probe)**:sdk 稳定面(PaworkClient/PaworkOptions/EventSubscription/SdkError{,Kind}/Transport/spawn_pawork/SDK_API_VERSION/SDK_VERSION + experimental::CompatOutcome + protocol reexport)平移为 `pawork_client::headless` 并保持 `pub`,夹具(5 件 + client_tests 20 测 + spawn_e2e 3 测)随迁;probe 9 场景(MemoryTransport harness 形态保留)→ client `tests/`,live 模式(`--connect`/`--live-two-gui`/`--live-pty`)→ client `examples/probe.rs`。
- **撤包**:host/gui-server、host/channels、clients/sdk、apps/protocol-probe 四源目录删除;**members 25→21**。
- **契约面**:GUI 帧/headless-json 字节 golden 在 protocol(本波不动);typegen 链输入集不变;行为锚点(EventHub Snapshot/ReplayUnavailable、F33 fail-closed、ACP golden)随迁保绿;UDS 0600/token 锚点在 transport+protocol(写入集外,不动);cli `gui.rs` 强制 token 接线原样保留。
- **验证**:被合并包与下游 `cargo check/test -p` 全绿;`cargo tree -p pawork` 闭包只减不增、无环;四解散包名在闭包与 Cargo.lock 零残留。

### 波 E(2026-08-19,收口:members 定稿 21 + 扁平目录迁移)

- **目录迁移(D1 落地)**:19 库 `git mv` 至扁平 `crates/<短名>`——domain/protocol/storage/testkit/providers/auth/workspace/policy/exec/tools/git/engine/workflow/orchestration/control-plane/app/transport/cli/client;apps/{pawork,desktop} 不动;八个空域目录(foundation/providers/net/storage/workspace/execution/control-plane/host/clients/extensions/agents 中已清空者)删除;`cargo metadata` 确认 members = 21(19 库 + 2 应用)。
- **path 依赖改写**:crates 内互依 `../../<域>/<x>` → `../<短名>`,apps 两包 → `../../crates/<短名>`;**use 路径零变更**(包名不变,import 不动);根 Cargo.toml members 定稿 `crates/*` + `apps/*`,历史 glob 注释清理。
- **红线断言随迁**:desktop 业务生产依赖仅 `pawork-client`(`apps/desktop/src/platform.rs`);新建 `crates/engine/tests/domain_only.rs`(engine 生产依赖仅 pawork-domain);providers core→net 源扫描与 rmcp 隔离断言在位。整阶段审查进一步覆盖 target-specific dependency 表与 `package` alias，避免有限 deny-list 漏检。
- **文档回写**:design.md(头部说明 + §2 整节重写为 21 包表 + §3.1/§3.2/§5 G6 包名)、README(R0/R1 状态 + `crates/` 结构树)、AGENTS.md(成员数与布局表述)、v2-summary.md(信封 golden 在 pawork-domain)、gui-design.md(三处包名)、task-guide.md(两处 `host/` → `crates/app`)、ROADMAP(本行)。
- **验证**:全 21 包 `cargo check` 绿;73 测试二进制 1644 测绿(`--no-fail-fast` 全量 + 定向复跑);`cargo tree -p pawork` 闭包 724→711 行、无环、16 解散包名在闭包与 Cargo.lock 零残留;engine 生产依赖仅 domain。
- **既有缺陷修复 ×2**(收口定向测试暴露,按 task-guide §1 窄任务,ROADMAP §4 登记项销账):① client_tests `hello_ack.json` 夹具 negotiated 1.1→1.2(S13 升 API_VERSION 未更夹具);② workflow `review_flow_replays_identically` 测试侧改为携现有步骤修订(revise 空 steps 语义漂移,基线可复现)。两者均测试/夹具侧,不动生产形状。
- **真实冒烟**(deepseek/deepseek-v4-flash):chat 流式 ✓、read_file 真实执行 ✓、untrusted 工作区 fail-closed 拒 run_command ✓、always-ask 审批闸门真实弹出且超时 fail-closed ✓、never-ask 下 run_command 真实执行 ✓;`gui serve` 启动/握手/snapshot/create_session/RunStart 处理链路 ✓。usage 幂等键 warn(ROADMAP §4 登记项)复现,非回退。
- **desktop `--probe-smoke` 按波 E 实态登记**:当时确定性失败于首发 send_message,临时插桩定位为两个**既有缺陷**(非本波回退;client/gui_host/app lib/desktop main/providers registry 四行为文件与 HEAD 逐字节一致):① ModelList(运行期探测合并,`crates/app/src/lib.rs` models_overview)与 switch_provider(静态注册表)目录不对称——目录通告 glm-4.7(glm-coding 实探返回)而静态注册表仅 glm-5.2,切换报 UnknownModel;② client `FrameWant::Event` 匹配 `ServerFrame::Error`(S7 波 C 5aa9230 引入),desktop 事件泵常驻 recv 抢走命令错误帧并误判 Disconnected,等待方 10s 超时误报。两项当时登记 ROADMAP §4,后由 R1 整阶段审查修复(下一条)。
- **R1 整阶段审查与修复(2026-08-19)**:① app 新增按明确 `(provider, model)` 解析的 `resolve_provider_model`,静态目录未命中目标 provider 时只向该 provider 探测并惰性合并,仍 fail-closed;`switch_provider_accepts_runtime_discovered_model` 回归在位。② client 的 Response/Snapshot/Resume 错误按 request_id 归属,Event 只接 `request_id=None` 的连接级错误;`frame_wants_route_errors_by_request_id` 回归在位。③ desktop/engine 生产依赖红线改为 allow-only,覆盖普通/target-specific 表与 inline/nested `package` alias;providers core→net 扫描收紧为 `net` 标识符零引用。全 21 包 check/test、storage/providers feature 组合、711 行 pawork 闭包与解散包零残留均通过;隔离 instance 的 `pawork-desktop --probe-smoke` 实测 glm-4.7 首轮完成、切换 deepseek-v4-flash 后第二轮完成、取消/持久化/断线存活通过。
- **D5 实测**:目录纯移动不改变编译单元粒度,无新增 D5 代价;21 包全量 `cargo check` 在默认 target 增量缓存下秒级完成。

## 相关

- [plan/R1-package-consolidation.md](../../plan/R1-package-consolidation.md)(任务书:目标包清单、波次拆分、退出标准)
- [ROADMAP.md](../../ROADMAP.md) §2 R1 行
- [ADR-038](ADR-038-inventory-and-product-shape.md)(R0 裁决;D8 已预告 RedactingFmtLayer/Redactor 迁宿主)
- [v2-summary.md](../v2-summary.md) §4(冻结契约)、§5(S13 拍板)
- [references.md](../references.md) §7.1 R1 行(codex-rs 布局纪律)
