# R2 Wave C 收口记录（2026-08-27）

> 范围：R2 U2 最后缺口「连接失败重试」模拟操作相位（drop-socket 断连重试 + host 停机 ConnectFailed 重试双循环）与 R2 退出标准缺口盘点。视觉/交互零改动；F-03/F-04（R3）、F-05/F-09/F-12（R4/R5/R6）按 ROADMAP 指针不进本波。
> 驱动：新增 scripts/ui-wave-c-connect.sh（复用 Wave B 链路：barrier/轮询同步、ui-fixture 隔离实例、AX 语义定位；fixture 侧 drop-socket / restart-host 契约此前已冻结，本波首次接线）。

## 1. 实现落点（写入集：scripts/ 三个文件）

- scripts/ui-wave-c-connect.sh（新建）：双循环编排。循环 1（Disconnected 重试）：seed→serve→desktop→AXPress 开会话→drop-socket→disconnected 相位（reconnect 在场、壳层/rail/会话选中/旧时间线条目全保留、空态引导缺席）→AXPress reconnect→新 settle barrier→AXPress 会话行重开→reconnected 相位。循环 2（ConnectFailed 重试）：冻结的 serve_stop.request barrier 停 host→disconnected 相位→host 停机窗口内 AXPress reconnect→轮询 connection-status 出现「Connect failed ·」文案且重试入口仍在→restart-host→AXPress reconnect→settle barrier→重开会话→reconnected 相位。全程 barrier/轮询，无固定 sleep；多轮证据以 stem 区分不覆盖。
- scripts/ui-wave-d-tools.py：新增 disconnected / connect-failed / reconnected 相位（1440 三栏几何合同复用）。disconnected：reconnect-present + connection-status-disconnected（「Disconnected ·」）+ session-selected + timeline-loaded + empty-hint-absent；connect-failed：reconnect-present + connection-status-connect-failed（「Connect failed ·」）；reconnected：reconnect-absent + connection-status-connected（「Connected ·」）。默认 initial/final 与 Wave B 各相位行为未动。
- scripts/test_ui_wave_c_tools.py（新建）：15 个正/负向单测（含 Disconnected/ConnectFailed 互斥、Connected/Connecting 假阳性、reconnect 缺失/残留、条目清空、空态引导残留、未知相位拒绝、driver 守卫、bash -n、1440 几何钉住）。

## 2. 审查与同批修复

- glm_reviewer 第一轮：无 P0/P1；P3-2 reconnected Connecting 瞬态加固已落地（connection-status-connected）。P3-1 serve_stop.request 契约名散落记录为测试基建候选，不扩大写入集。
- 主代理收口审查（拍板 a 后）：disconnected 相位原先只认 reconnect-present，Connected 树也能过；循环 2 的 ConnectFailed 与 Disconnected 共用同一相位。已拆出 connect-failed 相位，文案前缀互斥（Disconnected · vs Connect failed ·），并补负向单测。
- 真窗口证据（wave-c-1）仍有效：当时循环 2 由 wait_connect_failed_label 钉住 Connect failed 文案，ax-tree-connect-failed.txt / ax-tree-disconnected-connect-failed.txt 已记录该值。下次重跑会写出 assert-connect-failed.json。

## 3. 定向回归（全绿）

- python -m unittest test_ui_wave_c_tools test_ui_wave_b_tools test_ui_wave_d_tools：42/42 OK（wave-c 15 新增 + wave-b 17 + wave-d 8，既有回归零回退）。
- bash -n scripts/ui-wave-c-connect.sh 通过。
- 未跑 cargo：本波写入集纯脚本/Python，desktop 二进制与源码无漂移（git_head=b744550）。

## 4. U2 真窗口门禁：🟢 通过（2026-08-27 15:02 +08，屏幕未锁）

- 命令：scripts/ui-wave-c-connect.sh run --out docs/ui-review/r2-wave-c/u2 --label wave-c-1（exit 0）。
- 五相位断言全绿：disconnected（drop-socket 后 reconnect 在场、25 条时间线保留、空态引导缺席）→ reconnected（重连后 reconnect 缺席、connection-status=Connected、会话重开、timeline 25 条恢复）→ disconnected-host-stopped（host 停机）→ disconnected-connect-failed（停机窗口内重试；当时相位名仍是 disconnected，Connect failed 文案由 wait_connect_failed_label 钉住，见 ax-tree-connect-failed.txt）→ reconnected-host-restart（restart-host 后重试成功，settle seq 2→5→6，timeline 恢复）。run-manifest.json structural_pass=True。
- 证据归档 u2/：五相位 assert-*.json / ax-tree-*.txt / geometry-*.txt、六条 action-*.txt、barriers 与日志。
- 已知非阻塞项：composer-height 各相位仍为 156（合同 88–94），即 F-09 视觉漂移（R5 范围），断言 blocking=false，与 Wave B 登记一致。

## 5. R2 退出标准盘点（本波后实态）

| 退出标准 | 实态 |
| --- | --- |
| 1440×1024 壳层结构 100% 对齐、区域几何在容差内 | ✅ 结构门禁 Wave A/B/C 全 PASS（assert-final / 五相位 / 本波五相位） |
| 无白带、重复 titlebar、布局跳动、面板溢出或主操作遮挡 | ✅ F-01/F-02 落地并有 layout invariant 钉住 |
| State A/B shell 各区域 SSIM ≥0.99 + 结构/overlay 人工复核 | ❌ Wave A 实测 9/9 zone 小于 0.99（0.65–0.81）；分区内容依赖 F-03/F-04（R3）、F-05（R4）、F-09（R5）、F-12（R6，State B 前置），R2 范围内无法达成 |
| 启动、连接失败、失焦、resize、Inspector 开合可重复模拟操作测试 | ✅ 本波补齐连接失败重试后全覆盖（Wave B 五相位 + 本波双循环） |

结论：R2 范围内可做的实现与测试已清零。2026-08-27 用户拍板 **a**：R2 以壳层结构门禁为准退出，分区像素 SSIM 移交 R8 汇总（见 [plan/R7-R8](../../../plan/R7-R8-ui-quality-gates.md#3-视觉终局门禁) 与 [history R2](../../history.md#r2--window-shell-与全局视觉系统2026-08-27)）。R3 可开启。
