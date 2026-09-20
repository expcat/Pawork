# 测试与门禁重构计划

> 2026-09-20；用户要求：先规划全部分析任务，再分批检查、修改与重构。事实基线为当前工作区；不是按历史测试数恢复旧门禁。本计划不代表已完成分析或验收。

## 1. 目标与取舍

每个保留的测试都应回答：用户或外部系统做了什么、经过哪条生产路径、结果或副作用是什么、哪些边界必须拒绝，以及失败后状态是否完整。真实用途包括 CLI / GUI 操作，也包括协议互操作、持久化恢复、工具执行与凭证安全；不要求所有断言都升级成昂贵的端到端测试。

优先用真实临时目录、SQLite、进程、socket、HTTP 请求驱动生产代码；只在外部服务、时钟或故障注入边界使用替身。替身自己返回预设值、重复实现生产算法、只检查常量/默认值/内部布局、没有效果断言的 smoke，均列为核查候选，而非仅凭名称删除。

每项处置只有五种：**保留、合并、改写、移出日常入口、删除**。删除必须记录无有效行为的证据，或保留测试的具体替代锚点；安全拒绝、持久化/重放、协议兼容不能因慢而直接删除。以场景和缺陷检出能力判断价值，不设测试数量、覆盖率或耗时的任意配额。

不改产品契约，不新增测试框架/生产依赖，不提交或发布。已有 Desktop 未提交改动保留。验证失败先定位，不用重新录制 golden、放宽断言或重复运行掩盖问题。

## 2. 全部分析任务与批次

| 批次 | 分析范围 | 必须回答的问题 | 交付与验收 |
| --- | --- | --- | --- |
| T0 基线 | 24 个 Cargo 成员、feature/target/ignore、脚本、验证文档、现有差异 | 哪些会执行、哪些被隐藏、是否有重复入口/空跑/假通过？ | 可核对的入口清单；区分源码计数与实际执行计数，记录环境依赖 |
| T1 操作与判据 | CLI/GUI → Host → Engine/工具 → 存储/Provider 四条链 | 正常操作、取消/拒绝/失败、重连/重启分别观察什么？ | 用途—边界—测试—入口对应表；明确单测、集成、真实环境各自证据范围 |
| T2 安全与本地操作 | policy、exec、auth、tools、workspace、git | 路径/symlink/Secret/审批/进程/配置导入是否检查实际副作用及拒绝后不变？ | 按包核查记录、精简/改写候选与替代锚点；受影响定向回归 |
| T3 状态与契约 | domain、storage、protocol、transport、workflow、control-plane | 写入→重开→重放、幂等/并发、损坏输入、跨租户和 wire 是否真的被验证？ | 保留冻结契约证据，删除无价值重复；必要时改为操作序列测试 |
| T4 模型与任务运行 | providers、engine、orchestration、testkit | mock 是否仅替代外部边界？流式/续轮/失败/取消/预算是否经过生产逻辑？ | 正常任务与关键失败链的准确断言；重复 fixture/自证测试处置 |
| T5 用户入口 | app、cli、client、pawork | CLI 参数→真实服务→持久结果是否贯通？隐藏集成目标、忽略项是否有可靠入口？ | 真实输入驱动的定向集成验证；环境缺失不得记为通过 |
| T6 桌面与外设 | desktop、terminal、browser、computer-use、UI fixture/AX 脚本 | reducer/布局计算能证明什么？哪些必须有真窗口、PTY、WebView、容器事实？ | 保留有效状态/边界测试；清理常量/映射自证；真实环境清单与未验项分开 |
| T7 门禁重构 | mock gate、Cargo feature 组合、脚本及文档约定 | 是否按受影响用途选择？是否重复编译、吞掉失败、无条件验证无关包、缺环境假绿？ | 少量显式入口，失败非零；静态检查、行为回归、真实环境各自报告；单 Cargo 进程 |
| T8 收口 | 全部处置、引用、文档与执行证据 | 删除后边界是否有锚点？每个批次已检查/已修改/已验证是否区分？ | 差异审查、受影响命令结果、剩余真实环境缺口；不以测试减少数作为成功标准 |

执行顺序：T0 → T1 → T2/T3/T4/T5/T6 分批核查 → 每批最小修改与验证 → T7 → T8。独立范围可并行只读分析，文档由主代理统一维护，Cargo 全程串行。先读各包 Spec，再核对对应生产代码和测试；不以搜索命中代替测试体阅读。

## 3. 每批记录要求

每条处置记录包含：测试/入口路径、真实用途、正常结果、关键边界、判断依据、保留的替代锚点、改动文件、实际命令与结果。未读完不能记为已检查；未执行不能记为已验证。对平台/凭证/图形会话依赖，明确写出待验条件。

门禁的最小原则：文档改动只验链接/diff；行为改动跑相应包/target 和必要 feature；跨包用户操作用已有集成入口；真实 Provider 固定 `opencode-go / glm-5.3-flash`；真窗口像素、真实 OS 隔离与容器输入不能由 mock 自测代替。全量发布门禁不由此次测试重构推定为已完成。

## 4. 进度与基线

- T0/T1 规划、入口盘点与操作判据已完成。T2 全范围审查、处置与六包定向验证已完成。T3 六包测试体已读完，自证项已删/改写（含 protocol 命令计数钉板）。T4 51 个含测文件已读完（报告 /tmp/pawork-audit-T4-complete.md）；stream/framing/default-context 与 Ord/空累加器自证已落地。T5 cli/client/pawork 已读完；app 271 项已按文件与测试名读完，无新删除项。T6 terminal/browser/computer-use 已保留；Desktop 映射自证已再收口（socket 文件名并入 token 路径，工具状态映射并入视图构造）。
- T7 入口已实现并用于各批定向验证。T8 已按当前工作区收口：区分已检查 / 已修改 / 已验证；冻结面键表与 tab_stop、Desktop 握手能力面保留钉板；真实 Provider、Linux/Windows、WebKit 与容器效果仍待专项。
- 当前分支 `main`；已有 Desktop controller/projection/UI 与对应 Spec 的未提交差异，属于本轮开始前基线。
- 没有 `.codegraph/`、没有 `.github/`；现有集中脚本入口为 `scripts/mock/gate.sh`，另有 UI 与 mock 自测脚本。
- 静态 `#[test]` / `#[tokio::test]` / `#[gpui::test]` 数量只是定位线索，不含所有宏展开，也不代表默认 feature 下的执行数量。

### 全范围入口清单

以下为 2026-09-20 分批修改期间的静态定位快照，只计算源码中的 `test` / `tokio::test` / `gpui::test` 属性；不是通过数量，也不作为删减目标。成员和测试文件均实际枚举，所有范围都有责任批次。

| 批次 | 包 | 含测试的 Rust 文件 / 属性数 | 环境与入口 |
| --- | --- | --- | --- |
| T2 | policy / exec / auth / tools / workspace / git | 5/78 · 8/69 · 11/83 · 20/141 · 20/169 · 9/63 | 本地文件、进程、Git、凭证与平台隔离；按包入口 |
| T3 | domain / storage / protocol / transport / workflow / control-plane | 14/63 · 23/174 · 24/181 · 5/20 · 2/28 · 14/206 | SQLite / UDS / 内存传输；storage、protocol、transport 补 feature |
| T4 | providers / engine / orchestration / testkit | 30/215 · 12/68 · 7/86 · 2/9 | HTTP/SSE loopback / 工具边界替身 / 真实 Git；九通道与 git feature |
| T5 | app / cli / client / pawork | 44/271 · 11/89 · 6/48 · 1/2 | ui-fixture / probe-self-test；真实 Host 子进程单独 `--host` |
| T6 | desktop / terminal / browser / computer-use | 34/266 · 1/9 · 2/4 · 2/4 | Desktop 单独；WebKit / 系统输入法 / 容器效果需真实环境 |
| T7 | scripts/mock 与 UI 工具 | gate / capture / OAuth / review / server / scenarios；scanner / ui-fixture / AX helper | Python 与 Shell 工具自身回归；不能替代产品验证 |

剩余显式 ignore：domain/storage 各一个 golden 更新器（不是验证，禁止自动重录）、auth 的跨进程测试子入口（由父测试驱动）、Kimi live 专项（真实凭证）。app live-smoke 已去掉双重 ignore，只保留 feature 与必需环境检查。

## 5. 用途与证据对应

| 真实操作 | 正常结果与完整性 | 必须拒绝/恢复的边界 | 现有验证承载 |
| --- | --- | --- | --- |
| 用户发起任务，模型调用工具后续答 | 调用真实 Engine、工具结果进入下一轮、只产生一个终态、用量累计 | 预取消不发请求，审批拒绝不执行，损坏流不伪造成功 | engine tool_loop、providers HTTP/SSE contract、app run |
| CLI/客户端创建、查询、导入会话 | 参数进入 Host、真实 SQLite 可查询、关闭进程回收 | 未授权能力拒绝，缺二进制失败，重启/重复命令不复用错误状态 | cli fixtures/ACP、client spawn_e2e、app gui_server |
| 保存文件、运行命令、修改 Git | 临时真实目录/进程/Git 的外部结果准确 | 越界、symlink、Secret、拒绝审批后零副作用 | policy/exec/tools/workspace/git 定向回归 |
| 恢复历史与连接 | 写入、重新打开、重放后内容/顺序/归属一致 | 迁移损坏、重复 append、断线、迟到帧、跨租户 | storage/domain golden、protocol/client/app、control-plane |
| Desktop 输入、切任务、缩放和关闭面板 | 草稿/焦点保留、控件可达、实际 bounds 与 AX 一致 | 断线禁写、过期回执不污染、缩放不遮挡 | GPUI 操作/布局与 projection；像素另用真窗口 |
| 终端/网页/虚拟桌面操作 | 文本/按键/导航/坐标经生产解析与传输 | 非法 URL、观察过期/跨 scope、取消释放、拒绝后无输入 | terminal、browser、computer-use RFB；WebKit/容器效果另验 |

### 定向命令

```bash
bash scripts/test.sh policy tools       # 按实际写入集选择；一次 Cargo 调用
bash scripts/test.sh providers storage  # 显式补齐各包必要 feature
bash scripts/test.sh desktop            # 图形 target 单独执行
bash scripts/test.sh --host             # 构建当前 pawork + 真实子进程
bash scripts/test.sh --print client     # 只显示将执行的命令，不算验证
bash scripts/mock/gate.sh --level 0,2   # 文档与 mock 工具自身回归
```

入口使用 `--tests`（包含 lib/bin 单测和集成目标），不硬加 `--lib` 导致 bin-only 包报错。Provider 九 feature、storage 的 compaction/checkpoint/protected、protocol typegen、transport memory、orchestration git、app ui-fixture、client probe-self-test 按所选包启用；不把真实 Provider 或 spawn-e2e 隐式加入普通包测试。显式外部验证缺环境必须失败或记录待验，不能计作已执行成功。

### 已执行的处置

| 项 | 处置与依据 | 保留证据 | 状态 |
| --- | --- | --- | --- |
| Desktop theme/main 常量副本 | 9 项 theme 测试收为 3 项行为/可读性测试，删除 main 的最小尺寸副本；这些副本不能证明控件使用了常量 | composer_layout_keeps_controls_in_card_and_ax_aligned、shell resize、inspector_motion_reverses_without_jump_and_snaps_when_narrow、theme 对比度/字号边界 | Desktop 243 通过（补回 U1 的 EntityInputHandler 导入） |
| mock gate 重复录制回放 | 删除独立 /usage 服务器启动，把字节+形状检查合并到 server_smoke 的原回放阶段 | phase_recorded_default_names 中 usage 分支 | L0/L2 通过，5.2s（当次本机） |
| mock gate 逐包 Cargo | 追加包去重后交给 scripts/test.sh，一次选择 feature；非法包失败、Cargo 错误透传 | 真实 Bash + 可执行 Cargo 替身验证参数/退出码 | 自测通过；真实入口已用于 T2、T3–T5、providers 复跑 |
| focus helper | 前台查询失败或空 PID 不再被视为失焦成功 | Bash 替身验证读取失败/空值拒绝、有效其他 PID 成功；未发送真实 UI 输入 | Shell 语法和三条路径通过 |
| quota 探针 | 空窗/失败/过期缓存/非 Absolute reset/缺 provenance/`served_stale` 均 exit 1；Go 三窗与 percent 类型/范围逐项核对 | review_selftest 的真实子进程 + UDS 主路径与失败矩阵（成功夹具含 Absolute reset + provenance） | 3 项 review 回归通过（含 quota 成功夹具与 unknown-reset / missing-provenance 失败用例） |
| UI fixture scanner | 保留实际临时文件扫描、脱敏输出、退出码和 symlink 边界 | scripts/test_ui_fixture_scan.py | 20 项通过 |
| terminal/browser/computer-use | 保留文本解析、导航拒绝、一次性观察及实际 RFB 字节交互；小测试不因短而删除 | 三包当前全部 17 个测试体已读 | 本批无改动，不重复编译；不推定 WebKit/容器真环境通过 |
| exec setsid 逃逸回收 | Linux 缺 `setsid`/`perl POSIX::setsid` 时不再 `return` 记绿；与 macOS 同样按平台前提失败 | `kill_reaps_descendant_that_escaped_with_setsid` | macOS 进程批次通过；Linux 前置条件修改未在 Linux 执行 |
| Composer 空 display_name 回退 | 删除仅断言 helper 的 `model_menu_row_title_is_single_line_for_any_name`；空名回退 id 是独立用户行为，并入真实分组菜单用例 | `model_menu_selected_follows_effective_model`：空名 `glm-5.3` 仍出现在分组、标题回退 id、按 id 可检索 | Desktop 当前入口 243 项通过 |
| U1 时钟/几何自测 | 删除 `advance_clock` / `debug_bounds` 几何自证；点击与滚轮仍用 debug_bounds 取命中点 | `clipboard_roundtrip_via_paste_action`、`scroll_wheel_event_reaches_overflow_container`、IME commit / keymap | 已修改；清掉 Arc/AtomicBool/Duration；EntityInputHandler 因 IME 用例仍使用而保留 |
| tools Seatbelt 上报 | macOS `run_command` 不再在 backend 不是 sandbox_exec 时 `return` 记绿 | `metadata_reports_seatbelt_isolation_when_available` | T2 定向批次通过 |
| Composer 发送尺寸常量 | 删除 helper 测试里的 36px 副本；高度钳制仍验证空闲/中等/封顶 | `composer_panel_height_clamps_across_input_sizes`、`composer_layout_keeps_controls_in_card_and_ax_aligned` | Desktop 当前入口 243 项通过 |
| domain hint 值上限 | 删除只钉 64KiB 常量的测试；键长边界保留 | storage `provider_hints_namespace_enforces_secret_size_and_shape_limits` | T3–T5 大批次 domain/storage 已通过 |
| engine framing 常量 | 删除业界约定副本；图片占位并入真实 Image content 计数 | `count_message_includes_framing_and_content` | T3–T5 大批次 engine 已通过 |
| transport 1MiB pin | 保留 `default_max_frame_matches_protocol_limit`：transport 不得依赖 protocol，超大帧拒绝用独立上限 | `oversized_declared_length_is_rejected_before_allocation` | 已检查；无改动 |
| UI fixture 启动 | 清旧 barrier；就绪必须是本进程启动后写入的有效 JSON（desktop：`settle_seq>=1` 且 `at_ms` 不早于启动；host：含 socket 的 JSON），PID 须通过 `pid_matches_kind` | 真实编译的替代 desktop 二进制 + 真 socket 黑盒五条路径：就绪 / 空文件 / 过期 at_ms / 进程早退 / 超时；不是 GPUI 真窗口验收 | 五条通过（/tmp/pawork-fx-harness）；初次 driver 因 `bash -c` 的 BASH_SOURCE 失败后改为文件 driver |
| AX / HID helper | AX 缺权限/无应用 identifier 非零；HID 投递前确认前台 PID 与投递权限 | 两份 Swift `-typecheck`；未向本机窗口发送输入 | 类型检查通过；真实权限/焦点路径待验 |
| client 子进程隔离 | 三条真实 Host 用例都清环境，限定临时工作目录与配置/数据根；无 Provider 错误精确归类 | spawn_e2e 原有三场景，按异步契约校验缺凭证终态 | 真实 Host 三项通过 |
| Desktop 工具状态/socket 文件名 | 删除独立映射表与 socket 文件名副本；未知词/失败/取消并入真实 ToolRowView 构造，socket 文件名并入 token 路径用例 | `tool_row_view_from_parts_normalizes_detail`、`default_token_path_aligns_with_a5_gui_token` | 已修改；Desktop 本轮复跑见执行证据 |
| Provider 能力夹具 | 补 `ModelCapabilities.supported_efforts` 缺失字段，恢复当前模型能力类型的测试编译 | negotiate full_caps 的现有能力门回归 | providers 复跑通过（/tmp/pawork-refactor-providers-rerun.log） |
| Provider Ord / 空累加器自证 | 删除 `capability_source_priority_is_static_then_probe_then_override` 与 `accumulator_starts_from_zero`；derive 顺序和 Default 零值不能证明生产合并或累计 | registry 三源收窄合并、usage 同请求覆盖 / 跨请求累加 / `finish_request` | providers 212 passed / 1 ignored，EXIT=0；日志 /tmp/pawork-refactor-providers-rerun.log |

### 当前执行证据（2026-09-20）

- `bash scripts/test.sh desktop`：冻结钉板恢复前 243 passed（`/tmp/pawork-refactor-desktop-2033.log`）。补回键表/tab_stop/握手能力面后的复跑日志 `/tmp/pawork-refactor-desktop-final.log`。
- `bash scripts/mock/gate.sh --level 0,2`：通过；日志 `/tmp/pawork-refactor-mock-2040.log`，L0 0.3s / L2 4.6s。
- `swiftc -typecheck scripts/ui-ax-dump.swift` 与 `swiftc -typecheck scripts/ui-key-event.swift`：通过。
- T2 六包：578 passed，exit 0；日志 `/tmp/pawork-refactor-T2-final.log`。真实 Host 三项：exit 0，日志 `/tmp/pawork-refactor-host-final.log`。
- app 270、cli 89、client lib/headless/contract 44 项已通过；原七包批次在 client probe 失败后停止，未执行的包未计通过。修复终端 fixture 后 `cargo test -p pawork-client --offline --features probe-self-test --test probe` 通过 1 项（内含 13 场景），日志 `/tmp/pawork-refactor-probe-final.log`。
- T2–T6 已按包读完测试体或给出完整审查报告；抽样片段不记完成。真实 Provider、Linux/Windows、WebKit 与容器效果未因本次静态审查获得验收。
- T3–T5 大批次各 target 0 failed（约 1153 passed / 3 ignored，含删除自证前的 providers 174 项）；日志 /tmp/pawork-refactor-T3T4T5-final.log。包装脚本未写 EXIT。
- providers 删除 Ord/空累加器后复跑：212 passed / 1 ignored（Kimi live），EXIT=0；日志 /tmp/pawork-refactor-providers-rerun.log。

## 6. 继续批次记录

- 后端首批 `bash scripts/test.sh domain app cli client providers engine orchestration tools exec` 在 `ui_fixture_seed_to_host_snapshot_and_timeline` 失败：实际 Workspaces 为 3，旧断言为 1。源码和 Spec 均确认 Host 返回全注册表；已改成完整 id/name 集合、注册根目录及会话归属断言；定向测试已通过（/tmp/pawork-refactor-fixture.log），根目录两端统一 canonicalize 后随 app 批次复核。
- 恢复时发现两个并行遗留 Cargo 批次；另有审查报告通过 Shell 命令替换意外触发 T2 Cargo。重复批次均终止；这些不完整日志不作为整批通过。后续 Cargo 只由主代理串行运行。
- `mock/gate.sh --packages` 增加包名检查，拒绝 `--print` / `--help` 等选项注入，防止下层脚本空跑却显示 L1 PASS。实际 Shell 子进程 + fake Cargo 已验证拒绝、去重与失败退出码传回；不算 Rust 行为验证。
- `test.sh --host` 与 `ui-fixture.sh` 的 build 均经 `cargo build --message-format=json` 的 artifact 消息定位本次产物；config 的 target-dir / build.target 不会让测试落到路径上的旧二进制。fake Cargo 已验证带空格路径、build 失败后不启动测试及 `--print` 不执行。
- T2 全范围只读报告已核对并完成必要处置；T3–T6 继续补齐，最终状态以逐文件记录和实际执行结果为准。
- T3 全范围审查完成（82 个测试文件 / 664 项，报告 `/tmp/pawork-audit-T3-complete.md`）：无 `return` 假绿。已实施：删除 8 项（QuotaError 变体互异、Confidence 默认值自证、now_millis 时钟漂移、SqliteUsageLedger Send/Sync 自证、TransportFrame getter 自证、RetentionPolicy 默认值副本、workflow 两条 `include_str!` 源码扫描）；合并 4 组（错误 Display 进 retryable 分类表、空 JSON policy 进 legacy 回填、snapshot validate 进 V1 golden、窗口毫秒常量进 rolling-window 查询、predict_exhaustion 主路径并入 None 边界）；改写 `legacy_map_is_frozen` 去掉映射表第二份拷贝，保留逐键行为断言。`seal_for_test_matches_pwb1_golden_hex` 保留：集成 golden 用的是测试内手工构造，只有该项钉住生产 `seal` 输出冻结字节，删除会失去生产路径固定。domain hint 键长、超大帧、resume 矩阵、导入零残留等冻结契约全部保留。
- 新门禁补跑暴露 protocol `registry_tables_are_complete_and_unique` 硬编码命令总数（44）过期（实际 45）：旧默认入口不编译该集成目标，HEAD 即红。完备性已由穷尽 match 测试证明，删除字面量总数，保留登记表 wire 名唯一性检查；registry 目标复跑 8 项通过（/tmp/pawork-refactor-registry.log）。

- 真实 Host 缺凭证用例原先因未注册 workspace 提前报错；隔离 HOME/XDG 并使用合法无项目会话后，确认真实契约为 Accepted → 异步 Failed。改为先订阅、按 run_id 等待失败终态，并断言历史只有一条终态且明确缺凭证原因。三项真实进程测试全部通过，日志 `/tmp/pawork-refactor-host-final.log`；生产契约未改。
- T2 审查处置：保留 auth MCP 文件名契约、workspace 跨平台配置路径和 MCP 依赖边界检查；Git 大输入留正确性检查、去除任意耗时断言。其它删除/合并由 golden、真实操作或 trait 编译约束承接，各包 Spec 已同步。

- T2 六包入口 `bash scripts/test.sh policy exec auth tools workspace git` 完整 exit 0：578 passed；auth 的 1 ignored 是由父测试调用的跨进程子入口，不是漏验。日志 `/tmp/pawork-refactor-T2-final.log`。本机实测 Seatbelt 读写与 Secret 拒绝、进程取消/杀树、真实 Git 与文件副作用；不推定 Linux/Windows 通过。
- mock L0/L2 复跑出现入口自测子进程 10s 启动超时；`bash -x` 同参数诊断确认命令命中临时 Cargo 替身并正常结束（2.39s），没有执行真实 Cargo。将防挂死等待上限改为 60s，参数和退出码断言不变；不能把首次超时隐藏为全程稳定。复跑 L0/L2 exit 0，日志 `/tmp/pawork-refactor-gates-complete-2.log`。scanner 20 项通过，日志 `/tmp/pawork-refactor-scanner-complete.log`。

- client probe 的 terminal-gate 原先以未注册 ws-unbound 创建 PTY，提前报错而未验证审批；现先经 WorkspaceAdd 注册临时目录，再分别创建/关闭与拒绝，两条快照均无残留终端。13 场景通过。

- T3 落地：删除 quota 变体/默认值/墙钟/Send+Sync、transport 字节持有、retention Default 副本；legacy hint 只留 canonical_hint_key 行为。protocol `registry_tables_are_complete_and_unique` 不再钉 44 条命令（实际 45，含 set_model_reasoning）；完备性改由穷尽 match + 无重复 wire 名承担。T3–T5 合并批次随后通过（日志 /tmp/pawork-refactor-T3T4T5-final.log；该次未含 app）。
- T5 app：settings/run/session/idempotency/gui_server/approval/import/usage 等均为真实 Host 操作、持久化或 fail-closed；不删。client 版本文案自证已删。

- T8 收口（2026-09-20 续）：冻结面要求 bin 内钉住 `APP_VIEW_KEYBINDINGS` 与 `MAIN_PATH_TAB_STOP_IDS`，已恢复这两项；握手能力面同样恢复，因 Browser/Terminal 能力声明没有其它测试钉生产列表。Activity 可见性布尔、键表以外的常量副本、last_acked 单调性自证仍删除。`bash scripts/test.sh app` 通过（244 lib + 6 multi-gui + 17 gui_server + 2 timeline + 1 ui-fixture，exit 0，日志 /tmp/pawork-refactor-app-final.log）。Desktop 在补回冻结钉板后复跑，日志 /tmp/pawork-refactor-desktop-final.log。
- 未把抽样审查或建议记为完成。未执行：真实 Provider live-smoke、Linux/Windows 隔离、WebKit 页面效果、computer-use 真容器、全 workspace / 发布门禁。
- T7 门禁终审（Cicero，/tmp/pawork-gate-final-review.md）四项缺陷全部修复：`test.sh --host` 与 ui-fixture 两个 build 改经 `cargo build --message-format=json` 的 artifact 消息定位本次产物（config 的 target-dir/build.target 不再导致误测旧二进制）；quota 探针对 stale / 缺 provenance / reset 非 absolute fail-closed，selftest 夹具同步并补未知 reset 与缺 provenance 两个失败用例；`ui-key-event.swift` 的前台 PID 与投递权限校验移到输入源切换之前；`ui-fixture.sh desktop` 就绪闸门要求 barrier 为本进程启动后写入的有效 JSON（`settle_seq>=1`、`at_ms` 不早于启动时刻），等待循环与终检改用 `pid_matches_kind` 归属校验。最后一项用真实编译的替代 desktop 二进制 + 真 socket 文件黑盒验收五条路径（就绪 / 空 barrier / 过期 at_ms / 进程早退 / 超时）全部通过（/tmp/pawork-fx-harness，未入仓库）。
- Spec 同批回写：workflow.md §7 移除两条已删 `include_str!` 源码扫描的覆盖声明（红线约束改引 architecture 依赖方向与评审）；storage.md §7 修正内嵌测试计数为 21（HEAD 即 21，原 27 为历史笔误）；cli.md 无需改（render.rs 仅删常量字面量等式，「已截断」检测行为测试保留）。
- 子代理事故：T6 代理（Peirce）第二次偏离只读授权——累计 127 条命令，反复启动/杀死 desktop 编译，并用 shutil 删除 `target/debug/incremental/pawork_desktop-*`（违反 target 缓存纪律），已归档关闭；T5 代理（Kierkegaard）当前轮 80 分钟无任何输出视为卡死，一并关闭，其 T5-complete.md 已含 pawork/client/cli 完整结论，app 结论由主代理直读完成。被删增量目录仅使下一次 desktop 编译变慢，无仓库影响；仓库文件清单全程核对无越权写入。此后 Cargo 仅由主代理串行执行。
- Desktop 终跑（主代理）：`bash scripts/test.sh desktop` 243 passed / 0 failed，EXIT=0；三个恢复钉板 `handshake_capabilities_pin_desktop_surface`、`keybinding_table_includes_approval_and_cancel`、`main_path_buttons_are_marked_tab_stops` 均在列。日志 /tmp/pawork-refactor-desktop-final.log。

## 7. 关联文档

- [验证与证据规格](spec/verification.md)
- [跨包关键链路](spec/flows.md)
- [活动路线图](ROADMAP.md)
- [工程约定](../AGENTS.md)
