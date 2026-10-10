# GUI2-05 · 工作面板与窄窗可达性人工验收

> 状态：✅ 完成（模拟验收）；独立 review 初审 P2 修复后复审 PASS。真实窗口、原生 AX bridge、真实像素、真实 Host、PTY、浏览器与文件系统外部事实待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收 Inspector 的并排 / 中央呈现及已接通工作面板。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 在宽窗、最小窗口和三档字号下验收 Inspector 开合；宽度不足时中央呈现，返回对话后草稿、阅读位置和连接状态保持。
- 按现有 capability 验收 Changes、Terminal、Resources、浏览器与文件面板的已接线路径；刷新、切换、重开和错误 / 空态与真实数据一致。
- 核对 Changes 的文件状态、双列行号和长行横滚；中央面板遇到待审批时可回到对话处理，不隐式批准。

## 完成条件

- 用户完成面板与窄窗路径，所示文件、PTY、浏览器页面或 Host 状态有独立事实支持。
- 只验收当前合同已提供的能力；不可用路径如实记录，完整终端输入另见 [GUI4](gui4-status-empty-terminal.md)。

## 完成记录

2026-10-11 在真实 AppView 投影、GPUI `TestAppContext`、键盘 / AX 分发路径中模拟验收，基线 `c0e61afb62c639ab483a7ed3f0ca6657e431d1dd`。产品逻辑无改，扩展既有 `work_panel_center_mode_reaches_all_tabs_and_restores_conversation` 并同步 desktop Spec。

- **面板与窄窗**：1440×1024 在 100% / 125% / 150% 字号下保持右侧并排；1080×720 三档字号与 1280×720 的 150% 字号进入中央呈现。并排动画在测试执行器中 snap 到生产动画同一终宽，插值仍由既有动画回归覆盖。
- **工作面板往返**：AX 打开后依次切换 Changes、Terminal 与 Resources，返回对话后 Composer 草稿、Timeline 阅读位置和连接文案保持；terminal create / close 的本地 pending 为空。active run 与 pending approval 仅断言本地投影保持，未校验取消 / 审批命令派发。
- **既有面板路径复验**：Changes 文件状态、双列行号、长行横滚、空文件与复制；Resources 空态、断线陈旧与旧代际拒绝；浏览器离线导航边界与生命周期；文件草稿、目录懒加载、保存回执与错误路径；终端已知终态恢复动作。15 个 GUI2-05 过滤器在当前测试二进制上均为 1 passed。
- **自动门禁通过**：`cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders --no-run` exit 0；同一测试二进制执行 15 个过滤器全部 exit 0。修复轮仅调整注释、Spec 与导入顺序，未重跑 Cargo；`rustfmt --check --edition 2021 apps/desktop/src/ui/accessibility/app.rs` 与 `git diff --check` exit 0。
- **Review 记录**：Sol max 初审发现 Spec 把「不取消 Run / 不默认批准」写成自动覆盖，而实际测试只证明本地投影保持；Grok xhigh 修复后同一 reviewer 复审 PASS，阻断与非阻断问题均无。
- **待验边界**：真实窗口、原生 AX bridge、真实像素、真实 Host、PTY、浏览器与文件系统外部事实未验；鼠标横滚、切文件回到左端和完整终端输入分别保留待验或归 GUI4。本记录不登记为真人窗口验收。

证据：`/tmp/pawork-gui2-05-work-panels/evidence/`（命令、输出、diff 与 SHA256）。独立 review（Sol max，thread `01a12832-5e42-7ee3-aa2d-a14c59cffc94`）turn `01a1283a-c374-7712-8acf-73685a3c8a00` PASS。本地提交由指挥用 `/tmp/pawork-final-commit.py` 精确执行；提交哈希记录在证据 `COMMIT.json`，不推送。
