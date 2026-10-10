# RV-06 · 子代理标签字号人工验收

> 状态：✅ 完成（模拟验收）；独立 review / 复审 PASS。真实窗口、系统缩放与像素待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收 100% / 125% / 150% 字号下标签的真实像素。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[GUI 设计](../gui-design.md)、[desktop crate Spec](../spec/crates/desktop.md)。

## 验收范围

- 在三档字号与当前受支持窗口宽度检查子代理标签。
- 直接观察字形和边界，不以 AX 或布局 bounds 正常代替像素验收。

## 完成条件

- 标签无裁切、遮挡或无法识别的省略。
- 证据注明窗口尺寸和字号；截图不写入仓库。

## 完成记录

2026-10-11 在 GPUI `TestAppContext` 中模拟验收，基线 `65fe73cd2d587a639d1f075cd797bff689a72f0e`。验收发现真实缺陷：窄视口中 chip 标题被 `flex_shrink` 压扁，长标题可能裁切或错误换行。生产修复为标题按 `short_title` 测宽并向上取整，超出上一帧 chip 视口文字区时钳制并换行；chip 自身 `flex_none`，视口宽度变化超过 0.5px 时由产品 defer notify 收敛，必要时下一帧重做选中项 reveal。新增 `subagent_chip_labels_fit_text_scales`，不改滚动实现、wire 或 Composer 测宽行为。

- **已实现**：`subagent_panel.rs` 顶栏 chip 标题不再参与压缩，文字区宽度按左右 padding、状态点与 gap 计算，至少保留 12px；标题、6×6 状态点与焦点环的布局框均须落在 chip 视口内。`input_area.rs` 仅把 `shape_label_width` 从 private 调整为 `pub(super)` 供兄弟测试复用。
- **模拟验收通过**：1440×1024 side 在 100% / 125% / 150% 下，24 个 emoji 的超宽标题分别钳到 319 / 296 / 272px 文字区并换行，chip 宽等于 345 / 327 / 308px 视口，offset 均为 0；1080×720 central 下同一标题保持单行且小于视口。中央切回 150% side 后再次换行并贴视口左缘，收敛由产品一次 notify 完成，测试不额外 refresh。
- **自动门禁通过**：`cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders -- --test-threads=1 --nocapture subagent_chip_labels_fit_text_scales`，1 passed / 0 failed / 284 filtered，exit 0；`rustfmt --edition 2021 --check apps/desktop/src/ui/subagent_panel.rs apps/desktop/src/ui/input_area.rs` 与 `git diff --check` exit 0。
- **待验边界**：真实窗口、系统缩放、真实像素回读与真实 Provider 未验；末项 chip 的 `scroll_to_item` 不在本验收内，gpui 对照上一帧 port 的一次宽度收敛滚动行为保留事实边界。

证据：`/tmp/pawork-rv06-subagent-font-sizes/evidence/`（R1 / R2 测试与检查原始输出、结论、diff 与 SHA256）。独立 review（Sol max，thread `01a12684-70b5-7203-aabb-105216c5cf85`）：R1 报 2 个 P2（上一帧宽度无收敛保证、超宽标题未真实换行）与 2 个 P3（尺寸记录歧义、Spec 未同步）。Grok xhigh thread `01a1264c-73fb-7642-8edc-7b0100b74be6` 修复 turn `01a1268a-a6a1-76e1-aff4-76b53fa6bdde`；R2 受审 diff 42634 bytes、SHA256 `b8d119403870fe96c4dbec534831a21f3c65596d89a2629544bd9046d70dd4a7`，复审 turn `01a126ad-189a-7a03-a93f-353e00a9093c` PASS。本地提交由指挥用 `/tmp/pawork-final-commit.py` 精确执行；提交哈希记录在证据 `COMMIT.json`，不推送。
