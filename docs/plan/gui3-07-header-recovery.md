# GUI3-07 · Header 动作与断线层级人工验收

> 状态：✅ 完成（真窗口与 GPUI 模拟验收）；初审记录修正后独立复审 PASS；系统按键 / 悬停像素与真实 Provider 待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收 Header 导航、状态 chip 与断线恢复入口的层级。

## 当前依据与读取范围

本次按用户授权模拟人工验收，以现行合同、真实 GPUI 窗口和临时 Host / Git 事实核验。复用 GUI3-05 原构建；其基线 `3d334a1191d31ad6f504fa43796fd3414351a181` 到本项执行前 `a60f85af5acd78727d6d73ea2f70e7bda062de1b` 的生产源码未变，唯一 Rust 差异是 GUI3-06 的 `mod tests` 扩充。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 验收查找 / 回合图标、tooltip 和快捷键；branch / live chip 只在有真实数据时显示，不留空槽。
- 制造真实断线并核对主区原因与 Primary 重试，底部 Local 行保留较轻的重试入口，原始错误不挤入底行。
- 验收 Inspector 重开与 Activity 入口，以及恢复后的当前任务、草稿和焦点保持。

## 完成条件

- 用户完成 Header 导航与断线恢复，Host 状态和窗口反馈一致。
- 操作主次明确，错误原因可访问，宽窄字号下入口不丢失。

## 完成记录

2026-10-11，Grok 4.7 xhigh 执行真窗口与 GPUI 模拟验收；用户授权模拟人工操作。本项未修改生产代码、测试或 Spec，未跑 Cargo。Sol 6.1 max 初审发现导航 / tooltip 证据、重连焦点表述两项 P2，以及像素间距测量 P3；记录已按原事实修正；同一 reviewer（thread `01a129d4-5fdd-74c2-84bb-e249da10d0b6`）在 turn `01a129e4-c463-7242-9c7a-910b1dc5f73c` 复审 PASS，阻断与非阻断均无。

Header 查找 / 回合入口的真窗口可见性已核对；导航和快捷键复用同日 [GUI2-04](gui2-04-conversation-navigation.md) 已审的 `conversation_find_identity_reading_and_turns` 模拟回归：正式 GPUI 输入覆盖 Header / Cmd+F 开关、命中循环、回合滚动与 Enter 定位、切任务清理和草稿保持。tooltip 本轮仅核现行源码与 AX 文案，系统 Cmd+F 和悬停像素未验。alpha 会话显示真实 Git 分支 `main`，切换 beta 无分支时不留 chip。临时 fixture 原先仅在内存绑定 workspace，本轮在临时 SQLite 的该会话补齐 `workspace_id` 后正式 Host 查询返回 Changes ready / 4 files；改前数据库有备份，未改用户数据。live 正向复用同日已审 GUI3-05 的 `100-cancel-running.png`、`ax-after-send.txt` 和 Host `serve.log`：真实 Mock Run 显示蓝点「运行中」与 Cancel，Provider stream 调用存在；本项空闲重连后没有 live chip。Inspector 重开同样复用 GUI3-05 的正式 `inspector-expand` 回执和 AX 记录。

本次补验 Activity → Changes：收起 Inspector，点击「动态」及 `activity-open-changes`（4 files · +9/−14），Changes 选中并聚焦。Host 停机后主区显示原始原因 `connection is closed` 与 Primary「重试连接」，底 Local 行只显示「本地 · 已断开」及 Ghost「重试」。分别通过主区与底行入口恢复；底行补测先停 Host `67270`、启动 `68613`，窗口仍断开，点击 Ghost 后才连接成功。当前任务、`main`、4 files 和原草稿 `gui307-draft-keep-me` 保留，未重输草稿；最后手动点击 Composer 后恢复焦点，此图不证明重连自动回焦。

检查 1440×1024、1080×720 窄窗及 150% 字号。窄窗正式点击齿轮可打开 Settings，无需产品修复；原像素间距测量漏算末尾文字，不作为结论。经 GUI 恢复 `zh` / 100%，偏好 SHA-256 与操作前一致。自身 Desktop `66085`、Host `67270` / `68613` 已按身份核对后停止，socket 消失；Host 等待会话退出码 0，用户既有三个进程仍在。

证据位于 `/tmp/pawork-gui3-07-header-recovery/evidence/`：`WORKER-RECORDS.json` 保存三轮原工具输出与输入；`header-alpha-branch.png`、`header-beta-long.png`、`activity-to-changes.png`、`disconnect-main-retry.png`、`disconnect-ghost-visible.png`、`ghost-retry-connected.png`、窄窗原图和 `stop-final.log` 对应上述路径。Host 日志在同级 `fx/logs/serve.log`。新截图实际命令退出码 0；最初 `shot-connected.exit=1` 和递归 AXApplication 记录不计成功证据。复用的 live / Inspector 原证据在 `/tmp/pawork-gui3-05-svg-icons/evidence/`，原构建退出码均为 0。

导航补采先出现 AX 递归与「连接中」；在已经启动的自身窗口执行 AXRaise 后恢复连接，Header 的 AX 描述与源码一致。`timeline-find` 的 AXPress 回执为 0，但随后 AX 树塌缩，无输入框或命中计数；未继续输入、Cmd+F、Enter 或回合选择，不计真窗口导航成功，也未重输已退出进程的旧草稿。原记录在 `NAV-RECORDS.json`、`nav-before-find.png`、`nav-press-find.txt`、`nav-find-open.png`；后者不作为查找打开的成功证据。自身 nav Host `70127` / waiter `70126` 与 Desktop `70142` 已按身份停止，`nav-cleanup.txt` 与 Host shutdown 日志有独立事实，用户进程保持运行。

Validated: Activity → Changes、两级重连与草稿保持；导航 / 快捷键复用已审 GPUI 模拟回归；Host 原日志、截图及退出码、偏好恢复和进程回收；文档 diff / 相对链接检查。Targeted regressions: none（产品代码未变）。Full workspace gate: NOT RUN（当前未设置全量门禁）。Mock Provider 与临时数据验证的是产品集成和窗口操作；系统按键 / 悬停像素、真实 Provider 与系统 IME 等环境边界仍待验。

独立复审单 poll 保存为证据 `REVIEW-PASS.json`；指挥在冻结 `FINAL.json` 后精确本地提交，提交哈希见 `COMMIT.json`，不推送。
