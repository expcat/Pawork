# 修复执行计划

> 问题详情见 [Review](review-2026-09-23.md)。实施前先读取所列包的 [Spec](../spec/README.md)，核对当时源码与工作区，只补剩余缺口。
>
> 状态（2026-09-24）：**第一批 T-01～T-07 与第二批 T-08a/b/c、T-09a/b、T-10a/b/c、T-11、T-12 已实现并通过定向门禁**——
> - T-01～T-03：`bash scripts/test.sh tools`（140 过）、`bash scripts/test.sh providers`（224 过）、`bash scripts/test.sh app`（含 R-02 全链路、R-03 并发/替换失败回归）。
> - T-04（R-04/R-24）：`bash scripts/test.sh transport app cli` 全绿（新增 `instance_lock` 7 单测、R-24 旁路装配验收、listener close 归属回归）+ `bash scripts/test.sh --host`（3 过）+ 真实二进制手动验收（双启动仅一成功、崩溃后 stale 清理与再启动、SIGINT 优雅退出清理 socket/pid/登记、旁路 `sessions list` 不影响活跃 Host）。Windows 侧（`local_windows.rs`、share_mode 锁）未在本平台验证，记待验。
> - T-05a（R-05，同宿主部分）：`bash scripts/test.sh workflow app` 全绿（新增 `shared_cancel_token_stops_real_executor`、`tasks_cancel_stops_blocking_run_and_records_canceled_once`，并扩充 GUI 取消测试断言任务记 Canceled）。CLI `tasks cancel` 跨进程路由活跃 Host 需新增 `TasksCancel` wire 命令，草案见 T-05a 行「待确认」，确认前不动协议冻结形状。
> - T-05b（R-16/R-23）：`bash scripts/test.sh app`（264 过，新增 `orphan_tasks_sealed_only_after_owner_exits`、`demo_pairs_worker_task_terminals`）与 `bash scripts/test.sh cli` 全绿。孤儿任务经 `open_control_plane` 按实例清扫权收口为 Failed(interrupted)，旁观者/旁路 CLI 不误扫；demo 任务与 supervisor 终态配对。
> - T-06（R-06/R-19）：`bash scripts/test.sh app` 全绿（266 lib + 4 集成二进制；新增 `run_start_second_run_same_session_rejected_until_settled`、`run_start_unapproved_plan_rejects_synchronously`（改写自旧合成兜底测试）、`run_start_async_early_death_seals_durable_failed`）。会话槽占用早于首个 await 保证并发突发确定性；seal 是否补 `RunStarted` 以 store 投影为准（覆盖绕开 bus 落库的种子行）。
> - T-07（R-07）：`bash scripts/test.sh storage app` 全绿（storage 更新 `golden_session_keeps_critical_state`/`zero_retained_turns_drops_all_conversation_history` 计数并新增 `retained_messages_keeps_exact_count_suffix`；app 新增 `compact_suffix_matches_projection_after_restart`：早期 System + 折叠旧轮 + 含工具轮的保留后缀压缩后，engine 重建窗口、resume 消息与重启重放投影三者同界）。`RetentionPolicy` 新增 `retained_messages` count 后缀规则（serde 缺省 0 向后兼容），早期 System 不再豁免；`loop_ctx` 从轮后缀切换到 count 后缀，与 engine 折叠边界同语义。未动 Compaction 事件与磁盘 schema。
> - T-08a/b（R-09/R-10）：`bash scripts/test.sh tools`（144 过；新增 `queued_call_cancelled_while_waiting_for_slot`、`timeout_waits_for_cooperative_drain_before_responding`、`cancel_between_ops_stops_later_ops`、`cancelled_token_starts_no_ops`）。scheduler 槽位等待与取消 select；派生执行令牌（调用方取消桥接 + 超时单独触发），超时等待工具协作收口再回执 Timeout；write/edit 提交边界与 apply_patch 操作边界检查取消（命中即回滚保持全成或全滚）。
> - T-08c（R-18）：`bash scripts/test.sh engine app` 全绿（新增 `manual_compaction_pre_cancelled_token_fails_without_provider_call`、`manual_compaction_cancelled_summary_is_not_degraded`）。摘要取消不再降级为结构摘要，持久提交前再查取消，宿主 `compact_history` 写入前检查令牌；取消路径零压缩事件，手动入口返回取消。
> - T-09a（R-11/R-17）：`bash scripts/test.sh transport cli app` 全绿（transport 新增 `pending_accept_is_woken_by_close`；cli 新增 `connection_set_shrinks_as_sessions_complete`；app 集成新增 `wait_done_fires_after_client_disconnect`）。`GuiConnection` 新增默认方法 `wait_done`；cli `gui serve` 以 `ConnectionSet` 登记连接 + reaper 回收，退出按关 listener→close 连接→`wait_done` 收口→删除 PID→drop Host 持有者→pty/Core shutdown→随 Core 释放实例锁 排序（core 仍被共享时告警不静默跳过）；Unix listener accept 与 close 通知 select，Windows 同形态（watch）记待验。
> - T-09b（R-12）：`bash scripts/test.sh client` 全绿（新增 `snapshot_timeout_discards_connection`）。Snapshot/Resume 回复帧无 request_id，等待超时即 `disconnect()` 废弃连接，迟到旧快照不再可能被后续请求误收；恢复走既有重连路径，wire 未演进。
> - T-10a（R-20/R-21）：`bash scripts/test.sh storage app` 全绿（storage 新增 `maintenance_listing_includes_archived_sessions`；app 新增 `startup_sweep_seals_archived_session_and_recovers_usage`）。storage 新增维护查询 `list_sessions_including_archived`；启动清扫覆盖归档会话，补偿 `RunFailed` 携带崩溃前各 request 的最新 `UsageUpdated` 快照之和（同 request 不重复相加、不推测未回报）。
> - T-10b（R-22）：`bash scripts/test.sh app` 全绿（新增 `reconcile_backfills_missing_ledger_record_once`）。`UsageService::reconcile_ledger_from_terminal_runs` 启动对账：终态已知用量但账本缺记录的 run 幂等补账恰好一次，归属只取自持久 `ProviderRequestStarted`（缺归属跳过不猜），只写账本不改 Run 终态；经 `AppCore::reconcile_usage_ledger` 在 load_with/load_from 末段按 `can_sweep` 门触发。
> - T-10c（R-13）：`bash scripts/test.sh control-plane app` 全绿（control-plane 新增 `invalidate_local_scope_drops_only_local_entries_in_scope`；app 新增 `record_usage_invalidates_local_quota_cache`）。`QuotaService::invalidate_local_scope` 只失效本 scope 的 LocalLedger/Derived 派生窗口，远端权威缓存口径不变；所有本地入账入口（正常路径与对账共用 `record_attributed_usage`）成功写入即失效。
> - T-11（R-14）：`bash scripts/test.sh tools`（145 过，新增 `oversize_file_is_skipped_and_reported`）。search_text 单文件 4 MiB 读取上限：超限文件不装入内存直接跳过，metadata `skipped_oversize` 计数且 `truncated=true`，不冒充完整搜索。
> - T-12（R-15）：`bash scripts/test.sh cli` 全绿（新增 `systemd_unit_quotes_and_escapes_executable_path`、`launchd_plist_escapes_xml_special_characters`）。systemd ExecStart 双引号包裹 + 反斜杠/双引号转义 + `%` 翻倍，plist `<string>` XML 五字符转义；只验证文本生成，不安装系统服务。
> 已列修复已实施；T-05a 跨进程取消仍 **待确认、未实现**，见 wire 草案。

提交前复审（2026-09-24）：补齐 GUI 锁持有至 Core 退出、Run/Task 恢复完成前保持清扫互斥、并发 accept 关闭竞态、按 request 汇总恢复用量、补账保留发生时间、实际有界读取及 Provider 取消的 Task 终态映射。`bash scripts/test.sh tools providers app transport cli workflow storage engine auth client control-plane` 完成 1,368 项通过、0 失败、3 项既有忽略；`bash scripts/test.sh --host` 3 项通过。当前二进制在临时数据目录实测重复 GUI Host 启动被拒、原 Host 保持运行，SIGINT 清理 socket/PID/登记，随后同实例重启与关闭成功。L0、文档链接、46 个修改 Rust 文件的格式与 diff 检查通过。Windows、真实 Provider、Desktop 真窗口与全 workspace 门禁未运行，不记为已验收或已发布。

## 1. 顺序与共用边界

第一批解决 P1：T-01/T-02 可独立推进，T-03 建立可靠快照写入；T-04 建立 Host 所有权后再做 T-05 的跨进程取消与孤儿恢复；T-06 收口 GUI run 接受边界；T-07 统一压缩保留语义。每个任务独立验收，不把第一批做成跨仓大提交。

第二批解决取消、关闭、用量恢复和有界资源；重叠文件的任务顺序执行。T-08a/T-08b 共用 tools scheduler，T-10a/T-10b 共用恢复与用量服务，分别串行。包级整理放在对应行为修复之后，不为修复新增包或通用框架。

## 2. 第一批：正确性与安全

| 任务 / 发现 | 最小写入集及 Spec | 交付边界 | 定向验收 |
| --- | --- | --- | --- |
| T-01 文件原子写安全 · R-01 | tools `common.rs`、现有文件工具回归；tools Spec | 独占创建同目录临时文件、失败保留目标、只清理本次拥有的临时文件 | 预置 symlink 的外部哨兵不变；正常覆盖保持权限 |
| T-02 Provider 流完整性 · R-02/R-08 | providers 流错误/解析器及既有契约测试；providers Spec；必要时 engine/storage 的现有集成测试 | 原始错误正文不进公共错误；畸形 JSON 明确失败。顺带核对 O-02 不可达分类规则 | 含假 Secret 的错误经持久链不泄漏；畸形片段后正常 stop 仍失败 |
| T-03 Task 快照提交 · R-03 | app `tasks_host.rs`、`services/tasks.rs`；app Spec | 取快照到提交串行，原子替换失败不丢上一版 | 替换失败重载旧状态；并发任务完成后重载含全部终态 |
| T-04 Host 实例所有权 · R-04/R-24 | cli 装配/`gui.rs`、app 打开库/恢复入口、transport `local_unix.rs`；cli/app/transport Spec | 恢复写入前原子占有实例；普通查询不清扫活跃 run；stale 识别、PID/token 初始化与释放次序 | 活跃 Host 下旁路查询不改 Run；双启动仅一成功，旧端点 close 不破坏赢家 |
| T-05a Task/Run 统一定位 · R-05 | app task/run/GuiRunRegistry；必要的 workflow 接口；app/workflow Spec | 同宿主共用真实 run token，终态映射以 Run 为准；明确 CLI 如何找到活跃 Host | task/run 两入口分别取消阻塞执行，持久任务与 run 均为取消，无双终态 |
| T-05b Task 启动恢复与 demo · R-16/R-23 | app task/orchestration host；app Spec | T-03/T-04/T-05a 后，真正的所有者恢复孤儿任务；demo 不留下生产悬空任务 | 所有者退出后重载收口；活跃 Host 不被旁路 CLI 误扫；demo 完成/取消无遗留 |
| T-06 GUI Run 接受边界 · R-06/R-19 | app `gui_host/handlers/run_start.rs`、运行注册表与 run 前置校验；app Spec | 读取历史前保留 session 活动槽；能同步拒绝的错误不返回 Accepted；异步失败必须有持久生命周期 | 同 session 第二轮确定性拒绝，结束后可再启动；Plan 拒绝响应与重开 store 一致 |
| T-07 压缩保留一致性 · R-07 | app `loop_ctx.rs`、engine compaction、storage retention/projection；三包 Spec | 先决定现有契约内能否表达保留集合，当前请求与重放使用一个决定；保持分支隔离 | 早期 System + 折叠旧轮 + 保留尾轮压缩后、重启后消息一致；沿用既有分支回归 |

T-05a 若涉及新增 Host 控制命令，先交出 wire 草案；T-07 若需要改 Compaction 事件或磁盘 schema，先交出兼容与 golden 草案。按现有工程约定确认后实施，不能在“修 bug”名义下静默改冻结形状。本轮只登记该决策点，不引入新审批制度。

**T-05a wire 草案（2026-09-23 登记，待确认后实施）**：同宿主部分已落地（Agent 任务经 `register_with_cancel_token` 共享 run 令牌；run 收尾 `tasks_finish_from_run` 按真实终态映射，已终态跳过）。CLI `tasks cancel` 与执行体天然跨进程，兑现「两入口都停止同一阻塞 run」需要新增 Host 控制命令：

- `AppCommand::TasksCancel { task_id: String }`（沿用命令账本与幂等 wrap）；Host handler 即 `core.tasks_cancel(task_id)`——同进程状态机 + 共享 run 令牌，执行体停止后 run 收尾见任务已终态而跳过。响应 `AppResponse::Data { cancelled: [task_id...] }`；未命中/歧义沿用结构化 host_error。
- CLI `tasks cancel` 路由：Catalog 加载解析 spec → 任务非活跃则本地幂等收口；活跃则先探测 GUI socket（ops 既有 connect/handshake 路径），Host 可达即发 `TasksCancel`；socket 不可达但 `hosts/` 登记显示活跃 Executor（无控制通道）→ 如实拒绝并提示属主 pid；无任何活跃所有者 → 如实拒绝，孤儿收口归 T-05b。

## 3. 第二批：运行与恢复

| 任务 / 发现 | 最小写入集及 Spec | 交付边界与定向验收 |
| --- | --- | --- |
| T-08a 排队取消 · R-09 | tools scheduler；tools Spec | 获取 permit 与取消竞争；槽位未释放前请求已取消，executor 零调用 |
| T-08b 写操作取消/超时 · R-10 | tools 文件工具和 scheduler；tools Spec | 传递 token、明确提交边界、等待/追踪不可中断部分；响应后不能继续启动新写操作，取消回执与文件最终状态一致 |
| T-08c 压缩取消 · R-18 | engine compaction、app loop_ctx；engine/app Spec | 摘要 Cancelled 不走普通降级；宿主写入前检查 token；取消后无 CompactionCompleted |
| T-09a 连接回收与 Host shutdown · R-11/R-17 | cli GUI、app gui_server、transport local；三包 Spec | 收到完成信号即移除句柄；停止监听、等待连接收口、释放 core 持有者；pending accept 可被 close 唤醒，重复 close 幂等 |
| T-09b Snapshot 超时 · R-12 | client；client Spec；必要时 desktop 恢复调用 | 先做现 wire 下的连接废弃/恢复；迟到旧快照不得满足新请求。协议 request_id 另作兼容演进，不捆绑当前修复 |
| T-10a 崩溃终态恢复 · R-20/R-21 | app session/usage、storage 维护查询；app/storage Spec | 包含归档会话；按 request 重建已知 usage，补终态一次。先验证归档悬空 run，再验证累计用量快照不重复相加 |
| T-10b 用量补账 · R-22 | app run/usage/启动恢复，必要的 control-plane 查询；app/control-plane Spec | T-10a 后，持久终态可幂等对账；一次入账失败恢复后只补一次，成功 Run 终态不变；核实可恢复的 provider/model/account 归属，缺信息不能猜 |
| T-10c 本地 quota 一致性 · R-13 | app usage、control-plane quota 缓存接口；两包 Spec | 所有本地 ledger 成功写入入口失效相应缓存；读→写→立即读窗口不旧，远端权威额度语义不变 |
| T-11 有界文件搜索 · R-14 | tools search_text；tools Spec | 选有界读取/流式扫描的最小方案；大文件与超长行不破坏内存界限，截断/跳过可见，保留正则与上下文行语义 |
| T-12 服务文件编码 · R-15 | cli service；cli Spec | 目标格式正确编码；包含空格/XML 特殊字符路径解析后 argv 完整，不安装系统服务 |

任务如果跨出上述写入集，先解释新增改动如何直接服务该条验收；不要顺手统一所有 token、数据库或错误框架。用量对账需要稳定事实标识，优先复用已持久化字段；缺少字段时明确契约决定，不静默编造补账记录。

## 4. 冗余与性能切片

O-01/O-02 可随相关 Provider 修复收敛。O-03 先测长账本查询，实际有瓶颈才优化；O-04 等分页需求触达再抽取。O-05 是后续描述清理。O-06/O-07 先做可选编排模块的产品取舍，再决定修复并激活或退出；O-08 只在增量重放语义需要时处理。完整条件见 [包分析](package-boundaries.md)，没有任务要求合并现有 24 包。

## 5. 实施时的验证入口

先用受影响的现有定向测试；只有当前缺陷没有覆盖时，增加能观察到错误行为的最小回归。下列是未来命令模板，不是本轮已运行记录：

| 范围 | 入口 |
| --- | --- |
| 文件安全、取消、搜索 | `bash scripts/test.sh tools` |
| Provider 流 | `bash scripts/test.sh providers`；跨持久化回归按实际目标追加 engine/storage/app |
| Task/GUI Host 服务 | `bash scripts/test.sh app`；改 workflow 状态接口时追加 workflow |
| 压缩与恢复 | `bash scripts/test.sh engine app storage`，或按三包 Spec 选择更窄目标 |
| 连接与 CLI | `bash scripts/test.sh cli transport client app` 中只选实际写入包；真实 Host 进程行为用 `bash scripts/test.sh --host` |
| 用量与额度 | `bash scripts/test.sh app control-plane` |
| Desktop 恢复路径 | `bash scripts/test.sh desktop` 单独运行；涉及连接/UI 行为还需真窗口 |

同一时刻一个 Cargo 进程；必要 feature 由脚本补齐。平台相关路径必须在目标 OS 验收；缺环境记待验。只有文档变动时执行 L0 与链接/diff 检查即可。全 workspace、发布、真实 Provider 扩展矩阵不自动加入修复任务。
