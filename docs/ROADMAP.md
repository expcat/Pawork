# Pawork 活动路线图

> 更新：2026-09-23；源码基线 `main / f14edb23`。本轮完成全项目静态 Review 与文档整理，未修改实现。这里只保留待实施和未闭合验收，历史完成记录从 Git 追溯。详细入口：[Plan](Plan/README.md)。

## 1. 优先修复

[审查报告](Plan/review-2026-09-23.md) 登记 24 项源码问题（8 项 P1、16 项 P2）；其中 listener 并发关闭是库 API 条件问题。以下按风险与依赖排序，具体写入集和验收见 [执行计划](Plan/remediation.md)。T-01～T-04 已实现并通过定向门禁（2026-09-23，tools/providers/app/cli/transport 回归与 `--host` 全绿；T-04 另经真实二进制手动验收），尚未提交。T-05a 同宿主部分已落地（R-05：Agent 任务共享真实 run 取消令牌、任务终态以 run 终态映射且仅写一次，workflow/app 定向回归全绿）；CLI `tasks cancel` 跨进程路由到活跃 Host 需新增 wire 命令，wire 草案见 [执行计划](Plan/remediation.md) T-05a 行，按冻结契约待确认。 T-05b 已完成（R-16/R-23：孤儿任务按实例所有权收口、demo 任务终态配对）。T-06 已完成（R-06/R-19：同 Session 活动 Run 期间第二轮 `session_busy` 同步拒绝且收尾后可再启动；未批准 Plan 的 RunStart `plan_not_approved` 同步拒绝；engine 无事件早死由宿主 seal 补持久化 RunStarted+RunFailed，重开 store 可见完整 failed 生命周期）。T-07 已完成（R-07：`RetentionPolicy` 新增 `retained_messages` count 后缀规则，早期 System 不再豁免，压缩保留集、当前请求窗口与重启重放水位三者同界；未动 Compaction 事件与磁盘 schema）。T-08a～c 已完成（R-09/R-10/R-18：工具槽位排队取消即回、超时经派生令牌等待协作收口再回执 Timeout、写工具提交/操作边界停写并回滚；压缩摘要取消不再降级提交，持久写入前查令牌，取消路径零压缩事件）。 T-09a/b 已完成（R-11/R-17/R-12：`GuiConnection.wait_done` 完成信号 + cli 连接登记 reaper 与有序关闭收口 Core shutdown、Unix listener pending accept 可被 close 唤醒（Windows 同形态记待验）、Snapshot/Resume 等待超时即废弃连接防迟到旧快照误收；transport/client/cli/app 回归全绿）。 T-10a～c 已完成（R-20/R-21/R-22/R-13：启动清扫覆盖归档会话、补偿终态携带崩溃前已知用量快照、终态缺账 run 启动幂等补账恰好一次（归属只取自持久 ProviderRequestStarted）、本地账本入账即失效同 scope 本地派生 quota 窗口缓存；storage/control-plane/app 回归全绿）。 T-11/T-12 已完成（R-14/R-15：search_text 单文件 4 MiB 读取上限、超限跳过并计数标记不完整；service 定义 systemd ExecStart 与 plist XML 正确编码，含空格/特殊字符路径 argv 完整；tools/cli 回归全绿）。

| 顺序 | 任务 | 剩余工作 |
| --- | --- | --- |
| 1 | ~~T-01 / T-02~~ ✅ | 已完成：文件临时路径独占创建；流错误安全文案与畸形 SSE 失败语义（R-01/R-02/R-08） |
| 2 | ~~T-03 / T-04~~ ✅ | 已完成：Task 快照原子提交（R-03）；Host 单实例所有权、socket 生命周期与恢复清扫隔离（R-04/R-24，Windows 侧记待验） |
| 3 | ~~T-05a~~（同宿主部分）✅ / ~~T-05b~~ ✅ | Task 与 Run 取消/终态贯通——同宿主令牌与终态贯通完成（R-05）；孤儿任务按所有权收口与 demo 终态配对完成（R-16/R-23，app/cli 回归全绿）；跨进程 tasks cancel 路由待 wire 草案确认 |
| 4 | ~~T-06~~ ✅ | 同 session Run 排他；Accepted 与持久生命周期一致（R-06/R-19，2026-09-23 app 回归 266 全绿） |
| 5 | ~~T-07~~ ✅ | 已完成：压缩保留集合、当前请求和重放投影一致（R-07，storage/app 回归全绿） |

## 2. 随后收口

| 任务 | 剩余工作 |
| --- | --- |
| ~~T-08a～c~~ ✅ | 已完成：工具排队取消、阻塞写超时与压缩取消（R-09/R-10/R-18，tools/engine/app 回归全绿） |
| ~~T-09a / T-09b~~ ✅ | 已完成：连接句柄回收、Core shutdown 有序收口、listener close 唤醒 pending accept、Snapshot 超时废弃连接（R-11/R-17/R-12，transport/client/cli/app 回归全绿，Windows 侧记待验） |
| ~~T-10a～c~~ ✅ | 已完成：归档会话崩溃清扫与已知用量恢复、终态缺账启动幂等补账、本地 quota 派生窗口入账即失效（R-20/R-21/R-22/R-13，storage/control-plane/app 回归全绿） |
| ~~T-11 / T-12~~ ✅ | 已完成：search_text 单文件读取上限与跳过报告、service 文件路径按目标格式编码（R-14/R-15，tools/cli 回归全绿） |

## 3. 包定位与冗余收敛

[24 包分析](Plan/package-boundaries.md) 建议保留现有包边界。近期只收敛重复契约辅助断言与不可达错误分类；长账本聚合先测量，Desktop 分页抽取随真实改动进行。未装配 DAG/worktree/merge、durable lease、Git staging 和未消费 API 先作产品取舍，再决定接线或退出，不新增包或第二套框架。

## 4. 产品与验收

详细范围与逐项验收见 [产品计划](Plan/product-and-acceptance.md)。

| 线 | 当前剩余项 |
| --- | --- |
| MM-1 | 真实识图完整往返、图片预览与失败反馈真窗口验收 |
| MM-2 | 视频 canonical/能力/wire 草案、契约确认与分层接线 |
| MM-3 | Kimi/Qwen 实际 endpoint 与 GLM MCP 搜索核对、Desktop 来源展示及正向验收 |
| RV-01～11 | 已实现功能的逐项真窗口复验与用户验收；不重复开发 |
| GUI/账号/平台 | GUI2/3/4 用户验收，IME/模型目录/字号与键盘矩阵，真实 OAuth/额度，以及尚缺的特定环境证据 |
| 未排期产品缺口 | 文件夹附件、持续目标 GUI、Plan GUI、外部浏览器上下文、技能录制/绘图/插件列表 |

其它候选仍见 [Backlog](spec/backlog.md)。冻结契约变更先形成具体草案；真实功能验证按 [验证规格](spec/verification.md) 使用当次指定模型。全 workspace 门禁和发布不在本轮范围。
