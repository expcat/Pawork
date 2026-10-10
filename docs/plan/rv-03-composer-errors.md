# RV-03 · Composer 附件错误人工验收

> 状态：✅ 完成（模拟验收）；独立 review / 复审 PASS。真实窗口手动操作、系统文件选择器与像素待验。更新：2026-10-10。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收错误本地化，以及附件类型和大小限制的准确反馈。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[GUI 设计](../gui-design.md)、[desktop crate Spec](../spec/crates/desktop.md)。

## 验收范围

- 以真实允许与拒绝的附件输入检查类型、大小限制和本地化错误。
- 确认拒绝后草稿、焦点和可执行的修正入口仍正确。

## 完成条件

- 界面提示与真实限制一致，无误导成功或吞掉错误。
- 对应输入、错误和草稿状态有人工记录，不重新扩大附件能力。

## 完成记录

2026-10-10 在 GPUI `TestAppContext` 与桌面附件控制器中模拟验收，基线 `d79d1510a6b4da170d513db06a5e9b6df91389d2`。新增 `attachment_limits_keep_draft_focus_and_correction`，并通过仅测试构建可见的 `read_composer_files_for_test` 调用真实 `read_files`；不改生产行为或附件限制。

- **已实现**：一条 UI 主路径回归覆盖真实临时文件允许、图片入口类型拒绝、64 KiB+1 文本拒绝、合法选择清除错误与数量超限；不扩大附件能力。
- **模拟验收通过**：`note.txt` 与有效 1×1 PNG 进入附件列表并保留文本 / 图片分类；图片入口选择文本得到仅图片格式指引，不提 UTF-8；64 KiB+1 文本得到 64 KiB 指引，不提 8 MiB；随后合法选择清除错误并补足 4 个附件，再套用同一结果触发数量上限且不追加。英文与中文错误、Files / Image 菜单提示一致。拒绝后草稿「草稿还在」仍在，焦点留在 Composer，错误不写入 status_hint、不记成发送失败；添加按钮可打开菜单，Files / Image 行有实际布局和 Press，关闭菜单焦点回到添加按钮，并可回到输入框。未激活 Files / Image，避免打开真实系统选择器。
- **自动门禁通过**：`cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders -- --test-threads=1 --exact ui::attachments::tests::attachment_limits_keep_draft_focus_and_correction`，1 passed / 0 failed / 279 filtered，exit 0；`cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders -- --test-threads=1 attachment`，10 passed / 0 failed / 270 filtered，exit 0；`rustfmt --edition 2021 --check apps/desktop/src/ui/attachments.rs apps/desktop/src/controller/attachments.rs apps/desktop/src/controller/mod.rs` 与 `git diff --check` exit 0。
- **待验边界**：系统文件选择器、prompt_for_paths 到后台通道、真人窗口、鼠标 / 键盘实机操作与像素回读未验。既有 `read_files_maps_structured_failures` 覆盖文本允许、图片入口拒绝文本、65 KiB 文本拒绝、PNG 魔数与 5 个文件超限；JPEG / GIF / WebP、目录、根路径、空文件、NUL 与精确边界不在本记录声明为已验；本记录是模拟验收，不登记为真实人工验收。

证据：`/tmp/pawork-rv03-composer-errors/evidence/`（命令、原始输出、结论、diff 与 SHA256）。审查元数据见下；不推送。

独立 review（2026-10-10，Sol max，thread `01a125ea-26e8-7692-8aca-8555194eeaa8`）：R1 turn `01a125ea-293c-7a70-b844-146c5a3d8a39` 报 2 个 P2——TooLarge 只查传入值与刷新前文案、文档及证据夸大既有分类测试覆盖。Grok xhigh thread `01a125a4-d6e2-7bd3-9315-87a690a134ae` 修复后，测试断言实际保存的 TooLarge，刷新后核对类型 / 大小错误英文与中文 AX，合法选择清错后错误节点消失；计划与证据按真实断言收窄覆盖声明。R2 受审完整快照 SHA256 `af02732cd8ffed00cc05ef2b9f4c58b3418526f23a249b6965c53e243e06ca95`，复审 turn `01a125f3-a083-7851-98f9-a3abcc60bb04` PASS，未发现新问题。非阻断边界为系统选择器、后台通道、真人窗口、像素及未列入本回归的其它格式和边界；本结论不代表真实人工验收。

本地提交由指挥用 `/tmp/pawork-final-commit.py` 精确执行；提交哈希记录在证据 `COMMIT.json`，不推送。
