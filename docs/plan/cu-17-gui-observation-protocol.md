# CU-17 · GUI 观测查询与协议版本接入

> 状态：已实现（P5，自动门禁通过；GUI 真窗验收归 CU-18/19）。前置：[CU-02](cu-02-target-contracts.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

为 GUI 提供有界目标状态与截图证据查询，保留 Host 作为唯一入口。

## 当前依据与读取范围

TimelineItem 没有图片字段，join_text 只保留文本/视频说明；持久消息中的图片不能直接在 Timeline 展示。现有 ArtifactRead 提供有界读取，但模型 Artifact 图片解析并不完整。

[protocol Spec](../spec/crates/protocol.md)、[client Spec](../spec/crates/client.md)、[app Spec](../spec/crates/app.md)、[storage Spec](../spec/crates/storage.md)、[架构](../architecture.md)。

## 实施范围

- 先复用现有查询/图像组件能力，补最小观测引用与受限读取；不把巨幅 Base64 直接塞进每个 Timeline 帧。
- 查询绑定当前授权的 workspace/session 与目标证据，包含实际时间、尺寸和失效/历史状态；不得跨任务取图。
- 目标列表、审批与停止控制只按 CU-02 已裁决契约接入 registry，使用必要的最小新增消息。
- 冻结 wire 演进先完成具体裁决、版本协商、golden 与生成物；旧版本禁用新入口或明确拒绝。
- 模型图片仍走现有已支持编码，GUI 查询不迫使 Provider 改为尚未解析的 Artifact 来源。

## 完成条件与验证

- 有界查询能读到对应持久图片和真实状态，分页/重连不会错配身份。
- 跨 scope、超限、过期控制和旧版本被明确拒绝，历史读取不触发新截图或输入。
- protocol/client/app/storage 的受影响既有协议与持久化回归通过。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

2026-10-07 实现（分支 main，工作区未提交）。

**裁决**：API 1.27 additive、全部 GUI-only、registry since=1.27；旧 minor（<27）在 Host handler 显式拒绝（browser_next 先例），registry 自动门同步生效。TimelineItem / projection 不动——GUI 用 sequence/event_id 在客户端 join。

**实现**：

- protocol：5 条新 wire 消息（observation_list / observation_image_read / computer_target_list 查询，computer_target_approve / computer_target_revoke 命令）；app/computer.rs 全部 wire DTO；MAX_OBSERVATION_READ_BYTES = 64 KiB、MAX_OBSERVATION_LIST_LIMIT = 500 / 默认 200。观测条目身份 = (session 内事件 sequence, part_index)，扫描持久化 ToolExecutionCompleted.result.content 的 Base64 Image part；尺寸取 result.metadata.observation.image_width/height（无则 null）；status 恒 historical。读取按解码字节切片，offset ≥ byte_length、limit==0 或超限明确拒绝；events_on_lineage 返回序必须等于请求序（lineage 外不可读）。
- computer-use：TargetState 视图 + TargetRegistry::target_states()（grants ∪ windows 聚合，occupied/paused 按 anchor 附加，确定性排序）；impl NativeProbe for Box<dyn NativeProbe>；UnsupportedProbe（非 macOS fail-closed）与 platform_probe() 工厂；TargetAuthorizer::grants() 只读迭代。
- app：AppCore 持 targets: Mutex<TargetRegistry<Box<dyn NativeProbe>>>（GUI 路径不触发 probe）；services/run.rs run 终态 close_target_scope（关闭授权 scope，CU-16 契约）；gui_host/handlers/computer.rs 5 个 handler + 分发表 5 行（approve/revoke 幂等；run 必须 active 且属于 session，fail-closed unknown_run；protected bundle → forbidden_target；Website 构造器归一化 origin，回执回显生效 target）；base64_decoded_len / base64_decode_range（RFC 4648 padded，fail-closed；切片只解码覆盖 quad）。
- client：lib.rs re-export 新类型（since 门自动生效）；typegen 三组 schema 同步。

**审查修复**（2026-10-07，6.1 sol 审查 3 必修 + 2 建议修，全部处理）：

1. 终态关闭原子性：TargetRegistry 活跃 scope 登记（open_scope / close_scope），grant 只接受已 open 的 scope、否则 TargetError::ScopeClosed；AppCore close_target_scope 取代 release_target_scope；run_start 注册 run 前 open、spawn wrapper 在 approvals 清理前幂等兜底（覆盖 chat_turn 内 ? 早退绕过终态块的路径）；handler 把 ScopeClosed 映射 unknown_run（"run already reached a terminal state"）。二轮复审收敛内存语义：close 即移除活跃登记，不保留终态记录（原先 closed 集合会随每个已结束 run 永久增长）。
2. typegen 完整性：ComputerTargetListData / ObservationListData / ObservationImageData 补 TS derive 并显式导出（core-api / gui-protocol，headless 不导出）；golden 补 3 个 typed 服务端响应帧（共 8 帧）。
3. 响应有界：observation_list 单页条目序列化字节预算 MAX_OBSERVATION_LIST_RESPONSE_BYTES（512 KiB）事件粒度停止可续读、单事件超限 response_too_large；computer_target_list 条目上限 MAX_COMPUTER_TARGET_LIST_TARGETS（256）之外，二轮复审补完整响应序列化字节预算 MAX_COMPUTER_TARGET_LIST_RESPONSE_BYTES（512 KiB，bundle_id 无长度上限，条目数不足以约束帧大小），超限拒绝。
4. 范围解码：切片只解码覆盖 quad，不整图解码；二轮复审补整串字符集与 padding 扫描（零分配 O(n)）——切片外的非法字符（如 AAAA!!!!）同样整串拒绝，与全量解码语义一致。
5. 状态口径：本文档删「等待人工验收」，GUI 真窗验收归 CU-18/19。

**验证**（2026-10-07，kache wrapper，单 Cargo 进程）：

- Validated: bash scripts/test.sh protocol client app storage computer-use → 27 组 ok（首轮）；首轮审查修复后 bash scripts/test.sh protocol app computer-use client → 全部 ok；二轮复审修复后同命令重跑 exit 0（463s，protocol golden 117 fixture、typegen schema 同步零 diff；app 299 lib 测试；computer-use target 40 项）。
- Targeted regressions: gui_host::tests::computer——主路径（列表条目字段、两次分块读重组图像、status historical、零后端调用）+ 失败路径（跨 session sequence、part_index 越界、limit 0/超限、offset 越界、1.26 拒绝、approve 未知 run/protected bundle、run 终态后 list 清空、revoke 幂等）；首轮补终态关闭竞态 / 预算停止续读 / 单事件超限 / 目标数上限；二轮补 computer_target_list_rejects_oversized_response_bytes（600 KiB bundle_id 超字节预算拒绝）、observation_entries_with_invalid_base64_fail_closed（非法 base64 列表不收录 + 读取 app_error）、extension 整串字符集拒绝（"AAAA!!!!" / "YWJ j" / decode_range 切片外非法字符）。
- 真实窗口 / 真实目标的人工验收未执行（Desktop 消费侧接入另行任务）；截图证据不适用（本任务为 Host 侧查询与登记接入，无 GUI 视觉变化）。

**遗留**：无已知缺口。Golden fixture 计数 109 → 117；architecture §2 app 依赖边新增 computer-use。
