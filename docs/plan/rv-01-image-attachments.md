# RV-01 · 图片附件交互人工验收

> 状态：✅ 完成（模拟验收）；独立 review / 复审 PASS。真实窗口手动操作与系统文件选择器待验。更新：2026-10-10。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收图片缩略图、预览与移除的分离，以及键盘和草稿保持。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[Desktop Spec](../spec/desktop.md)、[desktop crate Spec](../spec/crates/desktop.md)、[GUI 设计](../gui-design.md)。

## 验收范围

- 真实窗口检查缩略图、预览和移除具有独立可达动作。
- 键盘操作、切换任务和发送失败时核对各任务草稿，不把 AX 节点存在当操作已通过。

## 完成条件

- 预览不误删、移除不误开；键盘可完成对应操作。
- 切任务后的附件与文本草稿归属正确，实际窗口结果有记录。

## 完成记录

2026-10-10 在 GPUI `TestAppContext` 的 1440×1024 测试窗口模拟验收，基线 `fa6ffe11dd6c84b7347bbb6e6e2c44f8d7850d95`。task-a 持有「甲草稿」与 69 字节有效 1×1 RGB PNG `shot.png`；task-b 持有「乙草稿」与 4 字节文本附件 `note.txt`。

- **已实现**：新增 `image_attachment_keyboard_actions_keep_per_task_drafts`，不改生产逻辑；现有测试拆分覆盖预览键盘、附件保留与文本发送失败，未把图片缩略图、独立移除键和 `open_session` 后的双草稿归属放在同一路径。
- **模拟验收通过**：布局排空后缩略图路径取得 `RenderImage`：1 帧、尺寸 1×1、解码字节 4，不只依赖缓存键；预览 chip 与移除按钮实测 bounds 均有面积且不重叠。Enter 打开图片预览不删附件，Enter 关闭预览不删附件；切到 task-b 后仅显示乙草稿与 note，task-a 保留甲草稿与 shot。发送目标仍为 task-a 时，迟到 `OperationFailed` 只落在 task-a 且附件不清空；回到 task-a 后 Enter 移除 shot，预览保持关闭，task-b 草稿不变。
- **自动门禁通过**：`cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders -- --test-threads=1 image_attachment_keyboard_actions_keep_per_task_drafts attachment_preview_keyboard_and_accessible_close unassigned_attachment_draft_search_and_failure_preservation composer_send_failure_stays_with_originating_draft`，4 passed / 0 failed / 274 filtered，exit 0；`rustfmt --edition 2021 --check apps/desktop/src/ui/attachments.rs` 与 `git diff --check` exit 0。
- **待验边界**：系统文件选择器、鼠标点击、Escape 关闭、22px 缩略图像素观感，以及运行中 Pawork 窗口内的真人手动操作未验。本记录是模拟验收，不登记为真实人工验收。

证据：`/tmp/pawork-rv01-image-attachments/evidence/`（命令、原始输出、结论、源码 diff 与 SHA256）。独立 review 通过后再补审查元数据并本地提交；不推送。

独立 review（2026-10-10，Sol max，thread `01a12566-3518-75f3-a91b-3cd233559aab`）：R1 初审 turn `01a12566-380d-78e0-a9a4-471513ed3e5b` 报 1 个 P2、2 个 P3——PNG 魔数不能证明解码成功、Cargo 串行参数被第二个 `--` 吃掉、执行者证据的写入集表述过时。Grok xhigh thread `01a12558-5fb3-7162-b1ad-7bb72f2a2008` 修复为有效 1×1 PNG 并断言 `RenderImage` 后，R2 快照 SHA256 `9d45639de0cbd854f78b95b758f2c52abb05f4c6aaa681a51c7685b2288f99c3`；复审 turn `01a12572-e0d6-7300-b496-89351c285eb6` PASS，三项 ISSUES 均消除。结论仅覆盖 R2 与证据，不代表真实窗口或系统文件选择器验收。

本地提交由指挥用 `/tmp/pawork-final-commit.py` 精确执行；提交哈希记录在证据 `COMMIT.json`，不推送。
