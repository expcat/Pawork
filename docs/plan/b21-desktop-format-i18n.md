# B21 · Desktop 大小格式与词条入口裁决

> 状态：已裁决保留；静态检查与独立 review / 复审 PASS；真实 GUI 未验收。更新：2026-10-08。基线 fea9012f7d319dffc5486f322bc353c838cf8baa。不标 ✅。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决大小舍入差异、工具组标题和任务消耗词条双入口。

## 当前线索与读取范围

登记项为 `format_size`（f32）和 `format_byte_size`（f64）边界舍入可能差 0.1，单工具组标题不用已删除的 `tool.group_one`，以及 `task_usage` 的 `tr` 与 `ui/i18n.rs` 的 `t()` 并行且词条未入主目录。

[desktop crate Spec](../spec/crates/desktop.md)、[任务消耗 UI](../spec/task-usage-ui.md)、[GUI 设计](../gui-design.md)。

## 实施范围

- 核对实际显示边界和词条消费者，再决定是否统一实现或保留职责差异。
- 只修改相关格式和翻译入口，不进行全库国际化重构。

## 完成条件

- 显示值、工具标题和任务消耗翻译口径一致，没有新增重复词条。
- 按受影响格式与真实 GUI 场景验证；必要变化同步对应 Spec。

## 裁决

三处都保留。不改运行行为，不新增词条，不合并实现。

### 大小格式

`format_byte_size`（`ui/attachments.rs`，f64）服务 Composer 附件字节和观测申报长度。`format_size`（`ui/attachment_blocks.rs`，f32）只服务时间线文本附件的消息内文长度。两者都是 B / KiB / MiB、1024 进位、一位小数。

用与源码相同的分支和 `{:.1}`，对 `0..=16777216`（含 16×1024×1024）逐值比较，显示串差异为 0。这只证明该区间的显示串差异为 0，不能扩成所有消息内文或所有观测申报长度的保证。64 KiB、8 MiB、16 MiB 只是附件接收和观测加载上限：文本附件 64 KiB，图片 8 MiB，观测加载上限 `OBSERVATION_IMAGE_MAX_BYTES` 为 16 MiB。f32 能连续精确表示 `0..=2^24` 的所有整数。

第一处 0.1 差异在 16829645 字节：f32 显示 16.0 MiB，f64 显示 16.1 MiB。这大于观测加载上限。观测预览仍会格式化被拒绝加载的申报长度，所以观测路径必须留在 f64。时间线函数的输入不是这份申报长度；改成调用 f64 也不会改变上限内的显示。不抽公共格式模块，不改单位、阈值或 wire。

Composer 显示文件字节。时间线文本头显示去掉尾部换行后的内文长度，图片段不显示字节。两处数字可以不同，因为输入不同，不是舍入分叉。

### 工具组标题

`tool_group_summary` 保持现状。单工具成功只留 headline；单工具非成功追加同一套状态计数，例如 `run_command cargo test · 1 running`。多工具用 `tool.group_many`（`{} tools` / `{} 个工具`）加前 3 条 headline 和状态计数。`tool.group_one` 不存在，不恢复。render 与 AX 都调用这个函数。既有断言覆盖单条 write_file headline 和单条 running 摘要。

### 任务消耗词条

`tr` 保留。它和 `i18n::t` 读同一个 `i18n::language()`。窗口标题和侧栏入口已经用 `usage.title`。`t` 对未知 key 原样返回；`t2` 只替换两个占位，不是英中二选一。

`tr` 有 96 处调用、86 条不同文案。与主目录英中文完全相同的有 7 对，但 key 都属于别的页面：`subagents.status.failed`、`composer.attachment_image`、`composer.model`、刷新相关 key、`settings.permissions.unknown_mode`、视频能力 key、`tool.group_failed`。字面相同不是同一词条。其余只重合英文或中文，例如任务状态 Cancelled / 取消对不上「已取消」。不迁这些 key，也不把 86 条正文复制进主目录。

经 `button()` / `text()` 包装的文案把同一次 `tr` 结果交给 `PanelAccess`，可视文字与 AX 同源。`task_usage.rs` 的「分组」「消耗类型」「状态」「调用日志」是直接 `.child(tr(...))`，不经这两个包装，因此没有对应 AX 节点；本次不补节点。provider、model、标题、路径和 Host 错误原因仍按原字段显示，不翻译。统计、分页、generation、未知用量和计费来源的句子没有改写。

主窗口 `set_language` 只通知主界面。已打开的统计窗口不因此重绘；下一次本窗口渲染才读到新语言，标题栏保持打开时的 `usage.title`。这与已写明的「已打开子窗口不跟随主窗后续字号」同一类，本次不改。

## 事实

- 工作区基线 fea9012f，开始时干净。B20 的未完成探针验收不作为本项证据。
- 比较程序在 /tmp/pawork-b21-desktop-format-i18n/size_cmp.rs。真实命令是 `wc -l /tmp/pawork-b21-desktop-format-i18n/size_cmp.rs && rustc -O -o /tmp/pawork-b21-desktop-format-i18n/size_cmp /tmp/pawork-b21-desktop-format-i18n/size_cmp.rs && /tmp/pawork-b21-desktop-format-i18n/size_cmp`，外壳 exit 0，不经过 Cargo。`rustc --version` 另行报告 `rustc 1.96.0 (ac68faa20 2026-05-25)`；版本号不是这条编译命令的参数。送审前没有重编译或重跑这次已通过的比较。
- 辅助词条 rg 的 commandExecution `exec-3dd8fdcf-0781-4480-966c-86d19ccc9b97` 在 zsh 解析阶段 exit 1，stderr 为 `zsh:1: unmatched "`，rg 没有执行。随后的精确 rg 与 `i18n_cmp.py`（exit 0）恢复了文案对照。这不是未解决验收失败。初始轮没有看到其它失败的 fileChange 或 commandExecution。
- 本修复轮三个辅助失败已恢复，不是未解决验收失败。exec-04431674-ccc7-4337-adfe-6fa01aa79e80 exit 1，zsh parse error near ')' 与 parse error in command substitution，辅助 rg 未执行，随后精确 rg 成功。exec-7c307528-3d6f-42ed-b16c-8c1c8c290428 外壳 exit 1，末尾 python -c 丢引号后 SyntaxError；links、diff-check、cmp 的 exit 文件为 0，外层不是整条通过。exec-91772218-6110-44d2-9451-e5a21436697f exit 2，show_results.py 不存在；exec-11d9c8e3-cb85-4ab1-831a-58f06bebb625 读到真实日志与退出码，exec-a0f01e2b-73d1-44b2-8c22-4c529ea004cb 精确 rg 与回读成功。
- 未改单位、阈值、wire、schema、golden、依赖，未改 B20 文件，未读真实 auth / config / token / desktop.json。
- 2026-10-08 独立 review 初审后只改文档事实。AX 同源只覆盖经 `button()` / `text()` 包装的文案；「分组」「消耗类型」「状态」「调用日志」仍是直接 `.child(tr(...))`，不登记 AX，不补节点。f32 连续精确表示 `0..=2^24` 的全部整数；`0..=16777216` 的显示串比较不外推到全部消息内文或全部观测申报长度。16829645 字节处 f32 仍是 16.0 MiB、f64 仍是 16.1 MiB，超限申报仍由 f64 格式化。

## 写入集

- apps/desktop/src/ui/attachments.rs
- apps/desktop/src/ui/attachment_blocks.rs
- apps/desktop/src/ui/timeline_entry.rs
- apps/desktop/src/ui/task_usage.rs
- docs/spec/crates/desktop.md
- docs/spec/task-usage-ui.md
- docs/gui-design.md
- docs/ROADMAP.md
- docs/plan/b21-desktop-format-i18n.md

源码只改注释，说明保留原因。

## 验证

未跑 Cargo。运行行为未改，不为注释重跑现有附件、工具组、i18n 或 task_usage 测试。

- rustc 比较 0 到 16777216 无显示差异；16829645 起出现 0.1 差异。
- 主目录与 tr 文案对照：7 对英中完全相同，但语义不属于任务消耗，不复用。
- rustfmt --check 与 git diff --check。

真实 GUI 未做。RV-01、RV-02、RV-03 和 AC-05 的人工验收仍待完成。本次不把旧窗口、AX 文本或历史通过记成新的像素验收，也不把任务消耗窗口标成 AC-05 已验收。

```text
Validated: rustc -O size compare（rustc --version 为 1.96.0；0 到 16 MiB mismatches=0；16829645 的 f32 为 16.0 MiB，f64 为 16.1 MiB；未重跑）；i18n/tr 静态对照；rustfmt --check；git diff --check。未跑 Cargo。
Targeted regressions: none（注释与文档，无运行行为变化；既有 tool_group_summary 断言未改）
Full workspace gate: NOT RUN
```

## 完成记录

裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

2026-10-08：按源码、上限和逐值比较保留三处实现，并回写对应说明。完成条件里的真实 GUI 场景没有新的像素验收；显示和文案都没有变，静态证据只证明没有需要修改的舍入或词条缺陷。完成标记留给指挥，本次不把 ROADMAP 标成完成。

同日初审修复：收窄 desktop Spec、任务消耗 Spec 和本计划里过宽的 AX 同源句，并改正「16 MiB 正好是 f32 还能精确表示的最大整数」这句。未改完成条件、Rust 源码、运行行为、AX 节点、词条或 ROADMAP 完成标记。真实 GUI、RV-01、RV-02、RV-03 和 AC-05 仍未验收，B21 不关闭、不标 ✅。

独立 review / 复审（2026-10-08）：同一 `gpt-6.1-sol` / max 审查线程 `01a11ad1-eeee-7530-ab98-6662aeee98e0` 在修复后最终 PASS（turn `01a11ae9-67ef-7620-97ae-2ce42fee43e9`，completed、error=null），阻断与非阻断问题均无。受审快照为 37469 字节，SHA256 `734c12c7bbddbcf3dd66e577c5379e0591584e678a8a15b173d569ea61d2d7fb`，指挥已核对与工作区逐字节一致；随后只同步本计划和 ROADMAP 的审查元数据，受审 Rust / Spec 内容不变。复审不替代真实 GUI 场景或 RV-01/02/03、AC-05 人工验收，原完成条件保留。
