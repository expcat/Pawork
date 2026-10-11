# GUI4 · StatusBar、空态与终端人工验收

> 状态：✅ 完成（模拟验收）；初审 P2 修复后独立复审 PASS；系统边界待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收三栏状态、共享空态和终端直接输入的剩余用户路径。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 核对 StatusBar 左侧项目 / 权威分支、中间实际用量、右侧操作 / 字号 / 连接反馈；未知用量显示未知，字号反馈收起不清除新错误。
- 验收首页、Changes 和 Resources 的共享空态 / 加载态，真实空、不可用、加载中与失败有准确说明。
- 在真实 PTY 验收首次打开、直接文字 / IME 输入、Enter / Tab / 方向键 / Ctrl-C、中文粘贴、清屏和视口尺寸；新建 / 关闭标签以 Host 回执为准，保留独立输入与焦点。

## 完成条件

- 用户完成三栏、空态和终端路径，用量 / 项目与 Host 事实一致，PTY 输出及必要落盘结果支持输入结论。
- 系统 IME 与跨面板结果可引用 [AC-02](accept-ime-model-navigation.md)；不将当前终端之外的完整 VT 能力计为通过。

## 完成记录

2026-10-11，按用户授权执行模拟人工验收。执行对话 `01a12a54-3306-7660-8a25-37531eee72da` 使用 `xai/grok-4.7 xhigh`，基线 `435708eb4fcfe686f0949df0c2fdd6325e2945b8`。生产终端、状态栏和空态行为未改；新增默认关闭且仅 macOS 测试启用的 `gui4-pty` feature 与一条正式 AppView → controller → Host → 真实 PTY 集成回归，并同步 Desktop 包级 Spec。

| 验收内容 | 实际结果与证据边界 |
| --- | --- |
| StatusBar 三栏与用量 | 当前三栏源码、项目 / 分支 getter 与正式渲染调用已核；居中串复用 `run_status_label_` 投影，live 可显示临时估算，终态以持久用量为准，缺失显示 —。4 条既有用量回归通过。左栏沿当前会话项目和 Header 权威分支，不发布重复 AX 节点；没有新真窗口像素结论。 |
| 字号与错误共存 | `text_scale_feedback_expires_without_clearing_errors` 通过：旧字号计时不清新错误，右栏仍保留错误；连接 / 反馈沿现行取值顺序。 |
| 首页、Changes 与 Resources 空态 | 复用同版已审 [GUI2-01](accept-gui-workbench.md)、[GUI2-05](gui2-05-work-panels.md) 和 [GUI3-08](gui3-08-surfaces-motion.md) 的正式 GPUI 投影 / 渲染、空 / 陈旧 / 错误与独立加载骨架证据；本次只补 PTY，不重复该矩阵。 |
| 终端创建与直接输入 | 等待本次项目打开回执后创建；预编辑更新不发送、清预编辑不发送，commit / Enter 经正式输入订阅到 Host，shell 在真实 Git 工作区写出 `ELIF4IUG\n`；此字符串未在输入中出现。 |
| Tab、方向键、Ctrl-C 与粘贴 | Tab 补全后读出真实文件内容；左方向键两次后插入 X 得到 ARRXOW；真实 sleep 子进程收到 Ctrl-C 后退出，shell 存活且随后写出 `KCAB4IUG\n`。中文经模拟剪贴板与正式 Cmd-V 输入到真实 PTY。 |
| 尺寸与清屏 | 各次按正式 canvas 动态测量、防抖和 Host 确认值，以独立 `stty size` 标记核对真实行列；模拟窗口改为 960×640、1600×720 后确认尺寸均变化。此前第 4 次诊断日志记下初始 58 列、62 行；第 6 次成功日志只打印测试通过，没有输出这些数值。Ctrl-L 后原始输出保留清屏前标记，解析屏幕已不显示该标记。 |
| 两个标签与关闭 | 新建回执得到第二个真实 shell；第二标签原始输出不含第一标签清屏前标记，关闭第二标签返回第一标签后，原始输出仍含第一标记且不含第二 shell 的 PID 标记；关闭回执后两 shell PID 均确认退出。输入焦点通过模拟窗口进入产品 TerminalInput，不登记为系统焦点验收。 |

PTY 第 6 次执行通过：`test-gui4-pty-06-passed.log/.exit` 为 1 passed / 0 failed / 296 filtered out，测试 43.60s，exit 0。其原 Cargo 命令输出明确为 `EXIT:0`，外层 shell 也为 0；`TEST-PASS-06-COMMAND.json` 保留这两个层级。随后两项 StatusBar 过滤器共 5 passed / 0 failed / 292 filtered out，测试 50.82s；`gui4-statusbar-existing.log/.exit` 保存原日志和 Python subprocess 的真实 exit 0。本次不计复用旧回归为新运行。

证据目录为 `/tmp/pawork-gui4-status-empty-terminal/evidence/`。原 HTTP 402、收回未实现声明及恢复记录均保留；前 5 次 PTY 失败原日志与实际 exit 101 独立冻结，前两次暴露工作区等待条件，第三次为固定尺寸断言，第四 / 五次为窄终端回显断言。第三次归档误取读日志命令的退出码已修正，保留 `TEST-FAILURE-03-ARCHIVE-CORRECTION.json`。第 4 / 5 次外层 shell 为 0，Cargo 明确回传 `EXIT:101`，没有把外层成功记成测试通过。

格式已收口：新 `gui4_pty.rs` 的 rustfmt --check 为 0；新增模块声明已放在 `mod goal;` 后、`pub(crate) mod i18n;` 前。`--skip-children` 参数错误为真实辅助 exit 1，后改用 `--config skip_children=true`。当前 `mod.rs` 与 HEAD 原文的同项检查均为 1；移除路径与行号标题后输出完全一致，证据为 `mod-head-rustfmt.log/.exit`、`mod-current-rustfmt.log/.exit` 和 `MOD-FORMAT-BASELINE-COMPARISON.json`，剩余均为既有格式差异。本次没有格式化无关生产接线。确定性检查收口后交下述唯一 `gpt-6.1-sol max` 独立 review；提交等待复审结果。

独立初审对话 `01a12afd-e9ea-7570-98b4-449b8a3b49a6`（`gpt-6.1-sol max`）提出 1 项 P2：原第二标签的解析屏幕负断言使用的标记已被第一标签 Ctrl-L 清除，不能排除屏幕误复用。原执行对话改为上述双向原始输出断言；第 7 次同一 PTY 主路径 1 passed / 0 failed / 296 filtered out，测试 12.36s，Python subprocess 真实 exit 0，原件 `test-gui4-pty-07.log/.exit`。新文件 rustfmt 与定向 diff 检查均为 0。没有重跑未改变的 Host build 或 StatusBar；初审与失败记录均保留，修后交同一 reviewer 复审。

同一 reviewer 复审 turn `01a12b06-0e82-7660-bf59-2b1d371bc844` completed / error=null，结论 PASS，阻断与非阻断问题均无。受审 `PENDING-R2A.diff` 为 44544 字节，SHA256 `4c481656344f569cc620efff9231d759c79ceb9d25b77dfb518ab721f75053fd`；指挥核对源码与 Spec 哈希不变后仅同步审查状态、冻结 FINAL 并精确本地提交。

本项证明 GPUI 模拟输入、渲染 / 投影与真实 Host / PTY 的上述路径；系统 IME、原生剪贴板、真窗口像素和真人验收仍待环境。真实 Provider / 账号用量不由这些投影 fixture 或 PTY 结果替代。

```text
Validated: cargo build -p pawork --offline --bin pawork（exit 0）；PTY 定向 Cargo（修后第 7 次 exit 0，1 passed）；StatusBar 现有过滤器 Cargo（exit 0，5 passed）；新文件 rustfmt（exit 0）；mod.rs 格式与 HEAD 基线一致（两者 exit 1）；git diff --check 与相对链接检查（exit 0）
Targeted regressions: AppView 正式输入到真实 PTY、终端回执 / 文件 / 进程 / resize / 标签；终态未知用量、持久用量收敛和字号错误共存
Full workspace gate: NOT RUN（当前未设置全量门禁）
```
