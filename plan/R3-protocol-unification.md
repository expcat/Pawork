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

- [ ] 三通道 dispatch/宣告/授权同源于 registry;手写表删除;未登记命令 fail-closed 有测试
- [ ] 投影 reducer 单一实现 + golden;desktop projection.rs 只剩渲染适配(目标 <800 行)
- [ ] OnFailure 有决议并落地;S13-F16 三处收窄注释消失
- [ ] 帧 golden 零 diff;冒烟(GUI/headless/ACP)通过;v3_plan §3 更新
