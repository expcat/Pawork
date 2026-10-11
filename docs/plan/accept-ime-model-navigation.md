# AC-02 · IME、模型目录与跨面板交互人工验收

> 状态：✅ 完成（模拟验收）；初审 P2 引用修正后独立复审 PASS；系统 IME 与真实平台边界待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

闭合系统 IME、目录操作及字号、错误和焦点的交互验收。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[GUI 设计](../gui-design.md)、[Settings Spec](../spec/settings.md)、[Desktop Spec](../spec/desktop.md)。

## 验收范围

- 验收换行后的系统 IME，模型目录鼠标、管理搜索和 Switch 键盘操作。
- 覆盖宽窄窗口与三档字号，核对错误和字号反馈共存及跨面板焦点路径。

## 完成条件

- 真实组合输入不被误发，键盘和鼠标操作结果准确。
- 字号、错误与焦点反馈可并存，真窗口证据注明环境与输入路径。

## 完成记录

2026-10-11：按用户授权执行模拟验收，系统 IME 与真实平台部分另列待验。IME 执行对话 `01a12b08-5670-7df2-9a49-a0a8b9e79ef6`，目录续接对话 `01a12b65-b788-7602-ba49-57187165be10`，均使用 Grok xhigh；独立 review / 复审已通过。生产行为未改。

`newline_ime_does_not_send_while_can_send` 使用正式 AppView，先核普通草稿的 `can_send` 为 true，再经 NewLine action 换行。preedit `ni`、update `nih` 和 commit `你` 走 `EntityInputHandler`；组合中直接调用 Enter 的 `on_send_message`，文本与 undo 保持，发送 / pending / run 状态不变。commit 恰好新增一次 undo，提交本身不发送；Undo / Redo 能往返换行后的草稿。连接投影只用来打开发送 gate，socket 不存在，不证明 Host RunStart。

定向命令 `cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders newline_ime_does_not_send_while_can_send -- --test-threads=1` 实际 exit 0，1 passed。首次编译因修正字符串中的双反斜杠被主动停止，exit 143；定向格式检查首次 exit 1 后完成格式化。执行者未把这些命令落盘，指挥从实际工具输出保存原件于 `/tmp/pawork-ac02-ime-model-navigation/evidence/IME-tool-original.json`，没有重造运行日志或重复已通过测试。

宽窄窗口、三档字号布局及跨面板草稿 / 焦点路径复用 [GUI3-02](gui3-02-composer-metadata.md)、[GUI2-07](gui2-07-main-path-acceptance.md) 的同版已审证据。这两项不覆盖字号反馈与错误共存，该证据改为下述现有回归。目录搜索 / Switch 的 GUI2-06 证据只覆盖本地焦点、pending 与 stale gate；本项另以真实 socket 补齐正式命令与回执结果。

`settings_catalog_search_and_switch_receipts` 让正式 AppView / controller / client / LocalTransport 连接 TempDir 中的模拟 Host，真实 Unix socket 承载请求与回执。经 `on_open_settings` 入口打开 Settings，鼠标点击供应商展开与 Manage models，`EntityInputHandler::replace_text_in_range` 搜索 `flash`，只保留该模型。鼠标点击实际 Switch 关闭 `glm/flash`，显式聚焦同一 Switch 后经 GPUI Space 重新启用；Host 捕获恰好两次 `set_model_enabled`，供应商 / 模型及 `enabled=false/true` 均匹配。每次回执后 pending 清除、目录 enabled 收敛，Composer 候选源移除 / 恢复 `flash`，另一个模型保持。

目录定向命令 `cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders settings_catalog_search_and_switch_receipts -- --test-threads=1` 实际 exit 0，1 passed，运行 7.82 秒。日志与真实退出码在 `/tmp/pawork-ac02-ime-model-navigation/evidence/settings_catalog_search_and_switch_receipts-20261011-230407.log` 及其 `.exit` 文件；编译警告原样保留。目录格式检查首次未通过，格式化后 diff 检查通过。最后仅清理未使用 import 与 helper 的 `allow(dead_code)`，不改动作、断言或 wire；定向 rustfmt 与 diff 检查均 exit 0，不因无行为变化重复已通过的两条回归。

独立初审对话 `01a12b88-b39b-77c3-8807-52d50522663c`（Sol max）指出 1 项 P2：字号反馈与错误共存错引为 GUI3-02 / GUI2-07。本项更正引用；[GUI4](gui4-status-empty-terminal.md) 的原始 `gui4-statusbar-existing.log/.exit` 已包含该回归（5 passed / exit 0），当前 `ui/accessibility/app.rs` 与 `ui/mod.rs` 相对 GUI4 提交 `0c736e55` 均无差异。补证指令发出后执行者已完成一次已有 `text_scale_feedback_expires_without_clearing_errors` 定向运行，1 passed / exit 0，8.50 秒；没有新增或修改测试。新原件为本项证据目录的 `text_scale_feedback_expires_without_clearing_errors-20261011-232643.log` 及 `.log.exit`。它经正式 `set_text_scale` 触发连续反馈，用可控 GPUI 时钟证明旧计时不清新反馈、反馈与错误共存且到期后保留错误；仍不证明系统字号键或像素。修后交同一 reviewer 复审。

本项完成的是授权的模拟验收。Host 的目录与回执是测试边界，不证明真实 Global 配置持久化；系统 IME、系统按键映射、原生 AX 与真实窗口像素仍待环境。

同一 Sol max reviewer 复审 turn `01a12b97-4d11-7453-ad4d-9fcda215805c` completed / error=null，结论 PASS，阻断与非阻断问题均无。受审 `PENDING-R2.diff` 为 42105 字节，SHA256 `a8908a2c01d72d5540ddebc76fcb0e2b54b1bfb46f127c1565fcd0fe17297370`；指挥核对两份 Rust 与 Desktop Spec 哈希未变后仅同步审查状态，冻结 FINAL。

```text
Validated: IME、目录和现有字号反馈三条定向 Cargo 各 1 passed / exit 0；两份 Rust 定向 rustfmt、git diff --check 与 123 个相对链接检查均通过
Targeted regressions: 换行后组合输入的发送拒绝；目录搜索与鼠标 / Space 开关回执；字号反馈与错误共存及到期保留错误
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

