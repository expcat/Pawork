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
> 5. 偏差与登记:① desktop projection.rs 1542 行未达 <800 目标——剩余为 UI 态/渲染分组/渲染测试,审查确认无 reducer 语义残留,继续压行需拆文件或丢测试,超出本波写入集,行数目标偏差登记在此;② 既有怪癖原样保留(与旧实现字节一致,非本波引入):ToolCompleted 历史臂无 seen 前置检查、assistant delta 跨臂 message_id 不对称可拆条——留 R4/R6 语境再议;③ probe snapshot-reconnect 既有 flake(ROADMAP §4 已登记)首跑复现一次,重跑与全量均 9/9。

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
- [ ] OnFailure 有决议并落地;S13-F16 三处收窄注释消失
- [ ] 帧 golden 零 diff;冒烟(GUI/headless/ACP)通过;v3_plan §3 更新
