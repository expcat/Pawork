# R3 — 协议与投影同源化(T3 + T5)

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R3 行。根因:S7 GUI、S10 headless+ACP 三条客户端通道各自生长,同一 `AppCommand` 三套 mapping、三套能力/授权模型;Timeline 投影语义由 host、desktop、gui-client 三处手搓。本阶段把「宣告 = 授权 = 实现」收敛到单一事实源,并把投影 reducer 下沉共享。收益/风险比最高的破坏式重设计(协议 wire 形状不变,变的是实现组织)。

## 1. 现状证据(执行时重验)

- **三套命令映射**:GUI 帧巨 match(`host/app/src/gui_host.rs`,2,594 行主体)、headless `command_capability` 手写表 + `foundation/protocol/headless/{json_mapping,translate,wire}`(F33 修成 fail-closed 但仍手写)、ACP `host/channels/acp/{map,wire,adapter}`(R1 后位于 `pawork-cli` `channels/acp/`)。
- **宣告与实现不同源**:K-08 双端谎报 ArtifactStreaming(R0 已停止宣告,根治在本阶段);F52 探针 scheme 漂移同因。
- **三处投影**:host `timeline()`、desktop `projection.rs`(2,346 行,含 907 行测试)、gui-client resume stash;F12 把锚点从 index 补成 `event_id` 属点修;S12-CR08-08 的 live/history 不一致仍在。
- **审批档位谎言**:`OnFailure`「当前等价 NeverAsk」靠三处同步注释收窄(`execution/policy/src/mode.rs:18`、`engine.rs:79`、compat `parse.rs:932`,S13-F16)。

## 2. 目标设计

1. **Command/Capability Registry**(protocol 内新模块):每个 `AppCommand`/`AppQuery` 一条登记——wire 名、所需 capability、授权域(GUI/headless/ACP 可用性)、幂等性质、协议版本引入点。三通道的 dispatch、能力宣告、授权检查全部从 registry 派生;未登记命令天然 fail-closed(F33 语义从「手写表」变「结构保证」)。
2. **投影 reducer 下沉**:`pawork-protocol::projection` 纯模块(输入 Snapshot/TimelinePage/AppEvent,输出可渲染状态),host 与 desktop 同源消费;desktop `projection.rs` 瘦身为渲染适配;投影 golden(去重/锚点/分页交错/live-history 一致)一套喂两端。gui-client 的 resume stash 逻辑并入。
3. **OnFailure 裁决**:二选一——实现真实「失败时回问」语义,或删除枚举变体(compat 导入映射到 NeverAsk 并记 Diagnostic)。推荐删除(V2 从未实现,产品面无人依赖);wire 兼容:serde 反序列化保留别名接受旧值。
4. 协议版本:registry 与投影模块不改帧字节;若 OnFailure 删除触及 `ApprovalMode` serde,按 minor 版本 + golden 先行(该枚举在 policy,属冻结契约——先确认 wire 是否暴露,若暴露则走「接受旧值、不再产出」的兼容路径)。

## 3. 波次拆分

> 波 A 实态复核记录(2026-08-20,三路只读核查):
> 1. 「17+9 条帧 golden」实为 25 帧 + 1 类型共 26 条,全部 GUI 面(crates/protocol/tests/golden.rs 驱动);headless 侧无逐帧字节 golden,唯一防线为 tests/fixtures/headless/ 16 案例——波 B 切 headless 前须注意。
> 2. GUI capability 宣告向量无任何检入快照基线;「registry 派生宣告 vs V2 快照零 diff」须先在波 A 建立 golden(现手写向量:server 端 cli/gui.rs {Events,Snapshots,TerminalStreaming,Approvals})。
> 3. gui_host.rs 内部无 capability/授权门;GUI 授权面实际分布于 protocol handshake.rs(过滤)、cli/gui.rs(装配 supported 集)、app/gui_server(connection/session 存 granted 集)。波 A 授权门落点在 gui_server 层。
> 4. 命令全集:AppCommand 19 变体(protocol/src/app/command.rs:295)、AppQuery 11 变体(app/query.rs:30);gui_host 仅实现 10 command(其余落 unsupported fallback);headless 手写表在 crates/cli/src/headless.rs(非 protocol/headless);ACP 白名单在 cli/src/channels/acp/adapter.rs:478。

> 波 B 实态记录(2026-08-20,三路核查 + 两路实现 + 一路审查):
> 1. 写入集实缩:protocol headless/(json_mapping/translate/wire)无命令映射,零改动;client tests(probe/fixtures)只跑不改;实际写入仅 crates/cli/src/headless.rs 与 crates/cli/src/channels/acp/adapter.rs。
> 2. ACP「method 白名单」语义裁决:decode_payload 的 session/* 四臂属 ACP 协议路由(解析),保留;准入决策改经 registry acp 列(admit_acp_command 作用于 Command 产物);session/cancel、$/cancel_request、initialize、session/load 显式拒绝与 catch-all 属协议事实,逐字保留;session/resume→Reattach、session/close→Disconnect 为连接生命周期请求,不经 command 门;registry acp: true 恰为 {session_create,run_start,run_cancel,tool_approve},与现行 ACP 可达面一致。
> 3. C2 缺口已补:HOST_CAPABILITIES 快照钉死 + registry headless 列 ⊆ HOST_CAPABILITIES 一致性测试(crates/cli/src/headless.rs)。
> 4. 审查 verdict=pass;两条低阶观察登记:admit_acp_command 拒绝分支现行 decode 路径不可达(fail-closed 防护网,单测直接覆盖);command_entry 对表缺失 panic(fail-fast,穷尽 match + 完整性测试钉死,不构成 fail-closed 变松)。

> 波 C 实态复核记录(2026-08-20,三路只读核查 C1/C2/C3):
> 1. host 锚点实态为**纯 sequence 游标**(`after: Option<u64>`,gui_host.rs:685-705),event_id 仅是 TimelineItem 载荷字段;「锚点 event_id/sequence」按此现实设计。host 侧**无去重逻辑**,去重/交错回填全在 desktop(`seen: BTreeSet<u64>` + partition_point 有序插入,projection.rs:330/980-998)。
> 2. host 映射 `project_timeline_item`(gui_host.rs:1389-1500)是类型化 AgentEvent match;desktop 历史臂 `merge_history_item`(projection.rs:749 起)用 snake_case 字符串匹配 TimelineItemKind 并经 `HistoryItem` 手工解构 JSON(因 TimelineItem 未从 pawork-client re-export,projection.rs:296-307 注释)——两侧不同源即 CR08-08 结构性根因。tool 锚点 live 按 run+tool_call_id、历史按 run+tool_name(TimelineItem 不带 tool_call_id,projection.rs:278-287),下沉 reducer 保留双键策略并 golden 钉死。
> 3. gui_server/session.rs:438-442 对 SessionGet 先调一次 `host.timeline()` 但 `let _ =` 丢弃结果(S7 wave A 遗留),随后 `host.query()` 内部再算一次(:766)——带分页参数的 SessionGet 实际执行 timeline 两次,本波随切换一并清理。
> 4. desktop 依赖红线:生产 pawork 依赖恰为 {pawork-client}(apps/desktop/src/platform.rs:146/157 断言 + gui-design.md:143 deny list);projection 类型必须经 pawork-client re-export 流入,禁止给 desktop 新增 pawork-protocol 直依赖。
> 5. wire 面:TimelinePage 经 `AppResponse::Data["timeline_page"]` raw Value 内嵌(gui_host.rs:746-767),无 ServerFrame 级字节 golden;本波**不改承载方式**(raw Value 保持),故 26 条帧 golden 与 schemas/ 零 diff 约束不变。storage `projection_snapshot`(crates/storage/src/session/projection.rs)是另一套词汇,本波不触碰。
> 6. 测试缺口(改前必须先有 golden):① 同一事件序列 host/desktop 两端对拍 golden 不存在;② fork 分支切换后按 lineage 取数的投影语义两端均无测试;③ resume 三态 × timeline 基线切换组合 host 侧无对拍。既有防线:desktop 去重/交错/三态测试 8 条(projection.rs:1581-2321)随迁 protocol;probe 9 场景不消费 timeline,只跑不改。
> 7. 影响面:crates/app/src/lib.rs:84 re-export 随迁;client stash/FrameWant(lib.rs:745-852)属帧路由不并入,并入的是 resume disposition→基线切换语义;controller.rs:844-849 保持唯一 timeline_page 解码点。

> 波 C 收口记录(2026-08-20,单实现 + 一路审查):
> 1. 落地:protocol 新增 `projection/`(805 行:`project_event` 自 gui_host 逐字平移、`TimelineProjection` 合并核 seen/有序插入/双键 tool 锚、resume 基线语义);host 删本地映射改 re-export 保名 + 删 gui_server/session.rs 丢弃结果的重复 `timeline()` 预调用(测试钉死恰好执行一次);client 仅追加 re-export(45-48);desktop projection.rs 2346→1542 行,时间线语义全迁出、只剩渲染适配(审查逐段核实无 reducer 残留)。
> 2. golden:新增 projection_golden(三种子:分页交错/Lagged→Snapshot/fork 切换)+ projection_semantics(desktop 8 条语义随迁 + live/history 文案对拍)+ app timeline_projection_host 对拍(真 SessionStore→timeline()→fixture);对拍口径 sequence/kind/timestamp/run_id,event_id 分属 wire/持久化标识域不比对。
> 3. 行为修正(CR08-08 根治本体,已钉死):live `RunChanged(Created)` 文案统一 `run started`;run/diagnostic 条目由 append 改 partition_point 有序插入(乱序到达不再错位)。
> 4. 验证:五包定向 cargo test 全绿(protocol 133 / app 118 / cli 74 / client 43 含 probe 9/9 / desktop 27 含依赖红线);26 帧 golden、events_golden、schemas/、probe fixtures 零 diff;protocol 零新增依赖边;真实冒烟 `pawork-desktop --instance r3c --probe-smoke` 通过(glm-4.7 首轮 completed→切 deepseek-v4-flash 次轮 switched,assistant_turns=3,cancelled=1,persisted=14,disconnect_survive=running,EXIT=0)。
> 5. 偏差与登记:① desktop projection.rs 1542 行未达 <800 目标——剩余为 UI 态/渲染分组/渲染测试,审查确认无 reducer 语义残留,继续压行需拆文件或丢测试,超出本波写入集,行数目标偏差登记在此;② 当时保留的 ToolCompleted seen 前置与 assistant 跨臂锚点怪癖已在下方 R3 整阶段审计中修复并补回归;③ probe snapshot-reconnect 既有 flake(ROADMAP §4 已登记)首跑复现一次,重跑与全量均 9/9。

> 波 D 收口记录(2026-08-20,单实现 + 一路审查 + 主代理三通道真实冒烟):
> 1. 落地:ApprovalMode::OnFailure 变体删除,NeverAsk 加 #[serde(alias = "on_failure")] 实现「接受旧值、不再产出」;compat 导入 codex "on-failure" 与 claude "acceptEdits" 映射 NeverAsk 并挂 CompatIssue::warning("approval_on_failure_mapped")(decision 保持 Ask + requires_review,绝不放宽);app/cli 解析续收 on-failure/on_failure 两种拼写 → NeverAsk(与旧 OnFailure 引擎行为逐字节等价);S13-F16 三处收窄注释(policy/mode.rs、policy/engine.rs、workspace import/parse.rs)全部清除;CLI help 与 unknown 错误文案不再宣告 on-failure 档位。
> 2. 实态漂移回写:写入集自任务书「policy(mode)、engine、workspace(import 映射)、protocol registry」修正为 policy/workspace/app/cli 六文件——protocol registry 零触碰(ArtifactStreaming 维持候选:无产品接线决议,K-08 已由 R0 停止宣告 + 波 A registry 数据编码闭环);crates/app/src/approval.rs 与 crates/cli/src/lib.rs 为变体删除的强制消费点(2026-08-18 快照未列);任务书「engine」实指 crates/policy/src/engine.rs(PolicyEngine)。
> 3. wire 暴露裁决:ApprovalMode 不在 protocol 帧/schemas/*.d.ts/事件信封/DDL;唯一 serde 面为 import 载荷 JSON(CompatPayload::PermissionRule.approval_mode)+ CLI 字符串,故按任务书走「接受旧值、不再产出」兼容路径,无需协议 minor 版本;旧 import 报告含 on_failure 仍可反序列化,新产出一律 never_ask。
> 4. 验证:cargo test 四包定向全绿(policy 62 / workspace 113+13+14 / app 100+6+11+1 / cli 35+16+23);protocol 26 帧 golden 与 domain events_golden 零 diff(protocol/domain 未触碰,cargo tree 确认 protocol 不依赖 policy);cargo check -p pawork 通过;审查 verdict=pass,低阶观察(五值序列化逐字节钉死)同波补测闭环——serializes_snake_case 扩为全变体断言。
> 5. 真实冒烟(阶段退出标准,auth 文件凭证,ROADMAP §1.1 低消耗矩阵):GUI desktop --probe-smoke 隔离实例 r3d 通过(first=glm-4.7 first_turn=completed → second=deepseek-v4-flash second_turn=switched,assistant_turns=2,cancelled=1,persisted=15,disconnect_survive=running);headless --json-stdio 一轮通过(hello→session_create→run_start→run_changed completed,assistant_delta "pong");ACP initialize/session/new/session/prompt 通过(protocolVersion=1,stopReason=end_turn,agent_message_chunk "pong")。

> R3 整阶段审计修复记录(2026-08-20~21,波 A–D 全量复核 + 四路 `xai/grok-4.6` 分域只读审计 + 一路最终复核):
> 1. Registry/GUI:registry 原将生产 `GuiHostAdapter` 未实现的 8 command + 4 query 标为 GUI 可用,现按真实 host 面改为 unavailable,拒绝发生在进入 host 前;`Events`/`Snapshots`/`TerminalStreaming`/`ArtifactStreaming` 从只宣告未完整授权改为覆盖首帧 Snapshot、Subscribe/Unsubscribe、SnapshotRequest、Resume replay/fallback、live/replay terminal 过滤及 ArtifactRead fail-closed;client 未获 Snapshots 时不再等待不存在的首帧,Subscribe/Unsubscribe 以同连接 Heartbeat 作有序屏障并消费 request-scoped 错误,无 Events 时拒绝不会污染后续收帧;所有 snapshot 路径在未获 TerminalStreaming 时裁掉 TerminalSessions section。
> 2. Headless/ACP:对照波 B diff、registry 列、HOST_CAPABILITIES、decode/admit/host dispatch 与 fixtures 未发现缺陷,保持零代码改动。
> 3. Timeline host:limit=0 归一为 1;分页 complete/next_sequence 改按原始持久化 envelopes 计算,不可投影事件组成的空展示页仍推进游标,不再提前截断历史。
> 4. Projection reducer:committed assistant 改为移除后按新 sequence 重插,同 run 多轮 assistant 在 committed 后清锚;live message_id 锚点可被后到 committed 权威全文替换并保留 tombstone 吞掉迟到同-message delta,较旧历史页不再清掉较新 live 锚点;ToolCompleted/live ToolOutput 均 seen 前置;工具历史身份用既有 `detail` 字符串承载内部 `tool_call_id` 上下文(reducer 剥离后再展示,不新增冻结 wire 字段),并发工具 output/completed 精确回填,旧历史仅在唯一候选时兼容;历史先到、live start 重放时补齐锚点;完成后释放锚点。三种子 fixture 的 `item.detail` 期望随内部上下文更新,帧/schema 形状不变。
> 5. OnFailure:实现与兼容映射无缺陷;仅修正 `docs/design.md` 残留的“六档”口径为五档并注明旧 alias。
> 6. 验证:`cargo test -p pawork-protocol`、`cargo test -p pawork-app`、`cargo test -p pawork-cli -p pawork-client -p pawork-desktop`、`cargo test -p pawork-policy -p pawork-workspace` 与 `cargo check -p pawork` 全绿;新增回归覆盖 GUI 空能力无泄漏、terminal live/snapshot capability、无 Snapshot 握手、订阅拒绝后 heartbeat、不可投影页游标、assistant 排序/多轮/live-history 对账、并发工具身份与重复 live output。联网 app smoke 保持既有 ignored,本轮未重跑凭证型真实冒烟;R3 状态保持 🟢,未进入 R4。

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| A | Registry 设计 + protocol 内落地(表驱动或 const fn;含 17+9 条帧 golden 复核);GUI 通道切换到 registry 派生(gui_host dispatch 改造第一步:宣告/授权走 registry,巨 match 拆解留给 R4) | crates/protocol、crates/app(gui_host + gui_server 授权面)、crates/cli/src/gui.rs(仅此文件:GUI 宣告装配点,2026-08-20 实态修正) | 串行(契约面) |
| B | headless 与 ACP 切换 registry;删除 `command_capability` 手写表与 ACP method 白名单;probe 场景同步 | protocol headless/、cli(headless+acp)、client tests(probe) | 并行 ×2(headless / acp) |
| C | 投影 reducer 下沉 + 投影 golden;host `timeline()` 与 desktop `projection.rs` 切换消费;live/history 一致性用 golden 钉死(CR08-08 根治) | protocol `projection/`、host/app timeline、apps/desktop projection、clients/client | 串行(单一 owner,三消费点一次切换) |
| D | OnFailure 裁决落地(推荐删除 + compat 映射 + 三处注释清除);`ArtifactStreaming` 若产品要则按 registry 接线,否则维持候选 | policy(mode)、engine、workspace(import 映射)、protocol registry | 串行 |

## 4. 验证

- 协议帧 golden(26 条)字节不变;registry 派生的能力宣告与 V2 快照对比(除已裁决项外零 diff)。
- 投影 golden:同一事件序列在 host/desktop 两端产出一致状态;分页交错、Lagged→Snapshot、fork 分支切换三个已知难点各一条种子。
- 定向:`cargo test -p pawork-protocol -p pawork-app -p pawork-cli -p pawork-client -p pawork-desktop`。
- 真实冒烟:desktop `--probe-smoke` 全指标 + Zed ACP `initialize/session/new/prompt`(S10 同款)+ headless `--json-stdio` 一轮。

## 5. 退出标准

- [x] 三通道宣告/授权同源于 registry;headless 手写表与 ACP 命令准入白名单删除;未登记命令 fail-closed 有测试(2026-08-20 波 B 收口;gui_host 巨 match 分发仍留 R4)
- [x] 投影 reducer 单一实现 + golden;desktop projection.rs 只剩渲染适配(目标 <800 行)(2026-08-20 波 C 收口;reducer 单源 + 三种子 golden + 两端对拍达成,行数 1542 偏差见波 C 收口记录 ⑤)
- [x] OnFailure 有决议并落地;S13-F16 三处收窄注释消失(2026-08-20 波 D 收口:删除变体 + serde alias 只进不出 + compat 导入映射 NeverAsk 记 issue)
- [x] 帧 golden 零 diff;冒烟(GUI/headless/ACP)通过;v3_plan §3 更新(2026-08-20 波 D 收口:26 帧 golden 与 events_golden 零 diff;probe-smoke / headless --json-stdio / ACP 三通道真实冒烟通过,见波 D 收口记录 5)
