# GUI2-02 · 对话阅读层级与 Composer 人工验收

> 状态：✅ 完成（模拟验收）；独立 review 初审 P2 修复后复审 PASS。真实 Host、真窗口、系统 IME、系统缩放真实像素与真实 Provider 待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收用户与助手消息、长内容阅读和模型管理入口。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 观察用户气泡、助手正文、消息菜单和时间的阅读层级；长代码与表格在块内横滚，复制保持原始内容。
- 用鼠标、键盘和 AX 操作消息菜单与模型菜单，空目录 / 无结果时仍可进入模型与供应商管理。
- 进入 Settings 后返回，核对当前任务、草稿与 Run 保持；宽窄窗口和三档字号下 Composer 可读可操作。

## 完成条件

- 真实消息和复制结果支持阅读与操作结论，用户完成上述路径验收。
- 本任务记录阅读与 Composer 结果；系统 IME 专项仍由 [AC-02](accept-ime-model-navigation.md) 跟踪。

## 完成记录

2026-10-11 在真实 AppView 投影、GPUI `TestAppContext`、鼠标 / 键盘 / AX 分发路径中模拟验收，基线 `3cfb7006ca799675e0c6214e451df2a0807295a7`。生产改动收敛 Markdown 横滚布局与宽用户气泡测量，新增 `conversation_blocks_scroll_and_menus_keep_run` 并同步 desktop Spec。

- **模拟验收通过**：短用户气泡与助手操作列共享阅读右缘，用户操作位于气泡上方，空闲助手不为作者行预留空间；长代码与表格在各自块内横滚，视口不超过阅读列宽度；代码复制经鼠标、键盘和 AX Press 三条路径，操作前分别写入不同剪贴板哨兵，操作后精确恢复含 CRLF 的原始代码。
- **模型与设置路径**：模型搜索无结果与空目录仍可进入模型 / 供应商管理；Settings 往返后当前任务、草稿与 Run 保持；1440×1024 与 1080×720、100% / 125% / 150% 字号下 Composer 模型与操作控件留在卡片内且尺寸可操作。
- **实现边界**：表格宽度来自 `shape_line` 实测列宽，同时作为宽度与最小宽度；代码行按实际排版宽度进入同一滚动机制，没有第二层滚动或测试专用生产分支。
- **自动门禁通过**：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders conversation_blocks_scroll_and_menus_keep_run -- --test-threads=1` 1 passed；定向 rustfmt 与 `git diff --check` exit 0。
- **Review 记录**：Sol max 初审发现键盘 / AX 复制可能受旧剪贴板内容假通过（P2），Grok xhigh 在哨兵写入与确认后修复并复跑通过；同 reviewer 复审 PASS，阻断与非阻断均无。
- **待验边界**：真实 Host、真窗口、系统 IME、系统缩放真实像素与真实 Provider 未验；系统 IME 专项仍由 AC-02 跟踪，本记录不登记为真人窗口验收。

证据：`/tmp/pawork-gui2-02-conversation-composer/evidence/`（diff 与 SHA256）。独立 review（Sol max，thread `01a127d1-4e11-7fe0-86cf-5b3622b2a488`）turn `01a127d9-a300-7f92-a33b-10e0f63d6db7` PASS。本地提交由指挥用 `/tmp/pawork-final-commit.py` 精确执行；提交哈希记录在证据 `COMMIT.json`，不推送。
