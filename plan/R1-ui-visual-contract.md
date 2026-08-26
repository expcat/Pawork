# R1 — 视觉合同、固定 fixture 与 UI 测试基座

> 状态：🔵 进行中（Wave A/B 已收口；Wave C/D 未开始）
> 目标：在修改生产 UI 前，把“设计稿是什么、测试输入是什么、如何证明一致”冻结成可重复执行的合同。R1 不以主观截图点评代替门禁，也不以写死演示数据换取相似度。

## 1. 输入与边界

事实源按以下顺序使用：

1. 架构红线、GUI Connection Protocol 和真实 capability；
2. [Desktop GUI 设计](../docs/gui-design.md)；
3. [design/README.md](../design/README.md) 与三张 v3 定稿图；
4. [UI 视觉复审与 99% 合同](../docs/UI_Review.md)；
5. [Agent UI 参照调研](UI-reference-research.md)，只用于操作方式与验证方法。

本阶段不修改设计稿，不重做产品能力，不把竞品品牌、云端能力或不存在的数据移入 Pawork。发现设计稿与真实 capability 冲突时，先记录冲突并给出真实的 hidden / disabled / unavailable 方案；需要改变 design 时停下请求用户批准。

## 2. Wave A — 冻结视觉合同

- 将三张 v3 图归一为 `1440×1024` 内容视口，记录裁切、缩放、色彩空间和 macOS titlebar 处理方式。
- 建立 State A（Timeline + Inspector 展开）、State B（Timeline + Inspector 折叠 + ActivityPopover）、State C（Projects）的可见组件树、z-order、锚点和状态表。
- 对 TaskRail、Workspace、Inspector、Header、Timeline、Composer、StatusBar、Popover 量图，记录宽高、间距、行高、字阶、图标槽、圆角、描边和 surface token。
- 解决现有范围冲突并写回设计事实源：
  - Activity 触发器固定在 Workspace 右上，Popover 向下展开且不得覆盖 Composer；
  - Inspector 中 `Resources` / `Add tool` 只有在 Host capability 存在时出现；定稿图中的 `+` 不得被不等价入口替换；
  - `1080×720` 的响应式规则必须保留主操作和可见焦点，不接受固定宽度溢出。
- 建立“结构一票否决 + 分区 SSIM + 人工 overlay”的判定顺序；动态遮罩只覆盖值，不覆盖容器、基线、密度和状态图标。

交付物：三状态量图表、组件/状态 manifest、reference 截图、遮罩定义和逐项 checklist。

### Wave A 收口记录（2026-08-25；合同复核 2026-08-26）

**已交付**（证据根目录 [docs/ui-review/](../docs/ui-review/README.md)）：

- 三状态归一参考图 [state-a](../docs/ui-review/state-a/reference.png) / [state-b](../docs/ui-review/state-b/reference.png) / [state-c](../docs/ui-review/state-c/reference.png)（1440×1024，全画布 LANCZOS 缩放、无裁切；源图为无 ICC 的 RGB，按 sRGB 解释且不做 profile conversion；traffic lights 条带计入内容视口；记录：[normalization-report.json](../docs/ui-review/normalization-report.json)，脚本 [ui-normalize-reference.py](../scripts/ui-normalize-reference.py)，原图/归一图抽样色值一致）。
- 三份量图表 [state-a](../docs/ui-review/state-a/measurements.md) / [state-b](../docs/ui-review/state-b/measurements.md) / [state-c](../docs/ui-review/state-c/measurements.md)（区域几何、逐组件槽位、像素法字阶 ±1px、颜色取样对照 token、8px 基线核对、冲突表；证据 crops 共 59 张）。
- 组件/状态 manifest [component-manifest.md](../docs/ui-review/component-manifest.md)：45 个组件条目（三态组件树、L0–L4 z-order、浮层锚点、状态表、能力映射 real/partial/honest-hidden/unavailable/结构缺口，演示数据全部标注非合同）。
- 遮罩定义：三态 mask.json 共 80/58/62 条（其中各 4 条为窗缘 reference artifact，其余动态文本按单行紧遮）；Diff 保留 gutter、+/− 前缀、行距和语义底色，当前无 `geometry-drift` 遮罩。zones.json 三份各 9 个分区，以左右锚点与最低共同覆盖率表达合同几何差异，并以 `max_mask_fraction=0.35` 拒绝过度遮罩（当前最高约 31%）。
- 逐项 checklist：三态 checklist.md（结构/几何项按量图填写，交互/AX 项 BLOCKED 待 Wave C/D）。
- 判定管线 [ui-visual-diff.py](../scripts/ui-visual-diff.py)：结构一票否决 → 分区 color SSIM≥0.99 → 人工 overlay；11×11 box-window SSIM 逐 RGB channel 计算并取最低通道，遮罩在统计前双图中性化；输出 raw overlay、masked heatmap/report 与逐 zone 对齐证据，无需 ImageMagick / scikit-image。
- D-01/02/03 已按 UI_Review §4 决定回写 [design/README.md](../design/README.md) §5.1（D-01）/ §8.5（D-02）/ §7（D-03）与 [docs/gui-design.md](../docs/gui-design.md) §3.3（D-01）/ §6（D-02+D-03）；[docs/spec/crates/desktop.md](../docs/spec/crates/desktop.md) §8 仅登记 D-01 的现实现偏差说明（该 Spec 镜像源码，D-02/D-03 目标态改动随后续实现波回写）。

**验证**：归一化尺寸与色彩保真（抽样 #061219 一致，重复生成 hash 不变）；mask/zones JSON 合法且矩形在界内；diff 管线 14 个定向回归覆盖自比、跨态、等亮度换色、遮罩窗口边缘、遮罩旁结构漂移、对齐后 heatmap、锚点、最低覆盖率、分区/全图/映射并集过宽遮罩、策略上/下限不可放宽、`covers` taxonomy/窗缘约束、非 RGB/ICC 输入拒绝，以及陈旧 PASS/后期失败部分证据清理；三态自比均 9 zone PASS，A→B 跨态按预期 FAIL。新色板按允许的文字角色 × surface 组合核算，5 组小字/按钮文字均达到 4.5:1。State C 量图一处标题误读（"Projects"→"Pawork"）及 State B Header 整行误量已由主代理复核修正。

**图像缺陷记录（以 design 规则为准，不问用户）**：State A 图 rail 选中行无背景填充（§3.6 与 State C 均要求有）；三图窗缘 1–2px 亮线为 ImageGen 伪影（已入 frame 遮罩）。State B Header 经复审确认实际具备 branch/终态元信息，先前“缺失”记录源于把整段误量为标题，已纠正量图、checklist 与 mask。

**合同仲裁（2026-08-26 用户拍板，两项均按主代理建议定案）**：

1. **几何合同**：保留 [design/README.md](../design/README.md) §2 定稿值为实现合同（rail 288 / inspector ~440 / composer 88–94 / statusbar 24 / popover ~320），三张图作近似视觉语言参考；SSIM 侧以 zone reference/current 矩形、左右 anchor 与 `min_coverage` 表达图像偏差（[ui-visual-diff.py](../scripts/ui-visual-diff.py)，R1-D 回填 current）。`geometry-drift` 只允许纯边缘背景，当前三态不使用；结构与几何硬门禁按合同值 + UI_Review §0.1 容差由 checklist 与 U2 实测执行。Composer、Header、Inspector/Popover 均拆出左右锚点分区，避免共同裁切丢掉整侧控件。
2. **色板**：按量图实测并受 WCAG 小字/按钮文字对比度约束重定 theme token，新冻结目标表见 [design/README.md](../design/README.md) §2.1（bg #07121a/#061219/#0e171d、surface.raised #10171c、text.tertiary/placeholder #7f7f7f、accent.hover #3270e8、success_hover #438251 等；派生值已标注，R2 落 theme.rs 并用真实组合色复验）。

**未做（按计划属后续 Wave）**：真实 fixture（Wave B）、GPUI AX/真窗口 spike 与 U0–U3 选型（Wave C）、State A 闭环与故意漂移捕获（Wave D）。

## 3. Wave B — 固定真实 fixture

定义一个测试专用、确定性的 Host 数据集，经正式协议和 projection 进入 Desktop，至少包含：

- 至少三个 workspace、两个 project、跨日期 task、选中/未读/运行中/需审批/失败/完成状态；
- user/assistant 多段文本、列表、流式片段、tool pending/running/succeeded/failed、approval、error、cancelled 与 completion summary；
- 至少两个文件的真实 diff，覆盖新增/删除/长行/横向滚动；
- Terminal 创建、输入、输出、resize、停止；
- Inspector 展开/折叠、Activity 摘要、断连与重连恢复；
- 缺 capability、空列表、超长标题、长会话和窄窗等边界状态。

fixture 必须使用隔离的 `PAWORK_DATA_DIR`，固定 seed、时间、workspace 路径替代符和数据版本；通过 scripted provider/tool 与小型确定性 PTY fixture 生成数据，不访问网络或真实凭证。同步提供 `timeline_stable`、`approval_visible`、`drop_socket`、`host_restarted`、`replay_complete` 等 barrier，禁止用固定 sleep 猜稳定时机。不得进入正式用户数据，不得含真实 Secret，不得在生产 UI 中出现 `if demo` 分支。

交付物：fixture schema/seed、启动命令、清理边界、projection 断言和敏感信息扫描。

### Wave B 收口记录（2026-08-26）

**已交付**（仓内资产 + dev-only 工具链）：

- 种子器与 fixture host：[crates/app/src/devfixture.rs](../crates/app/src/devfixture.rs)（默认关闭的 `ui-fixture` feature / `cfg(test)` 双门控 + `#[doc(hidden)]` pub 模块，只用 SessionStore / CheckpointService 公开 API 与文件/git 写入，不依赖 testkit）+ [crates/app/examples/ui_fixture.rs](../crates/app/examples/ui_fixture.rs)（`required-features = ["ui-fixture"]` 的 dev-only example，冻结四子命令 `seed / serve / self-check / snapshot-dump`，手动 argv）。
- 数据集 [fixtures/ui/seed.json](../fixtures/ui/seed.json)（schema v1）：3 workspaces（alpha-app git 含 diff / beta-lib git 干净 / gamma-notes 非 git）、7 sessions 跨 Today / Yesterday / Previous 7 days / Earlier 四桶、五类终态（completed / failed / cancelled / pending_approval / tool_failed）、200+ 字符超长标题、≥50 条目长会话、alpha 仓 4 文件 diff（modified×2 其一 >200 字符长行 / added / deleted）。263 事件经 SessionStore 真实写入路径落库，显式时间戳锚定 `FIXTURE_NOW_MS = 1767225600000`，`--now-ms` 可整体重锚；固定锚点下事件 payload 与 snapshot-dump 输出逐字节确定。
- serve 模式 MockProvider 前缀分派（默认 3 chunk / `fixture:hang` / `fixture:fail` / `fixture:tool`=read_file），只存在于 dev example；确定性 PTY [fixtures/ui/pty-fixture.sh](../fixtures/ui/pty-fixture.sh)（固定 banner + 回显 + `exit` 收尾，两次运行 sha256 一致）。
- Barrier 文件合同（禁止固定 sleep）：`host_ready` / `host_restarted` / `drop_socket.request|done` / `serve_stop.request` / `replay_complete` / `timeline_stable` / `approval_visible`；Desktop 侧 env `PAWORK_UI_BARRIER_DIR` 钩子（未设置零开销、tmp+rename 原子写、路径只在 barrier 目录内拼出、目录缺失惰性创建）。
- 驱动脚本 [scripts/ui-fixture.sh](../scripts/ui-fixture.sh)（seed/serve/desktop/drop-socket/restart-host/self-check/down/clean/scan；先离线 build 再直启最终二进制，PID 与完整 root/命令形态精确对拍；clean 只认 `.pawork-ui-fixture` marker）+ 敏感信息扫描 [scripts/ui-fixture-scan.py](../scripts/ui-fixture-scan.py)（七类规则，命中 exit 2；socket/FIFO/symlink 不读取，但任何形态的 `auth.json` 都 fail-closed）。
- projection 断言：[crates/app/tests/ui_fixture_projection.rs](../crates/app/tests/ui_fixture_projection.rs)（devfixture 种子 → 真实 GuiHostAdapter snapshot/timeline，断言值全部取自 seed.json）+ desktop `projection.rs` 新测试消费 [fixtures/ui/expected/snapshot.json](../fixtures/ui/expected/snapshot.json)（snapshot-dump 归一化 volatile 字段，再生步骤见 fixtures/ui/README.md）。

**关键设计拍板**（主代理冻结，备查）：

1. 静态数据集经 SessionStore 公开 API 显式时间戳写库，而非 engine 实时跑批：engine 事件时间戳取系统时钟，无法构造跨日期桶；写库路径与 storage/protocol golden 同源（真实持久化路径，非伪造字节）。live 动态面（running / hang / fail / tool）由 serve 模式 MockProvider 经真实 engine loop 产生，满足任务书「scripted provider/tool 生成」的要求面。
2. fixture host 以 app crate 的 feature-gated example 承载：`ui-fixture` 默认关闭，devfixture 与对应 integration test / example 均显式 opt-in；testkit 永不进生产二进制闭包，不新增包、不新增生产依赖。
3. Desktop barrier 钩子为 env 门控观测信号，不是 demo 数据分支；生产 UI 无任何 `if demo`。

**验证**：`cargo test -p pawork-app --offline --lib --tests --features ui-fixture` 全绿（lib 152 + 各 test bin，含 `ui_fixture_seed_to_host_snapshot_and_timeline`）；默认 feature 的 `cargo check -p pawork-app --offline` 通过；`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 43/43（含 `barrier_sink_writes_and_removes_contract_files` 与 `ui_fixture_expected_snapshot_rebuilds_groups_and_status`）；真实链路 `seed → serve → self-check（Resume Replay 5/5）→ drop-socket×2 → self-check → snapshot-dump → scan（0 命中）→ down（serve_stop 优雅停机、serve exit 0）→ clean` 在全新 `/tmp` 短 root 全通过；另以全局 `gpgsign`/hook 与 `GIT_DIR`/`GIT_WORK_TREE` 注入重跑 seed，外部 hook 未执行且仓库仍精确落在 fixture root；陈旧 PID 指向无关进程时拒绝发信号；`python3 scripts/test_ui_fixture_scan.py` 20/20 OK。

**审查**：GLM reviewer 门禁有条件放行（无 P0/P1）。P2-1 `serve_stop.request` 原为死代码 → `stop_host` 已接线（barrier 优先、信号兜底）并实测优雅停机；P2-2 README 三处失真（diff 文件数 / fixture:tool / pty quit）→ 已修正；P2-3 gamma-notes 经现有 wire 不可达 Desktop（snapshot workspaces 段只携带主 workspace）→ 记为已知缺口，扩段属 wire 演进须 ADR；P3 timeline_paging 窄路径卡死 → controller 发 OperationFailed + Disconnected 复位；P3 barrier 目录不建 → `BarrierSink::new` 惰性创建。

**二次安全复审**：补齐默认关闭 feature gate；seed 在写入前完成 schema 引用/枚举/相对路径、root 重叠、Unix socket 长度与时间锚点溢出/范围校验，并隔离全局 Git 配置与 `GIT_*` 路由环境；marker 采用 `preparing → ready` 两阶段；driver 改为 build 后直启最终二进制并精确核对 PID command line（信号升级逐次复核），过长旧 root 仍允许 `down/clean`；drop/replay/timeline/approval barrier 每轮失效陈旧信号且重复 drop 可用；扫描器不读取非 regular 项，但 `auth.json` symlink 仍按文件名拦截。末轮 GLM 路由因后端无法解密任务失败两次，未继续消耗重试；上述项以静态差异检查、定向回归和真实链路收口。

**未做（按计划属后续 Wave）**：GPUI AX/真窗口 spike 与 U0–U3 选型（Wave C）；State A 闭环与故意漂移捕获（Wave D）；Desktop 真进程的 `timeline_stable` / `approval_visible` 端到端观测（Wave D 真窗口链路）。

**顺带发现**：transport local_unix 的 accept 跨 await 持锁会导致 GuiServer `listener.close()` 死锁（example 以 abort accept 任务规避；生产 `pawork gui serve` 靠 select! 丢弃 accept future 不受影响）——已登记 ROADMAP §4.1（R9）。

## 4. Wave C — UI driver 与可观察性基座

先做最小技术 spike，再冻结分层工具组合：

- U0：domain/event → Desktop projection 的状态测试；
- U1：优先验证当前 GPUI `TestAppContext` 对 action、focus、key/mouse/scroll/resize、clipboard、deterministic executor 与 layout invariant 的实际支持；
- U2：真 `pawork` Host + 真 Desktop 进程，首选 macOS XCUITest/XCTest 或等价 AX 驱动按语义模拟 click/hover/type/key/scroll/resize/reconnect；外部驱动不得进入 Desktop 生产构建，若“纯 Rust 构建链”边界仍有歧义则先请求架构裁决；
- U3：真窗口 screenshot + ImageMagick（或等价可复现工具）差分、AX audit/VoiceOver、真实 IME 与性能取证；R1 只证明管线能稳定产出证据并能识别当前偏差，不要求当前 UI 达到 99%，也不要求用户签字。

当前调研只证明 `gpui = 0.2.2` 具备若干进程内输入模拟能力，**没有证明**它拥有当前 Zed main 的 AccessKit/完整 AX tree 或 Metal 离屏截图。R1 必须以真 macOS 窗口验证 role/label/value/action/identifier 映射；若 AX 只能看到 Window/traffic lights，R1 直接失败，必须在精确 revision 升级、有限 backport 或等价 AX bridge 中做出有界决策，不能降级为坐标驱动后宣称全功能。

选择标准：能按稳定 identifier/role/name 定位而非只靠坐标；identifier 与本地化 label 分离；能等待明确 barrier 而非固定 sleep；能导出 action trace、窗口几何、截图、AX tree、Host/event log；失败后可单场景重放。GPUI 真实视觉测试若采用主线程/串行机制，必须与“同一时刻一个 Cargo 进程”纪律兼容。

## 5. Wave D — 建立首个闭环

用 State A 的“启动 → 连接 → 选择 task → 检查三栏骨架 → 截图”跑通 U0–U3：

1. 重置隔离数据并启动 Host；
2. 启动 Desktop，按语义等待连接和目标 task；
3. 断言组件树、关键几何、焦点起点和真实状态；
4. 生成 `reference/current/overlay/diff/mask/checklist`；
5. 故意改变一个稳定 token，证明门禁会失败并留下完整证据；随后恢复。

失败产物统一包含：场景名、seed、Git HEAD/工作区摘要、OS/显示参数、窗口内容尺寸、driver action trace、最后 AX tree、Host/event log、current/overlay/diff 和失败断言。

## 6. 退出标准

- [x] State A/B/C 组件与状态 manifest 完整，所有可见组件都能映射到真实能力或诚实不可用态。
- [x] 量图表与 99% 判定脚本/步骤可由另一位执行者复现；主区域不能被全屏空白稀释。
- [x] fixture 经真实 Host/协议/projection 到达 Desktop，隔离、确定、无 Secret、无生产演示分支。（Wave B 收口：GuiHostAdapter 对拍 + Desktop projection 消费 expected snapshot；真窗口端到端属 Wave D）
- [ ] U0–U3 工具路线已以最小闭环验证；GPUI AX/visual 能力闸门有真实结论，稳定语义定位、显式 barrier 与失败证据可用。
- [ ] State A 基线场景可重复执行，且故意漂移能被门禁捕获。
- [ ] 实际验证、未覆盖项和技术限制已回写本文与 ROADMAP；未满足时不进入 R2。
