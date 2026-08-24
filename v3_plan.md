# Pawork V3 任务开启编排

> 本文是 V3 重构的**指定开启文件**。每次开新对话,提示词指向本文即可;主代理按本文编排**一个波次**(核查 → 设计 → 实现 → 收尾)。
>
> 本文只负责「选哪一波、怎么核查、怎么设计、怎么派子代理」。过程纪律(架构红线、契约、测试、凭证、收尾清单)以 [docs/task-guide.md](docs/task-guide.md) 为准,不在此重复展开。V2 版编排文档(`v2_plan.md`)已删除,历史见 [docs/v2-summary.md](docs/v2-summary.md) 与 git 历史。

---

## 1. 文档地图

| 文档 | 读它做什么 |
| --- | --- |
| 本文 `v3_plan.md` | 开启编排、当前指针、统一提示词、子代理模型约定 |
| [ROADMAP.md](ROADMAP.md) | 阶段总索引(R0–R9)、依赖、状态、遗留映射、未决 ADR |
| [plan/](plan/) | 本阶段任务书:目标、证据(带路径行号)、决策点、波次拆分、退出标准 |
| [docs/design.md](docs/design.md) | 包布局与冻结契约(§2 已于 R1 波 E 重写为 V3 布局) |
| [docs/gui-design.md](docs/gui-design.md) | Desktop GUI 设计;R8 组件化以其与 [design/README.md](design/README.md) 视觉基准为准 |
| [docs/task-guide.md](docs/task-guide.md) | 开启核对、红线、测试通道、并行纪律、收尾与报告 |
| [docs/v2-summary.md](docs/v2-summary.md) | V2 交付、冻结契约清单、S13 拍板、遗留债务原委 |
| [docs/references.md](docs/references.md) | 参照项目手册;§7 为 R0–R9 阶段参照指引(开波时随任务书查阅) |
| [docs/v1-migration-reference.md](docs/v1-migration-reference.md) | V1 迁移词典(冻结;归档代码考古用) |
| [AGENTS.md](AGENTS.md) | 仓库级红线与工作约定(V3 版) |
| [docs/code-map/README.md](docs/code-map/README.md) | 按需导览(按写入集读各包 `MODULE.md`)，**不是**布局/契约事实源；冲突以源码为准 |

---

## 2. 开启提示词(用户侧)

**子代理模型必填。** 未写模型时,主代理只完成「读指针 / 提议下一波」,然后提问并停止,不启动核查或实现。

```text
按 v3_plan.md 开始。
子代理模型:〈必填:当前宿主可接受的模型标识,见 §7〉
范围覆盖:〈可选。例:R1 波 B。不写则按 §4 自动选下一波〉
凭证:〈可选。本波无需真实 key / auth 文件与 .env 已就绪〉
临时约束:〈可选。例:只设计不实现——默认不要用〉
```

同一条消息里的「范围覆盖」优先于自动选择。

---

## 3. 当前指针(每波收尾由主代理更新)

| 字段 | 值 |
| --- | --- |
| 当前阶段 | R9 🔵 已开启(2026-08-24 用户指令,[plan/R9-consistency-closeout.md](plan/R9-consistency-closeout.md));R8 🔵 波 E 自动化部分已收口(CR09 复核 + K-03 取证 + 四项拍板 + 心跳修复,提交 528ab3d),仅剩 K-03 人工签字(gui-design.md 附录 A.2 十一项,签字后 R8 🟢) |
| 阶段状态 | R0 🟢(波 0/A/B/C 全部收口,2026-08-18;改判 3+1 项见 ADR-038 落实改判记录);R1 🟢(波 A–E 全部收口,2026-08-19);R2 🟢(波 A/B/C/D 全部收口,2026-08-20);R3 🟢(波 A/B/C/D 全部收口 + 整阶段审计修复,2026-08-20~21);R4 🟢(波 A/B/C/D 全部收口 + 整阶段审计修复,2026-08-21~22);R5 🟢(波 A/B/C 全部收口 + 整阶段审计修复,2026-08-22;真实 Anthropic 冒烟 fail-closed 登记 ROADMAP §4);R6 🟢(波 0/A/B/C 全部收口 + 整阶段审计修复,2026-08-23；真实 fork/compact 冒烟登记 ROADMAP §4 人工验收);R7 🟢(波 0/A/B/C 全部收口 + 整阶段审计修复,2026-08-23;审计修 P1 反引号地板漏检与 P2 probe 零终端场景,登记项见 ROADMAP §4;Desktop PTY 面板冒烟登记 ROADMAP §4 人工验收);R8 🔵(波 A ✅ 2026-08-24:theme tokens + 真窗口启动崩溃修复;波 B ✅ 2026-08-24:components 基础族 + 五组菜单浮层化 + hover/active + FollowScroll;波 C ✅ 2026-08-24:ui/ 六模块拆分 mod.rs 824<900 + Timeline list() 变高虚拟化 + 长标题 truncate;波 D ✅ 2026-08-24:Inspector 三页签 + K-04 只读 Changes 面(Files/Summary/DiffView/ActivityPopover)+ K-06 Resources 面(mcp_list 翻 gui.available)+ 「@」host 端到端(expand_at_refs,零 wire 变更);波 E 进行中:自动化部分 ✅ 2026-08-24(CR09 五项复核 + K-03 取证截图 7 张 + 用户四项拍板 D1–D4 + desktop 空闲心跳修复 33min soak 实证,grok_reviewer 终审 pass,已提交推送 528ab3d),K-03 人工走查待签字);R9 🔵(2026-08-24 用户指令开启) |
| 已完成波次 | R0 波 0(ADR-038)、R0 波 A(大块归档)、R0 波 B(小块删除与降级)、R0 波 C(D16 git 服务裁剪 + 收口,2026-08-18;补判 commit.rs 归档)、R1 波 A(ADR-039 Accepted + api→domain golden 先行平移 + diagnostics 迁宿主撤包,2026-08-19;members 37→35)、R1 波 B(storage/providers/workspace 三大合并 + host/app 装配缝,2026-08-19;members 35→28)、R1 波 C(mcp→tools ∥ quota+provider-control→control-plane + host/app 装配缝,2026-08-19;members 28→25)、R1 波 D(gui-server→app `gui_server/` ∥ channels→cli `channels/` ∥ sdk→client `headless/` + probe→client tests/example,2026-08-19;members 25→21)、R1 波 E(members 定稿 21 + 19 库 `git mv` 扁平 `crates/` + design.md §2 重写 + 红线断言随迁 + 21 包定向测试 + 真实冒烟,2026-08-19;整阶段审查已修复 probe 暴露的动态模型切换与 client 错误帧路由缺陷,并收紧三条红线回归;修复后 desktop probe 全绿)、R2 波 A(L1 rand→getrandom 6 点 + L2 parking_lot→std::sync 52 处含 orchestration 死声明 + L3 base64→auth 本地 base64url 模块,对拍 golden 先行后固化 13 组固定向量,2026-08-19;rand/parking_lot/base64 退出直接依赖,根 workspace 声明已清)、R2 波 B(升级 U1–U8+U10 九项全落地,2026-08-20:notify 8.2(debouncer 死声明删;整阶段复核补 Flag::Rescan 全量重扫)+ portable-pty 0.9(官方 signal() 替 Display 解析 hack,甩 nix 0.25 老栈)+ windows 0.61.3(0.58 退出,2 处适配,msvc 交叉 check 绿)+ ts-rs 12.0.1(7 个 .d.ts 索引签名去 ? 属形状变化,用户拍板 A 接受并登记)+ reqwest 0.13.4(上游强制 rustls-tls→rustls+form,TLS 信任栈 webpki-roots→rustls-platform-verifier 与 cmake 构建依赖已登记,redirect/proxy 语义不变)+ toml 1.1.4(47 测试双绿)+ rusqlite 0.40.2(SQLite 3.53.2,backup/迁移回归绿)+ sha2 0.11(RFC 7636 golden 字节不变)+ directories 6.0.0(macOS 快照×2;整阶段复核关闭 F3 环境短路并修正 Windows 路径注释);lock 836→830,CLI 直控面多版本清零;默认 desktop 例外 sha2/toml/thiserror,windows 0.57 为可选 screen-capture lock 残留;审查 F1/F3 已落任务书),R2 波 C(rmcp =2.2.0→=3.1.3 升级决议落地,2026-08-20:整阶段复核后 65 条 MCP 契约测试 + 隔离断言;历史 stdio 冒烟通过但 2.2.0 基线原始输出未归档;codec.rs fail-closed 适配(InputRequiredResult 专名措辞、显式回归、EchoServer 返回 CallToolResponse),dev 死声明 macros 移除;MSRV 1.85→1.88(rmcp 3.x 为 edition 2024),lock 830→826)、R2 波 D(收口断言,2026-08-20:默认目标 tree 归档断言 notify/reqwest 单版本及 sha2/toml/thiserror 例外;Cargo.lock 断言 windows 0.58 退出,0.57 仅为 Windows screen-capture 可选闭包;CLI 闭包传递残留登记 base64 0.22/0.23、syn 2.x(tracing/thiserror1/ICU)+3.x(async-trait 等)、thiserror 1.x(portable-pty→filedescriptor),直控面清零;lock 836→826 净 -10;历史编译数字与 xAI OAuth/MCP stdio 冒烟通过但原始输出未归档,不作仓内可复现门禁;raw tree 输出归档 plan/R2-cargo-tree-duplicates-2026-08-20.txt;整阶段复核修复 notify/directories/rmcp 测试缺口与文档口径)、R3 波 A(Command/Capability Registry 落地 + GUI 通道切派生,2026-08-20:protocol 新模块 app/registry.rs 表驱动登记 19 command + 11 query 全量条目——wire 名/三通道可用性/所需 capability/幂等/引入版本,headless 与 ACP 列照抄现手写表供波 B 消费;cli/gui.rs 宣告改 registry 派生,派生向量 = 原手写 {Events,Snapshots,TerminalStreaming,Approvals} 由新 golden 钉死,无条目 require ArtifactStreaming(K-08 编码为数据);app/gui_server 新增逐命令授权门,未登记/未授予 fail-closed(Terminal*/tool_approve/snapshot_fetch 紧化,拒绝先于进入 host);gui_host 删 command_name/query_name 硬编码镜像改查 registry,巨 match 不动留 R4;26 条帧 golden 与 schemas/ 零 diff;测试 +9:穷尽 match 完整性、serde 双射、宣告向量 golden、样本表双射、未授予拒绝 e2e×3;四包定向全绿;审查 F1/F3 同波补测闭环,probe snapshot-reconnect 既有 flake 登记 ROADMAP §4;写入集含 cli/gui.rs 单文件修正,实态复核记录已回写任务书)、R3 波 B(headless/ACP 切 registry 消费,2026-08-20:headless.rs 删 command_capability/query_capability 手写表,handle() 两处改查 registry headless 列,gate_capability 与两类 UnsupportedCapability 文案逐字保留;ACP decode_payload 四解析臂作为协议路由保留,Command 产物新增 admit_acp_command 查 registry acp 列 fail-closed,显式拒绝臂与 catch-all 逐字保留;新增 HOST_CAPABILITIES 快照钉死、registry headless 列 ⊆ HOST_CAPABILITIES、acp 列全集钉死与 admit/reject 文案测试;26 帧 golden、ACP 11 fixture golden、headless 16 案例、spawn_e2e 能力门、probe 9 场景全绿零 diff;审查 verdict=pass,两条低阶观察(admit 拒绝分支现行不可达、command_entry 缺条目 panic)登记为非缺陷;写入集实缩为 cli 两文件,protocol headless/ 与 client tests 只跑不改,实态记录已回写任务书)、R3 波 C(投影 reducer 下沉 protocol::projection,2026-08-20:805 行纯模块承载 project_event(自 gui_host 逐字平移)/TimelineProjection 合并核(seen 去重、partition_point 有序插入、双键 tool 锚)/resume 基线语义;host 删本地映射 re-export 保名 + 清除 gui_server/session.rs 重复 timeline() 预调用;client 仅追加 re-export;desktop projection.rs 2346→1542 行只剩渲染适配(行数目标偏差登记任务书);golden 三种子(分页交错/Lagged→Snapshot/fork 切换)+ desktop 8 条语义随迁 + host timeline() 真库对拍;CR08-08 根治:run started 文案统一 + run/diagnostic 有序插入;五包定向全绿、26 帧 golden 与 events_golden 零 diff、probe-smoke 隔离实例真实冒烟通过;审查 pass,两条既有怪癖(历史 ToolCompleted 无 seen 前置、assistant delta 跨臂 message_id 不对称)原样保留并登记)、R3 波 D(OnFailure 裁决落地,2026-08-20:变体删除 + NeverAsk serde alias「接受旧值、不再产出」;compat 导入 codex on-failure 与 claude acceptEdits 映射 NeverAsk + CompatIssue warning;app/cli 解析兼容行为逐字节等价;S13-F16 三处收窄注释清除;ArtifactStreaming 维持候选、protocol registry 零触碰;写入集实态修正为 policy/workspace/app/cli 六文件并回写任务书;四包定向全绿、26 帧 golden 与 events_golden 零 diff、cargo check -p pawork 通过;审查 pass,低阶观察五值序列化钉死同波闭环;probe-smoke 隔离实例 r3d、headless --json-stdio、ACP 三通道真实冒烟通过,R3 阶段收口) |
| R3 整阶段审计 | `xai/grok-4.6` 四路分域复核波 A–D + 一路最终复核;修复 registry/生产 host 可用面失真、GUI 帧能力泄漏、订阅拒绝后收帧污染、TerminalSessions snapshot 泄漏、Timeline 持久化分页游标、assistant committed 排序/跨轮/live-history 锚点、并发工具身份与重复 live output;headless/ACP 与 OnFailure 实现复核无缺陷;定向包级门禁与 `cargo check -p pawork` 全绿,保持 R3 🟢,未启动 R4(2026-08-20~21) |
| R4 波 A 进度 | 波 A 收口(2026-08-21,glm_worker 单 owner + glm_reviewer):阶段1 四服务(Usage/Task/Import/Extension,2026-08-21 早)+ 阶段2 Session/Run/Approval 三服务与 provider_assembly 落地,lib.rs 33 条内联测试随迁,lib.rs 4131→1413(<1500);gui_host.rs(2407)目录化为 gui_host/(mod.rs 679<800 + bus/events/handlers/tests),巨 match 改 QUERY_HANDLERS 7 / COMMAND_HANDLERS 10 静态分发表(wire 名查表,与 protocol registry gui.available 双射,新 pin 测试锁定),幂等 wrap 留在分发前,fallback 文案逐字保留;AppCore 对外 pub API 形状不变。验证:app 122 绿(1 ignored,+1 授权 pin)、protocol golden / domain events_golden / typegen / client probe+spawn_e2e / desktop 27 / cargo check -p pawork 全绿,26 帧 golden 零 diff;审查 verdict=pass,P2 两项(状态文档、doc 注释搬迁)同波闭环。CatalogOnlyProvider 显式无凭证状态留波 C/D;worker 首跑遇 idle-rustc 环境异常,主代理串行重跑均绿 |
| R4 波 B 进度 | 波 B 收口(2026-08-21,grok_worker 单 owner + grok_reviewer 双轮):CommandLedger 入 SQLite——storage 追加 v11 `command_ledger` 迁移(新表不动 v1–v10 DDL,CURRENT_SCHEMA_VERSION 10→11;v11 被占用致 R6 迁移顺延 v12,已回写),SessionStore::open 迁移后 reclaim inflight(open_read_only 不动),check SELECT+INSERT 单 actor call 原子,record 冲突映射 DuplicateCommand/KeyConflict,全局容量 4096 淘汰(与内存前身语义一致,注释+pin);app 侧 IdempotencyStore 改以 ledger 为持久态(进程内仅余 Notify 唤醒表,不算内存 CAS),client 作用域由前缀串改 (tenant, client_scope, command_id) 列式,record 失败 tracing::error + 计数不吞错,release 失败改 log。K-02:engine LoopContext::request_approval 加 emitter 参数,Requested 在等待(含 batch 短路)前 emit 落盘,engine 只补 Responded;GUI resume 改 resume_messages_keep_pending 不 seal(CLI resume_messages 维持 seal Denied,fail-closed 不变),snapshot PendingToolApprovals 只读合并投影 waiting(全局,对齐 host.pending() 语义),ToolApprove 对非 live run 的 waiting 调用走 durable resolve(Responded+Completed is_error+MessageCommitted,工具不重跑),live run 维持 queued 竞态语义。写入集实态含 engine tool_loop.rs(emit 时序)并已回写任务书;共 15 文件。验证:storage 90+5/engine 65+2+1/app 110+6+13+2/domain/protocol(45+10+7+5+15+16+3+16+8+8+7)/client(9+22+9+1+3+1) 全绿,cargo check -p pawork 绿,golden 零 diff;审查首轮 changes-needed(P0:Queued 竞态误封 live 等待调用;P1:record 失败断言未走生产 wrapper、K-02 回归未经 GUI handler;P2×2 登记为 pin),修复后复核 verdict=pass;K-02 崩溃回归为 app 层 drop+reopen 模拟,真实 kill -9 冒烟与 GUI 人工验收留待人工验收 |
| R4 波 C 进度 | 波 C 收口(2026-08-21,grok_worker ×2 并行 + grok_reviewer 双轮):契约面主代理先行落地 crates/domain/src/degrade.rs(DegradeKind 六类 + 默认 sink 分级 + 双出口 AgentEvent::Diagnostic / protocol From 转换,code 命名空间 degrade.* pin 冻结,serde 零 diff)。轨 a ACP actor 化:AcpHost 单 actor(独立 OS 线程 + current_thread runtime,证据注释在 new)+ mpsc 信箱独占 5 map/negotiated/outbox,std Mutex 与 35 处毒锁 expect 清零(wire 序列化 expect 走 degrade.acp_state),prompt 串行语义与 HEAD 一致(只覆盖 reserve→dispatch→bind,主代理核实旧 gate drop 点),urgent cancel select! 插队,DrainOutbox interruptible 即时服务,fail_closed ack 收敛,公开 API 与 acp.rs 装配零变化,floor.rs 追加双会话交错种子 + Diagnostic 不发 ACP pin。轨 b 五接点:HOME 回退与无凭证兜底真实 tracing::warn 外发(RecordingSubscriber 证外发,AppCore.last_degrade 死存储删除)、Lagged 删 seq-0 旁路改经 hub 真序列 + host_tx 直发 + ReplayUnavailable(测试改断言递增序列帧)、tasks_finish/persist_tasks 经 TaskService→RunService 交接 persist-first 落盘(本接点 let _ 清零)、幂等 record 失败主代理拍板只 tracing 不发客户端帧(防误读重试,+pin);gui_host/events.rs degrade.* 特判 level/message。验证:domain 56 / protocol 141(frames+golden+projection+typegen 全量)/ app 139 / cli acp 41 全绿,cargo check -p pawork 通过,fixtures/golden/schemas 零 diff,合计 377 绿;审查首轮 4×P1 changes-needed,修复后复核 verdict=pass findings=0。登记:cli map.rs 两条 dead_code 为 HEAD 既有留波 D;DataDirOutcome pub 导出暂无生产消费者;ACP 不承载 degrade 帧、桌面忽略 degrade.* 为显式决议;双连接传输层压测与 Zed 冒烟留人工验收;任务书 §2.3/§2.4 已回写 |
| R4 波 D 进度 | 波 D 收口(2026-08-21,grok_explorer ×3 核查 + glm_worker 单 owner + glm_reviewer;实现中途按用户指示由 grok 切 glm):host 域非测试 `let _` 58 处分类清零(常态竞态 debug、fail-closed 升 warn、弃绑定改定义处命名),rg 断言非测试归零(整阶段审计复核实态残留 4 处全在 #[cfg(test)]);HOME 回退告警升级——resolve/default_data_dir 纯路径选择,单一 consume_data_dir_outcome 结构化出口(load_with + ops inspect),attach_workspace/GUI/extension 静默以免重复告警;usage 哨兵按 ADR-038 D1 钉死(三段 doc + 三字段 pin,账本写入值零变化;legacy None vs host Some(1) 口径差异登记 ROADMAP §4 留 R9);hub.rs 简化(RingInner 单字段包装拆除、零消费 subscriber_count 删除、publish_with_envelope 收窄,8 条序列测试断言未动);acp/map.rs stop_reason_for/cancel_request 死码删除。写入集 app 10 + cli 8 文件(357+/124-)。验证:app 121+6+13+2 / cli 35+16+25 / protocol golden 5 / domain events_golden 3 / client 45(含 probe 场景与 spawn_e2e)全绿,cargo check -p pawork 通过,golden 零 diff;审查 verdict=pass,P2×2(pin 未端到端驱动已核实接受、登记项提醒)闭环。K-02 kill -9 冒烟、GUI/Zed 人工验收、双连接压测登记 ROADMAP §4,不阻塞 R5 |
| R4 整阶段审计 | `xai/grok-4.6` 四路分域只读审计波 A–D + 主代理逐项裁决 + grok_worker 修复 + grok_reviewer 双轮复核;修复 7 项:InFlight 同键不同 command_id 占位挂死与丢唤醒(50ms 有界等待 + SQLite 权威 CAS 重查,hazard1/hazard2 独立回归)、record 失败 inflight 不释放(DB 类错误幂等重试一次再 release,带键 Replay 断言)、tasks_start_agent 吞错升 warn、lib.rs compact_session 内联残留搬入 RunService(1458<1500)、cli/acp.rs 三处 flush_outbox 补 warn、wait_std 无界 recv 改 recv_timeout(2s) 并区分 Timeout/Disconnected、storage open_read_only 不 reclaim 补回归;驳回虚构路径等误报;主代理独立门禁 355 passed / 0 failed + `cargo check -p pawork` 全绿,冻结契约零触碰;保持 R4 🟢,未启动 R5(2026-08-22) |
| R5 波 A 进度 | 波 A 收口(2026-08-22,glm_explorer ×3 核查 + glm_worker ×2 并行 + glm_reviewer;独立复核后提交):轨 a provider_hints 契约——domain 新模块 provider_hints.rs(命名空间 provider_hints.<provider>.<key> 语法、键 ≤128B/值 ≤64KiB、冻结 LEGACY_HINT_KEY_MAP 三旧拼写映射,pin 测试);storage event_store 删 OPAQUE/CONTINUATION 两 allowlist 常量,sanitize 改命名空间规则分派(Secret 键扫描 + 大小上限 + 规范后缀形状校验,命名空间外维持保形脱敏),写边界先规范化(旧拼写永不落盘),全部读取链(replay/tail/by_branch + rebuild_projection/events_on_lineage/snapshot 消息行)经共享 decode_persisted_json 做 legacy→canonical 映射 + 无旧键快路径短路(审查 P2-1;独立复核将 contains 改为带引号完整旧键,避免规范键误中慢路径并补 pin);生产者 SUMMARY_ENTRIES_KEY 改规范键 provider_hints.openai.responses.summary_entries,to_input 兼容三拼写——核查发现的键名错位(生产写无前缀 responses.summary_entries 被脱敏)随之根治。轨 b 通道注册表——providers 新增 channels/registry.rs(ChannelKind/OAuthFlow/OAuthPreset 自 app 逐字迁入 + const 镜像,CHANNEL_REGISTRY 六行顺序不变、行不带 cfg、is_enabled 单一 cfg 求值点 fail-closed);ApiKeyChannel 枚举删除改 preset 驱动(双重 fail-closed);app/channels.rs 改 facade 保公开 API(oauth_override 留 app,cli 零改动);engine 守护名单 = registry ids 派生 + 基线别名(审查 P2-2 后恢复 grok/glm/opencode/qwen 短别名,强度不缩减),engine dev-dep 加 pawork-providers(domain_only 注释同步)。写入集 15 改 + 2 新 + Cargo.lock 1 dev 边。验证:domain 54+4+3 / storage 94+5 / providers 140+8+16 / engine 65+2+2 / app 125+6+13+2 全绿,cargo check -p pawork 通过,fixtures/schemas/信封/DDL/PWB1/26 帧 golden 零 diff;审查 verdict=pass,两 P2 同波闭环后独立复核再修快路径误匹配。登记:gui_host record 失败 tracing 断言并行 flake(R4 波 B 测试机制既有,与本波无因果)入 ROADMAP §4;真实通道冒烟(六通道 models 聚合)未跑,留波 B/C 或人工验收 |
| R5 波 B 进度 | 波 B 收口(2026-08-22,glm_explorer ×3 核查 + glm_worker 单 owner 串行 + glm_reviewer):credential locator 合一——auth 新模块 locator.rs 单一事实源(env 名推导 api_key_env_name/read_api_key_from_env 自 resolve.rs 私有平移为 pub、PROVIDER_SERVICE_PREFIX/secret_service_for/oauth_secret_service、MCP_SERVICE_PREFIX/is_mcp_secret_service/MCP_AUTH_FILE_NAME),workspace config env.rs 过渡实现整文件删除(唯一外部消费者 app/provider_assembly.rs:23 改 auth locator,mod.rs 注释修正),workspace 无 auth 依赖边(cargo tree 断言)。keychain 词汇迁移——StoredCredential 字段 keychain_service/keychain_account→secret_service/secret_account(serde alias 读旧写新 + 迁移测试:旧字面 JSON 可读/新写只出新名/round-trip 相等),keychain_ref→secret_backend_ref,CredentialSource::Keychain→AuthFile(无 serde 派生),KEYCHAIN_SERVICE_PREFIX/keychain_service_for→locator 单点,default_credential 重复常量删除;核查确认该词汇本就不是 auth.json 落盘键,AuthFile{version:1,entries} 形状零触碰。mcp-auth 域隔离收编——pawork.mcp.* 前缀与 mcp-auth.json 文件名常量化进 locator,tools/mcp/security.rs 与 app/extensions.rs:335 改消费常量,F05 语义(env_clear、PAWORK_API_KEY_* deny、独立后端文件)与错误文案逐字节不变。写入集 14 文件(13 改 + 1 新 + 1 删)+ 任务书回写。验证:auth 72+1ignored / workspace 136 / tools 130 / app 146 全绿(worker、reviewer、主代理三方独立跑),cargo check -p pawork 通过,golden/DDL/帧/schemas 零触碰;审查 verdict=pass 无 P0–P2,两 P3(is_mcp_secret_service 暂无生产消费者为设计预期;收口登记项)已处理。偏差 3 项实态裁决:app/lib.rs 测试实零改动;oauth_service 实为 pub 且零外部消费者,连同 re-export 收敛进 locator;security.rs 保留 starts_with 形状只换常量来源。登记:serde alias 兼容期一个版本,移除入 ROADMAP §4(留 R6/R9) |
| R5 波 C 进度 | 波 C 收口(2026-08-22,glm_explorer ×3 核查 + glm_worker 实现 + glm_reviewer 双轮):K-10 Anthropic 能力收口——prompt cache Automatic 有 cap 才写 last system/last tool `cache_control ephemeral`,Required 无 cap → InvalidRequest,Disabled 永不写;thinking `{type:enabled,budget_tokens}`(显式或 Low=1024/Medium=2048/High=4096),temperature≠1.0 或 max_tokens≤budget 拒绝;hosted_tools/extensions nonempty 进 negotiator `required_tools`,未声明 HTTP 前拒绝;signature 经 protector,事件只带 `ProtectedBlobRef`。CapabilityNegotiator 在 `prepare_request` 接线。ReasoningProtector 持久化——app `protected` feature 拉 PWB1,`Arc<SwappableReasoningProtector>` 同一实例注入 ChatGPT/xAI/API-key/Anthropic,`open_protected` 后 bind;instance-level BlobScope `instance-reasoning` 已接受偏差。审查首轮 P1(无签名 thinking 回放必 400)与 P2/P3(Completed 重复、cite 合成 id)已修,复核 verdict=pass。真实 Anthropic 冒烟 fail-closed(无 ANTHROPIC/GLM env,auth.json 无 anthropic 条目)登记 ROADMAP §4。未开 R6,未提交。 |
| R5 整阶段审计 | `xai/grok-4.6` 分域核查波 A–C + 一路最终复核;主代理复现并修复 provider_hints 深层 Secret/旧键导出、损坏 auth 静默回退与 auth status 吞错、MCP Secret service/sandbox 域错位、Anthropic Required cache 落点与 thinking budget/signature/redacted payload/跨模型 continuation 绑定、negotiator 分区不变量、master.key 并发首建/权限/链接与 Secret 目录沙箱缺口;终审后主代理追加跨模型 continuation 绑定、master.key 固定数组读取/错误分支清零,以及 `PAWORK_HOME` 目录级 deny。providers/auth/workspace/exec/storage/app/cli 定向测试、PWB1 golden、`cargo check -p pawork` 与依赖树断言全绿;保持 R5 🟢,未触碰 R6 实现(2026-08-22) |
| R6 波 0 进度 | 波 0 收口(2026-08-22 起草,2026-08-23 用户确认 Accepted;grok_explorer ×3 核查 + ADR 主代理起草):三路核查修正任务书证据——session_events.branch_id 自 v1 即一等列(F09/v10 后补的是 messages 投影列,migration.rs:247-269)、压缩三处语义不一致(host lineage 水位 vs storage events_by_branch 读取 vs 按事件所属 branch 删除)、ancestor_lineage 公开 API 几乎无生产消费者、无检入的真实库升级 golden;任务书根因段已回写实态。[ADR-040](docs/adr/ADR-040-session-branch-lineage.md) Accepted:D1 原生化、D2 append-only 单表 + 全局 sequence 保持、D3 lineage 单点收编消灭 DEFAULT 'main' 静默回退、D4 schema v12 迁移(回填即校验 + 检入库升级 golden + 信封 v1 零 diff)、D5 压缩按分支水位三处合一、D6 K-05 波 C 范围登记;含迁移方案与回滚不可行性说明。文档波次只做链接/行号核查,未跑编译。ADR 闸门已过,波 A 可开 |
| R6 波 A 进度 | 波 A 收口(2026-08-23,glm_explorer ×3 核查 + glm_worker 单 owner + glm_reviewer):storage 分支原生化——CURRENT_SCHEMA_VERSION 11→12,新增 v12 迁移(TEMP 触发器对无事件背书的 messages 行 fail-closed,RAISE ABORT 整批回滚 → 按事件所属 branch 重建整表去 `DEFAULT 'main'` → 按原名恢复两索引),v1–v11 DDL 零改写;检入 4 个升级 golden(v10 fork 树 / v11 交错 / v10 压缩折叠 / v11 孤儿负例),fixture 7 JSONL 由真实写入路径落盘字节生成,`PAWORK_WRITE_STORAGE_GOLDEN=1` 门控再生;删除公开 `ancestor_lineage`(全仓核实零生产消费者),lineage 单点 `load_ancestor_lineage`/`events_on_lineage`;`create_session` 显式写 active_branch。写入集仅 crates/storage 6 路径。验证:storage lib 99 + read_range 5 绿(1 ignored = 门控再生器),`cargo check -p pawork` 绿,信封 v1 / export v3 / PWB1 golden 零 diff;reviewer verdict=pass,P2×2(本文档同步 + golden 生成器共享 lineage 实现知悉项,登记波 B 保持 fixture 不再生)。偏差 2 项:m-orphan 移出 v9 正例种子(旧断言与 D4 fail-closed 冲突),孤儿负例由专项 golden 承接;fixture 占位先行后由生成器覆写。登记波 B:压缩三处口径合一(含 `filter_retention_inputs` 二次过滤)、fork turn 边界(ADR D5)、`fork_from_event` 幂等不对称、Pi 导入零事件分支元数据 |
| R6 波 B 进度 | 波 B 代码与自动门禁收口(2026-08-23,GLM 三路核查 + glm_worker 双轨 + glm_reviewer):storage compact 读取/retention 过滤统一 active lineage；ProjectionSnapshot 从 append-only event ledger 重建消息并按 lineage 可见最大水位折叠，v12 branch-local 物化 fold/schema/fixture 不变；父支晚压缩、late-fork、兄弟隔离回归绿。host compact 错误显式上抛，无 outcome 水位 0；fork_from_event 仅 run 三终态 + CompactionCompleted 且同 tuple 幂等；Pi Branch marker 折叠为 main Diagnostic（含无 ID null 形态）。protocol 非 wire ForkBoundary 单点判型，Desktop 渲染/动作双 gate + 同 session 换 branch reset baseline。验证:storage `--features compaction` 125+5 绿(1 ignored)、engine/protocol/storage core 绿、app 串行 134+6+13+2 绿(1 smoke ignored)、Desktop runtime_shaders 28/28、`cargo check -p pawork` 绿；默认 Desktop 因本机缺 Metal Toolchain 阻断。GLM reviewer 无源码 P0–P2，首轮 P1 验证缺口已补跑闭环，P3 Pi 边界已补断言；schema/wire/export v3/fixture 零 diff。真实 Provider fork/compact 冒烟登记 ROADMAP §4；本波已提交 origin/main |
| R6 波 C 进度 | 波 C 收口(2026-08-23,glm_explorer ×3 核查 + glm_worker 单 owner 串行 + 四轮修复 + glm_reviewer 双轮):K-05 落地——compat 双形态解析(Claude Code 本地 JSONL 自动判定,sidechain/thinking/噪声跳过、aiTitle/customTitle 真实键取标题、未知行落 Raw;Codex rollout 信封 `{timestamp,type,payload}` 自动判定,session_meta 取 identity,agent_message/user_message 映射,event_msg 仅 token_count→Usage,损坏文件零 record fail-closed;旧导出/平铺路径逐字节不变);workspace 新增 `session_scan` 只读发现原语(有界/不跟 symlink/根缺失为空,Claude 排除 `agent-*.jsonl` sidecar);CLI `sessions import --from claude|codex` 批量导入经 app facade(不加 cli→workspace 依赖边),.jsonl 嗅探签名化(首行整行读取);fork 生产路径 `fork_from_event`→export v3→import 往返回归。写入集实态 storage(import)+workspace(import)+app facade+cli 已回写任务书。验证:storage 108+5 / workspace session_scan 定向 / app 135+6+13+2 / cli 39+16+25 全绿,`cargo check -p pawork` 绿;隔离数据目录真实样本导入 + export 还原 + `--from` 幂等通过。收口审查发现真实 home 上 Claude subagent sidecar 与父会话共用 sessionId,扫描层已排除;P3 登记 ROADMAP §4。冻结面零 diff(export v3/envelope v1/DDL/schemas)。R6 阶段 🟢 |
| R6 整阶段审计 | 整阶段审计收口(2026-08-23,主代理确定性门禁 + grok_explorer ×4 分域核查 + grok_reviewer 终审):四域 = ①storage 原生化(v12 迁移/4 升级 golden/lineage 单点/删除公开 ancestor_lineage/全局 sequence)②压缩-投影口径合一(events_on_lineage 读取 + filter_retention_inputs、event-ledger snapshot 水位折叠、父/子/兄弟回归、compact 错误上抛)③fork 边界·Pi 折叠·Desktop(ForkBoundary 非 wire + 双重 gate、同 session 切支 reset、Pi pi.branch_collapsed 单分支)④K-05 导入全链(双形态解析、fail-closed、session_scan sidecar 排除、CLI --from facade 接线、Secret 前缀拒绝、fork→export v3→import 往返)——全部通过,无 P0–P2。P3×2 已修:①parse_claude_local_jsonl 噪声计数补齐——thinking/queue-operation/last-prompt 与 sidechain 一致以 skipped_* 写入 unknown_fields(对齐波 C「跳过并计数」承诺与 codex 侧口径);②entry_index_by_identity 注释定性——sequence 回退仅为 reducer 内部防御性查找(首个命中,不校验唯一性),对外锚点仍只用 event_id。修复后门禁全绿:storage 131+5(compaction feature)/protocol 12 套件/engine 66/app 141+6+13+2/workspace 115+13+15/cli 42+16+25/desktop runtime_shaders 28/28/cargo check -p pawork;冻结面零 diff(envelope v1/export v3/DDL v1–v11/GUI 帧/波 A fixture 字节)。reviewer verdict=pass(两条注释措辞 P3 当场修复)。R6 保持 🟢;波 C 既有 5 项 P3 继续留 ROADMAP §4 待 R9 |
| R7 波 0 进度 | 波 0 ✅ 收口(2026-08-23 用户确认 Accepted;glm_explorer ×3 核查 + 主代理本机实测与起草):三路核查修正任务书证据——macOS profile 实态已是 deny-default 写白名单(读因 Darwin 25+ firmlink/cryptex 放开整盘)、network_allow_hosts 为无生产赋值的内存字段而非配置项、PTY 裸路径响应自证 uncontrolled、AskForDangerous 误分类静默放行;任务书 §1 已回写实态。主代理本机 Seatbelt 原型实测(macOS 26.6.2/Darwin 25.6.0):读侧全白名单连 /bin/echo 均 SIGABRT 不可行;写白名单矩阵(workspace 可写/HOME/.git/.ssh 全拒/网络 deny 有效)与工具链(clang/git/cargo/brew)全通;spawn 开销约 5.7ms。[ADR-041](docs/adr/ADR-041-sandbox-trust-model.md) Proposed:D1 写白名单正式化+读整盘 allow 挖洞、D2 PTY 创建入闸(豁免支留用户拍板)、D3 删 network_allow_hosts 字段(egress broker 转候选)、D4 手写 tokenizer;golden 先行面六清单入验证原则。文档波次未跑编译;glm_reviewer 审查 changes-needed(P2: §3.3 egress 候选悬空登记,P3×2: 写白名单措辞与行号)已同波修复;独立复核再修任务书导语/K-09「配置项」口径与行号范围。ADR 闸门已过,波 A 可开 |
| R7 波 A 进度 | 波 A 收口(2026-08-23,glm_explorer ×3 核查 + glm_worker 单 owner 串行 + glm_reviewer 三轮):golden 先行六面全部落钉——metadata.sandbox 键集+limits 六值(run_command)、IsolationLevel 五词汇 as_str/serde 双钉、投影 sandbox_timeline_detail/fallback_label 双 golden(原零覆盖)、CLI notice 空 note/legacy/Diagnostic 默认串分支、Seatbelt profile 整体输出、default_secret_paths 固定向量+受控 env 全向量(PAWORK_DATA_DIR 分支原零覆盖)。macOS profile 正式化:读=整盘 allow+secret 挖洞(删冗余系统读枚举);写=write_roots+/tmp+/private/tmp/$TMPDIR(raw+canonical 双形态)+/dev,每可写根永久禁写 .git(subpath)/.env(literal,双形态)——symlink 根(/var→/private/var)deny 不命中缺陷由主代理诊断、worker 修双形态;default_secret_paths 扩六项(.netrc/.git-credentials/.docker/.npmrc/.pypirc/.cargo/credentials.toml,MCP stdio deny 面同向收紧);IsolationLevel 词汇/serde 与 metadata.sandbox 键集零变化,macOS note 如实化;Linux/Windows 复核零行为变更(仅跨平台单测 dead_code lint 属性行)。验证:四包定向 17 组全绿(exit 0),真机种子五项断言实际执行(无 SKIPPED),cargo check -p pawork 绿(42.8s);审查首轮 P1(Windows env 期望矛盾,主代理发现)+P2×2(Diagnostic 测试假覆盖→纯函数提取等价断言、真机种子 SKIPPED 可见化)+P3(env RAII guard)修复,三轮复核 verdict=pass。登记:wiremock 两测试绑端口 EPERM 为宿主沙箱环境性失败(提权复跑绿,红→绿证据链 p1.log:579-580→p2.log:486-487);「沙箱回退」格式串 engine/cli/protocol 三处重复实现(本次未收敛,留 R9);MCP stdio 不经 selector 无 fallback 通知(既有,ADR-041 D2 已登记)。已提交 origin/main |
| R7 波 B 进度 | 波 B 收口(2026-08-23,glm_explorer ×3 核查 + glm_worker ×2 并行 + glm_reviewer):轨 a PTY 创建入 policy 闸——terminal_create 经 PolicyEngine(capability=Process,信任语义镜像 loop_ctx),NeverAsk/ReadOnly 直拒(D2),AskUser 一律 fail-closed 落 Deny(用户拍板选项 A;GUI 审批回路 run 键控、ADR-041 不含 wire 变更,命令级交互审批登记 ROADMAP §4 候选),响应替换 uncontrolled 裸语义为 sandboxed/policy/approval_mode/note 如实标注;golden 先行钉 TerminalCreate/Write/Resize/TerminalOutput 四帧 + 响应现状再演进到新形状(6 fixture);probe harness 配 AskForDangerous+trusted 保持终端场景走创建路径。轨 b shell 手写 tokenizer(引号/转义/管道/重定向/$()/反引号/变量感知)作统一解析前置,固定词表保留为分类输入;引号拼接程序名、程序位变量、curl|wget 管道进 python/perl 收紧为 Dangerous;-lc 组合簇提取同波闭环;灾难地板集合不变($() 内层完全静态可判时递归进地板,收紧非扩集合)。写入集 4 改 + 6 新 golden(protocol golden.rs+fixtures、app terminal.rs、policy shell.rs、client probe harness)。验证:policy 73 / protocol+app 17 目标 306 / exec 64(进程组回收回归)/ client probe self_test_all_scenarios 全绿,cargo check -p pawork 绿;schemas/、registry、wire、波 A golden 面零 diff。审查 verdict=pass(无 P0/P1):P2-1 wrapper(nohup/env/xargs)升档变松作已文档化残余接受并入 ROADMAP §4;P3 regex 死依赖入 §4。登记:Desktop PTY 面板冒烟入 §4 人工验收;既有 flake(gui_host record tracing)门禁中复现 1 次,登记不变。已提交 origin/main |
| R7 波 C 进度 | 波 C 收口(2026-08-23,grok_worker 单 owner + grok_reviewer;核查范围小按 §5.2 减为主代理自查):K-09 按 ADR-041 D3 落地——`SandboxPolicy.network_allow_hosts` 字段(原 sandbox.rs:61)与 os/macos.rs Enforce 分支 K-09 注释及死分支删除,共 2 文件 9 行删除 0 新增;`(deny network*)` 全拒不变,Seatbelt profile 输出零 diff,NetworkMode 三档与 IsolationLevel 词汇不动;全部 8 处构造点走 `..Default::default()` 编译安全,SandboxPolicy 不进 protocol/storage/domain 冻结面,metadata.sandbox 显式六键不受影响。验证:exec 64 绿(Seatbelt 真机种子全量复跑无 SKIPPED)、tools+app 定向全绿(1 smoke ignored 为既有登记)、`cargo check -p pawork` 绿、msvc 交叉 check 绿(Linux 按任务书走 CI,linux.rs 对该字段零引用);审查 verdict=pass 无 P0–P2,P3 文档回写同波闭环(ADR-041 为时点记录不回改)。R7 阶段 🟢;本波已提交 origin/main |
| R8 波 A 进度 | 波 A 收口(2026-08-24,glm_explorer ×3 核查 + glm_worker 单 owner 串行 + glm_reviewer;写入集实态扩为 ui/{theme,mod,text_input}.rs + controller.rs):theme.rs 落地——Theme 六组(bg3/surface2/border2/text10/accent2/semantic6,25 色值)+ 字阶 11/12/13 + metrics 14 常量,值类型 gpui::Rgba,impl Global 仅留挂载点(未来 main.rs 挂载),访问器静态 dark();消费点 rgb(/rgba(/0x 与数字 px( 字面量零残留,95 处消费点(92 rgb/rgba 调用 + 审批数组 3 裸 u32)与 HEAD 逐值等价(worker 等价脚本 + reviewer 多重集核对;任务书快照 97 处/0x3ecf8e/pty_view 实态不存在,已回写任务书)。前置修复:Desktop 真窗口启动崩溃——controller.connect 握手后 ack/subscribe_all 在 gpui 前台执行器(无 tokio reactor)上 await,receive_frame 内 tokio::time panic(client lib.rs:813,exit 134 实证),修复为握手/ack/subscribe_all 全部移入 runtime.spawn,ack 四分支语义逐字节等价,删死方法 record_last_acked;probe-smoke 走 platform.block_on 自带 runtime 无法暴露此路径,登记 ROADMAP §4。验证:cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders 28/28 绿两轮(worker /tmp/r8a-worker-test.log、修复后 /tmp/r8a-fix-test.log;--bins 替代 --lib --tests 因 bin-only 包,runtime_shaders 因本机缺 Metal toolchain),cargo check 同口径绿;probe-smoke r8asmoke 实例 EXIT=0(glm-4.7 completed→deepseek-v4-flash switched,cancelled=1,persisted=12,disconnect_survive=running;approval=not_requested 为模型未发起写工具调用,fail-closed 保持);真窗口启动实证(修复后进程存活并连接);像素截图因宿主显示器休眠未取得,视觉零变化由逐值等价+审查枚举兜底,截图对照并入波 E K-03 人工验收。审查 verdict=pass 无 P0–P2,P3 计数口径注释同波闭环。冻结面零触碰(帧 golden/schemas/reducer/Cargo 清单零 diff);未提交 |
| R8 波 B 进度 | 波 B 收口(2026-08-24,glm_explorer ×3 核查 + glm_worker ×2 串行(同文件冲突降串行)+ glm_reviewer 双轮):核查回写任务书——菜单实态 5 组(grouping/scope/model/entry fork/workspace confirm;无 provider/mode/session 菜单)、手写按钮 21 调用点、hover 全仓 0 处、gpui 0.2.2 anchored()/deferred()/hover()/active()/occlude() 全部在位。基准先行:design/README.md 新增 §8(hover/active 取值表——theme 25→29 色新增 surface.hover/accent.hover/semantic.success_hover/danger_hover;浮层菜单形态——deferred(anchored()) 单开互斥、Escape/外点关闭、occlude 滚轮无穿透;回底控件)+ gui-design.md §6 三行。轨 1:components/{button,label,panel,status_bar,list_row} 落地,16 处非菜单 on_click 迁移(审批 map 3 实例保留),hover/active 全量;轨 2:components/{dropdown,follow_scroll} 落地,五组菜单全迁浮层(workspace-confirm 条件打开保留),开合状态收敛 Option<MenuKind> 修双开,FollowScroll + 回底控件 + follow_terminal 重置。审查首轮 changes-needed(2×P2:FollowScroll 滚轮顺序假设反向 delta 双计→直读 is_scrolled_to_bottom;pending_outside_close 残留吞单击→(MenuKind, Point<Pixels>) 同一物理点击位置匹配;2×P3:不可达 dismiss_on_escape 移除、model 菜单开态 render 归一化),主代理修复后复核 pass。验证:desktop 28/28 三轮绿 + check 绿(6 条既有警告),probe-smoke r8bsmoke EXIT=0(签名同波 A),冻结面与禁动符号零 diff,model-picker 三件套在位;mod.rs 1990→1950(<900 为阶段目标,拆分在波 C)。登记:菜单 ↑/↓ 导航与 grouping/scope tab stop 留波 E;渲染面行为无自动门禁,三例人工验收(外点后再点同触发器/聚焦时 Escape/滚回底部重挂)并入 K-03;截图对照留波 E。未提交 |
| R8 波 C 进度 | 波 C 收口(2026-08-24,glm_explorer ×3 核查 + glm_worker 单 owner 串行 + glm_reviewer):核查回写任务书——Timeline 实态 eager children 全量物化(旧快照 uniform_list 措辞不成立)、DiffView 不存在且无消费面按红线留波 D、F44 不可溯源改实态登记。基准先行(design/README.md §8.4 + gui-design.md §6)。拆分:mod.rs 1950→824(<900 达标),新增 timeline/timeline_entry/approval_card/input_area/inspector/task_rail 六模块;Timeline 改 gpui list() 变高虚拟化(Bottom 钉底,reset 驱动,跟随/回底语义重映射不回归,审批卡作末项,Entry 菜单 close-on-reset);TaskRail 长标题 .truncate()(本仓首个 TextOverflow 消费点);菜单语义与禁动符号三件套字节等价(审查实证);FollowScroll 收窄终端专用,死方法 reset 删除。验证:desktop 28/28 三轮绿 + check 绿,probe-smoke r8csmoke EXIT=0(签名同波 A/B),真窗口启动截图实证 Connected 态渲染;冻结面零 diff。审查 changes-needed→P2-1/P3-1/P3-3 主代理同波修复;P3-4 Entry 菜单滚动卸载短暂失联登记并入波 E K-03;渲染面滚动/回底/锚点/truncate 四例人工验收并入 K-03。未提交 |
| R8 波 D 进度 | 波 D 收口(2026-08-24,glm_explorer ×3 核查 + glm_worker ×2 并行 + glm_reviewer 双轮):用户拍板 Q1=A(Changes 只读,git_stage/HunkStageService 顺延 ADR 候选,K-04 🟡 部分交付)、Q2=A(「@」host 端到端,补全 query 顺延候选);基准先行(design/README.md §8.5 + gui-design.md §6 两行 + 任务书 §2/§3/§4 按拍板改写)。轨 a(apps/desktop):Inspector 三页签(Changes/Terminal/Resources),新增 ui/changes.rs(705:Files/Summary/DiffView hunk 语义着色 + font::MONO=Menlo 显式等宽 + 全仓首个 overflow_x_scroll/ActivityPopover 摘要行点击定位/session_mismatch banner 与 popover 提示)与 ui/resources.rs(210:MCP 只读表),theme 补 MONO 与两 metrics,mod.rs 824→1031;轨 b:protocol registry mcp_list gui.available 翻 true(帧 golden/schemas 零 diff),app QUERY_HANDLERS + query_mcp_list(fail-soft),run_start 接线 expand_at_refs(附件独立 Text part,fail-closed,零 wire 变更),probe +3 场景 13 全 PASS。验证:desktop 41/41 绿(含 session_mismatch 单测)、cargo check -p pawork EXIT=0、probe-smoke r8dprobe EXIT=0、真窗口冒烟逐项截图实证(Changes 四面/Resources 行/「@」bubble 双 part/断线 fail-soft)。审查双轮:P2-1 失配标注、P2-2 本表指针、P3-1 close_open_menu ×2、P3-2 §8.5 等宽措辞已修,二轮 pass;提交前主代理复核修 P1(run_start 先 expand_at_refs 再登记 ActiveGuiRun,失败不留幽灵 run)。登记 ROADMAP §4 五项(HunkStageService ADR 候选/「@」补全/已加载规则出口/DiffView 横滚入 K-03/mod.rs 1031>900 波 E 定夺);冒烟残留已清理。未提交 |
| **下一波次** | R8 波 E(K-03 人工验收 + gui-design.md 收口(组件清单/S12-CR09 已修项复核:CR09-02 错误上屏、CR09-04 focus 恢复等不回退);波 A–D 登记的人工验收项汇总(截图对照/键盘走查/渲染面行为/DiffView 横滚)+ mod.rs 1031>900 行数定夺;人工 + docs;串行(用户参与)) |
| 阻塞 | 无 |

自动选择以本表为准,再用 ROADMAP / 任务书 / 工作区实态交叉校验。三者冲突时:**工作区实态 > 本表 > ROADMAP 状态列**;更新本表使三者一致后再开工。

---

## 4. 选任务规则

一次开启只做**一个波次**(任务书「波次拆分」里的 0/A/B/C/…)。做完即收尾,不自动跨入下一波。

1. 读 ROADMAP §2。硬前置阶段必须为 🟢;若当前阶段 ⚠️,停止并报告阻塞。
2. 取第一个非 🟢 的主干阶段(R0→R9),再按该任务书选**最早未落地的波次**;与 §3 指针、工作区实态交叉校验。
3. **ADR 闸门**:R0 波 0、R1 波 A、R6 波 0、R7 波 0 产出的 ADR 须用户确认(Accepted)后,同阶段后续波次才可开工;主代理不得代替用户拍板破坏式决议。
4. **跨阶段并行**只在 ROADMAP §2 依赖满足、写入集不相交、且用户明确要求时开第二条线(如 R7 ∥ R3–R6;R2 ∥ R3)。R3→R4→R5→R6 都触 `crates/app`,默认串行。
5. 用户覆盖(「做 R2 波 B」)立即生效。
6. 在聊天里用三行声明后立刻进入 §5(不必等确认):本次波次 + 一句话;子代理模型;写入集。

---

## 5. 主代理执行流程

未指定子代理模型 → **停在 §4 第 6 步之前**,向用户要 §2 模板中的那一行。

### 5.1 开启核对(主代理亲自读,不派发)

按 [task-guide.md](docs/task-guide.md) §2:任务书全文、ROADMAP 依赖与 ADR 状态、[design.md](docs/design.md) §3.2 本波相关冻结契约、[v2-summary.md](docs/v2-summary.md) §4/§5(契约与 S13 拍板不可回退)、写入集各包根 `MODULE.md`(只读写入集;禁止一次读完 21 份;不知道包时再开 [docs/code-map/README.md](docs/code-map/README.md);跨包才读 hotspot 一篇)。需要真实 key 的波次缺凭证即 fail-closed。

### 5.2 并行核查(只读,2–3 路同时派发)

V3 任务书均带 2026-08-18 分析的证据(路径 + 行号),但**执行时实态可能已漂移**(前序波次会改变消费者/依赖/行数)。写设计前按 §8.1 骨架并行派出核查子代理,默认三路:

| 路 | 核查什么 | 目的 |
| --- | --- | --- |
| C1 实态核查 | 任务书证据逐条重验:消费者、反向依赖(`cargo metadata`/`cargo tree`)、行数、调用点 | 证据过期即报告,不带病执行 |
| C2 契约面 | 本波触及的 golden/serde 形状/schema/协议帧清单与所在测试 | 圈定「改前必须先有 golden」的面 |
| C3 影响面 | 写入集之外会被牵动的 use 路径、测试、文档、断言(deny-list、红线测试) | 收尾清单来源;防漏改 |

约束:只读;回传带路径 + 行号;发现任务书与实态冲突时以实态为准并回写任务书。范围小的波次(单包清理)可减为 C1 一路或主代理自查。

### 5.3 本波实现设计(主代理写)

核查齐后,主代理在**本会话**写「本波实现设计」(结构化消息,默认不新建 markdown):

1. **目标 / 非目标**:对应任务书该波 +「明确不做」。
2. **事实源**:归档/合并/改写的具体路径;保留清单(不动项)。
3. **契约**:涉及的冻结契约与 golden;宁可字段闲置,禁止顺手裁剪。
4. **写入集**:允许触碰的目录/包;契约文件单一 owner。
5. **验证**:默认只列写入集 `cargo test -p <crate> --offline --lib --tests`(一条 Cargo 进程)。protocol golden / probe / spawn_e2e / desktop / `cargo check -p pawork` 仅当对应文件确有改动、且只由主代理收口跑一次。合并/归档波才加 `cargo tree` 无环/闭包不膨胀。是否需真实冒烟单独标注。
6. **派发图**:并行 ×N 的每路写入集;串行波主代理自做或只派一个实现子代理。

需要 ADR 或与冻结契约冲突时,先问用户再实现。设计默认留在会话;发现任务书缺口由主代理改任务书。

### 5.4 按波次实现

- 核查结束再实现;不要边查边写。
- 并行度严格按任务书该波标注;写入集互不重叠;契约文件与装配收口(`crates/app`、`apps/pawork`)不并行。
- 归档动作统一走「移出 workspace + 删除源目录」,git tag `v2-final` 已兜底(R0 波 0 打 tag);不把归档代码复制到仓库其它角落。
- 每个实现子代理用 §8.1 骨架(角色=实现)+ 设计切片 + 写入集边界;子代理之间禁止改同一文件。

### 5.5 本波收尾(主代理)

1. 跑本波写入集对应 `cargo test -p <crate> --offline --lib --tests`(多包可一次多个 `-p`,仍是一个 Cargo 进程,不用 `--workspace`)。审查者读 worker `/tmp` 日志,不再编译;主代理收口不重复 worker 已绿的同一条命令。protocol golden / probe / spawn_e2e / desktop / `cargo check -p pawork` 仅当对应文件确有改动时由主代理加跑一次。合并/归档波补跑 `cargo tree` 断言与红线测试。
2. 更新本文 §3 指针;阶段仍有剩余波次则 ROADMAP 标 🔵。
3. 最后一波跑任务书退出标准清单(含真实冒烟项),ROADMAP 标 🟢。
4. 简式报告(task-guide §4 第 5 条):写入集、验证、登记项;未跑全量门禁属当前路线正常状态。
5. 不提交、不推送,除非用户当场要求。

---

## 6. 并行与子代理纪律

- 文档、指针、设计、ROADMAP/任务书勾选:**主代理写**。
- 核查可并行(≤3 路);实现按任务书该波并行度(通常 1–3)。两阶段不叠加。
- 写入集以包/目录为界互不重叠;一次开启只派本波,不预派下一波。
- 子代理同样受 task-guide 全文约束;提示词写明「禁止越写入集、禁止改冻结契约形状、禁止 git commit」。

---

## 7. 子代理模型

开启提示词里的「子代理模型」作用于**所有** `Task` 子代理(核查 + 实现)。主代理用当前对话模型,不擅自更换。

本文不映射具体模型:用户写的模型标识由主代理**原样**落入 `Task`(落在哪个参数、取什么值以当前宿主为准),不猜测、不替换、不查表。想与主代理同模型写 `inherit`。宿主无法识别用户指定值时提问,不猜测;禁止核查与实现使用不同模型,除非用户在「临时约束」写明。

---

## 8. 统一提示词

所有子代理用同一骨架,只替换「角色 / 范围 / 产出 / 禁止」四段。模型按 §7 传入 `Task` 参数,不写进 prompt。

### 8.1 骨架

```text
你是 Pawork V3 的〈角色:核查 | 实现〉子代理。只做本提示词里的范围。

规范(纪律全文,必须遵守):
- docs/task-guide.md
- 仓库根 AGENTS.md
- 写入集各包 MODULE.md(实现前必读;禁止读未列入写入集的包;地图不是事实源,冲突以源码为准)
- 跨包热路径才读 docs/code-map/hotspots/<一篇>(Agent loop / GUI Connection Protocol / 事件持久化与重放 / 凭证与脱敏)

任务:
- 阶段任务书:plan/R<N>-*.md
- 波次:〈波 X:一句话〉
- 设计切片:〈实现角色必填——粘贴主代理设计中属于本路的部分;核查角色写「无,先于设计」〉

范围:
- 〈核查:只读路径/命令清单;实现:允许写入的包/目录清单〉

产出(完成后一次性报告):
- 核查:逐条证据核验结果(路径+行号),实态与任务书的差异,契约/影响面清单
- 实现:实际写入文件、验证命令与结果、未做项、发现的计划偏差

禁止:
- 超出范围的文件改动或无关重构
- 改变冻结契约的 serde/磁盘/线上形状(字段可闲置,不可顺手删减)
- git commit / push / 改 git config / git tag
- 把 Secret 写入仓库或日志
- 运行 cargo --workspace / clippy 门禁 / cargo clean
- 并行轨同时跑 cargo(会锁 target,出现 Blocking waiting)
- 审查者或主代理重复编译 worker 已绿的同一条 cargo 命令
- 默认跑 protocol golden / probe / spawn_e2e / desktop / cargo check -p pawork(除非本波实际改了对应文件)
- 核查角色:任何写入;实现角色:开始前改设计、碰契约面(除非写入集明确包含)
```

---

## 9. 与 task-guide 的分工

| | `v3_plan.md`(本文) | `task-guide.md` |
| --- | --- | --- |
| 何时读 | 每次开聊最先读 | 核对、进行中、收尾时遵守 |
| 选哪一波 | §3–§4 | 不负责 |
| 核查 → 设计 → 派发 | §5–§8 | §7 只给并行原则 |
| 红线 / 契约 / 测试 / key / 报告格式 | 引用 | 事实源 |

窄任务(例如「只修一条 golden」)可直接用 task-guide §1 的最小提示词,不走本文编排。**阶段波次开发默认走本文。**

---

## 10. 主代理自检清单(派发前)

- [ ] 子代理模型已由用户指定,且能落到 `Task` 参数
- [ ] 本次恰好一个波次,写入集已写清
- [ ] 写入集各包 MODULE.md 已读(禁止一次读完 21 份)
- [ ] 硬前置阶段 🟢;本波所需 ADR 已 Accepted(R0/R1/R6/R7 闸门)
- [ ] 核查回传后再写设计;证据漂移已回写任务书
- [ ] 设计未破坏冻结契约;冲突已升级用户而不是自行拍板
- [ ] 实现并行度与任务书一致;契约/装配未被拆并行
- [ ] 收尾会更新本文 §3,且不会顺手开下一波
