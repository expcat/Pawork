# CU-19 · GUI 截图证据与停止状态

> 状态：已实现；自动门禁通过；真窗验收覆盖证据区空态与 observation_list 真实往返，观测行 / 停止状态机待活动 run（环境阻塞）。前置：[CU-09](cu-09-ownership-cancellation.md)、[CU-10](cu-10-action-observe.md)、[CU-17](cu-17-gui-observation-protocol.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

让用户查看实际操作前后证据，并可靠停止后台任务。

## 当前依据与读取范围

现有图片附件有预览组件，但 computer 工具截图被 Timeline 文本投影丢弃；缺少目标观测和停止状态的完整展示。

[GUI 设计](../gui-design.md)、[desktop crate Spec](../spec/crates/desktop.md)、[client Spec](../spec/crates/client.md)、[protocol Spec](../spec/crates/protocol.md)。

## 实施范围

- 复用图片预览显示目标、时间、动作状态和新截图，历史观测与当前观测清楚区分。
- 停止入口通过 Host 取消相应任务/目标，区分停止中、已停止、派发后观测失败与结果未知。
- 仅以 run 终态确认已停止，不能点击即显示成功；GUI 关闭继续遵守不取消 Core Run 的现有合同。
- 有界加载、重连、切任务和键盘操作保持证据身份，不在仓库保存截图或复制整套媒体界面。

## 完成条件与验证

- 用户能核对目标和操作结果，图像与持久事件及新观测一致。
- 停止后无后续派发或残留输入，界面状态与 Host 释放结果对应。
- 真实窗口、外部文件/Host 结果和必要协议回归共同验收。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

2026-10-07 实施完成。

**实现**：Desktop「后台操作」页（apps/desktop/src/ui/computer.rs + controller/computer.rs）经 pawork-client 走 GUI API 1.27 的 observation_list / observation_image_read（只读持久事件账本，不触发新截图或输入），run_cancel 承载停止派发；目标区与证据区共栏单一滚动，观测行按新→旧排序、chip 区分「当前 / 历史」（wire 恒 historical，新近度由客户端判定），副行只显示观测自带 run 标识（观测不携带目标锚点，不与目标卡对齐冒充归属），选中身份在刷新 / 重试 / 断线重连中保持，用户停在最新时新截图自动跟随、手动选历史不抢；列表查询恒以单条目探测页锚定当前 head 再读尾窗（head 可能已前进，按旧 head 算窗会漏新事件），尾窗内被单页字节预算截断时按游标同代次有界续读；图像分块读取校验偏移连续 / Base64 / 总量≤申报，收齐后魔数嗅探 + image crate 真实解码校验，整图超 16 MiB 预算不派出，失败 fail-closed，解码图像只进内存缓存（4 张有界淘汰，淘汰与换会话复位连同 GPUI 全局 asset 一并移交清理，不写仓库）。停止为状态机（Dispatching → Confirming → Stopped / ObserveFailed / Unknown）：run_cancel 回执 Ok / Err（Host 正常成功回执即 Accepted）都只进确认（unknown_run 可能已终态），已停止仅由 run 终态事件确认——空列表不裁决释放（Host 取消即移除 registry 登记，清理可能仍在执行），列表重查只处理被拒且仍持有回落为可重试错误与新 run 接管退役 Stopped，确认查询失败与确认前断线分别如实标注；GUI 关闭不取消 Core Run 的合同未动。观测区可见性不耦合版本门（旧 Host 恒无数据不显示误导空态）。i18n 新增约 27 键（en / zh），AX 覆盖观测行、preview、查看 / 重试与停止按钮。Desktop 新增生产依赖 base64 与 image（树内已有传递依赖，显式声明不引入新包）。

**Review 修复**（2026-10-07，指挥线程 8 条全改）：分页续读按 Host 游标语义重写（!complete + next_sequence 同代次续读，探测页无条件锚定 head）；停止确认收敛为仅 run 终态事件（空列表、被拒回执都不冒充已停止，迟到回执不回退 Stopped）；run_cancel Accepted 回执纳入成功分支；图像 eof 加真实解码校验与 16 MiB 整图预算；淘汰图像连同 GPUI 全局 asset 移交 remove_asset；观测行 / AX 去掉按 run 匹配目标卡的归属冒充，副行只显示 run 标识。

**第二轮 Review 修复**（2026-10-07，3 必修 + 2 建议全改）：断线重连 Replay 补发的 run 终态同样收敛停止状态机（与 live 事件同语义，ui/mod 新增单测）；截图解码校验移出 UI 线程至后台执行器（解码期间只登记在途标记，回执按（身份, 代次）匹配落地防旧代次串扰），并增单帧像素（16M px）/ GIF 帧数（60）/ 累计像素（48M px）预算，GIF 逐帧真实解码不再只验首帧（首帧有效、次帧 LZW 损坏拒收）；「正在停止任务」占位符补全文案；desktop.md 测试说明同步为「停止仅由 run 终态确认；空列表不确认停止」与后台解码口径。ui/computer 测试改写为后台 Decode 语义并新增解码预算 / GIF 全帧校验单测（共 10 个）。

**第三轮 Review 修复**（2026-10-07，1 必修）：解码回执与当前选择的竞态收口——旧解码结果按截图身份隔离（失败 / 嗅探错误只落到选中身份，成功按身份入缓存），解码回执落地后自动续载当前选中项（解码占位期间切选 / 刷新跟随到的新证据不再滞留，ui/computer 新增 view 层单测以超预算本地拒收证明续载发生）。

**实际命令**：bash scripts/test.sh desktop（第一轮 271 / 第二轮 273 / 第三轮修复后 274 passed，0 failed，含 runtime_shaders）。

**定向回归**：ui/computer.rs 新增 4 个测试（总 9 个）——观测探测页 / 尾窗 / 预算截断续读保持选中身份并跟随最新（探测页带真实 Host 游标形状）、图像分块校验（偏移 / Base64 / 总量 / eof 真实 PNG 解码 + 魔数前缀垃圾字节解码失败）fail-closed、停止仅由 run 终态确认（空列表仍 Confirming、迟到回执不回退 Stopped、被拒仍持有回落错误、新 run 接管退役 Stopped、确认前断线 Unknown）、AX 钉板钉观测行实测几何发布（value 携带 run 标识）与无 client 时停止按钮禁用且被拒 Press 不产生停止状态。

**真窗验收**（2026-10-07，证据 /tmp/cu19/evidence.md，截图不入库）：no_workspace 与 no_run 两态像素确认证据区渲染（「观测证据 · 0 条观测 · 暂无观测证据。」），AX 组 help "ready · 0 targets · 0 observations" 证明 observation_list 对真实 Host 的成功往返（会话切换后重查同样成功）。

**遗留缺口**：观测行（真实截图分块加载 / chip / 跟随）与停止状态机真窗未端到端验收——两条路径都需要活动 computer-use run 产生截图与登记目标，GUI RunStart 被用户全局配置 opencode-go disabled_models（含 glm-5.3-flash）按 ADR-055 D4 fail-closed 拒绝（[CU-18](cu-18-gui-target-approvals.md) 已记录同一环境事实）；按验证口径未换模型、未改用户配置，该部分由单测与 AX 钉板覆盖，待 CU-20 目标登记与 native 操作接入后一并人工验收。

同步更新：[GUI 设计](../gui-design.md) §8、[desktop crate Spec](../spec/crates/desktop.md) 模块地图与 §4.7/§7。
