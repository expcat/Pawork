# GUI3-05 · SVG 图标与语义颜色人工验收

> 状态：✅ 完成（真窗模拟验收）；初审 P2 修正后独立复审 PASS。完成：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收图标在真实窗口中的清晰度、语义颜色和操作入口。

## 当前依据与读取范围

本轮按用户 2026-10-09 授权模拟人工输入，并以现行源码、独立真窗口像素、AX 与临时 Host 事实验收。代理模拟与真人操作、真实 Provider 和系统能力分别记录。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 在 Header、侧栏、Composer、Inspector 与 Settings 观察现有 SVG 图标，检查尺寸、对齐和深色背景下的可见性。
- 核对 Primary / Danger 操作上的图标颜色、禁用态以及 hover / pressed / focus，含 Send / Cancel。
- 用鼠标与键盘操作代表性图标按钮，tooltip / AX 名称与实际动作一致；三档字号下图标和文本不互相遮挡。

## 完成条件

- 用户完成代表性入口验收，图标含义、颜色和操作结果一致。
- 真实像素支持可见性结论，不以资产存在或静态 AX 树代替渲染验收。

## 完成记录

本轮执行：2026-10-11，`xai/grok-4.7 xhigh`。基线 `3d334a1191d31ad6f504fa43796fd3414351a181`。证据目录：`/tmp/pawork-gui3-05-svg-icons/evidence/`，截图仅在 `/tmp`。

- **真实窗口与 Host**：当次构建的平行 `Pawork-gui305.app` 通过 `open -na` 启动，最终 Desktop PID 57464、fixture Host PID 57333。正式 AX SetValue 写入 `fixture:hang`，鼠标 Send 进入真实 MockProvider Run，键盘 Space 取消；AX 页脚和 Host `provider stream called` / `provider stream done ok=false` 联核。两进程已精确停止。
- **图标与字号**：100% / 125% / 150% 下 Header、侧栏、Composer、Inspector、Settings 的代表图标由真实像素确认可见、与文字留有间隔；Primary Send 为蓝底白纸飞机，Danger Cancel 为红底白 X，禁用 Send 与键盘焦点环可见。原 `100-send-pressed.png` 已切换成 Cancel，只证明切换后图标可见，不作为旧 Send 按下态证据。
- **偏好恢复**：字号通过正式 Settings 按钮改动后恢复 100%；原值 `language=zh`、`text_scale=100` 与恢复值及 SHA-256 `b65d109b651d2b0731cba388792782c21f646834bebc09ea65b57e72db458e7c` 一致。`--instance` / `PAWORK_DATA_DIR` 不隔离外观或全局认证；首次覆盖 HOME 的尝试保留日志，不计最终隔离证据。
- **补充验收**：原 `100-shell-disabled-send.png` 与裁切图证明归档按钮 hover /「归档」tooltip；按下前 AX 为 `session-archive-fx-ses-beta-pending`、名称「归档」。`press3-archive.mov` 与正式 CUA 坐标按住输入重叠；按住代表帧 `f-072`（8.875s）与 `f-074`（9.125s）仍显示目标任务、项目计数 4；落地帧 `f-095`（11.750s）/ `f-096`（11.875s）才显示目标消失、计数 3 与「撤销归档」，随后正式点击撤销恢复。当前 `100-archive-press3.png` 来自 `f-072`，`100-archive-press3-release.png` 来自 `f-095`；旧报告和错误图片保留为 `before-review`。录像无法可靠区分 Ghost hover / pressed 色差，不作明显变色结论；旧松开后截图及无输入重叠录像保留，不计按下覆盖。初审指出的错误落地帧标注据同一原片更正，不重录。
- **补充进程与 Spec**：复用原构建重新启动的 Host 58522 / Desktop 58576 身份核对后已精确终止；开测前 3 个用户进程仍在。实际按钮为 `COMPOSER_SEND_SIZE=36`，Send 为纸飞机、Cancel 为 X；仅修 Desktop 包级 Spec 同一句旧尺寸与形状描述，未改 Rust 或资产。
- **独立审查**：`gpt-6.1-sol max`，对话 `01a12971-bf15-7352-abae-39614a311e34`，最终 turn `01a1297e-c54f-7782-a318-f8a60dbcd4fb` PASS，阻断 / 非阻断问题均无。`REVIEW-ISSUES-1.json` 保留初审，`REVIEW-PASS.json` 与 `PENDING-R2` 记录修后受审范围；完成状态元数据随后同步。
- **交付边界**：已有生产行为、当次构建与真窗模拟输入验收完成；Provider 为 Mock，不代表真实账号 / Provider 或系统能力验收。录像未可靠分辨 Ghost 色差，不把微弱采样差异宣称明显反馈；无新增功能或发布。

Validated: `cargo build -p pawork-desktop --bin pawork-desktop --features gpui/runtime_shaders`、`cargo build -p pawork-app --example ui_fixture --features ui-fixture`，均 exit 0；正式窗口像素 / AX / Host 事实如上。`git diff --check` 与 `python3 /tmp/pawork-roadmap-links.py` exit 0，121 个链接无缺失；原日志与退出码保存于证据目录。

Targeted regressions: none（本项未改 Rust 行为，无必要新增测试）。

Full workspace gate: NOT RUN（当前未设置全量门禁）。
