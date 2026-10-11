# B21 · Desktop 大小格式与词条入口裁决

> 状态：✅ 完成（模拟验收）；本轮自动回归与独立 review PASS；真实系统边界待验。更新：2026-10-12。原裁决基线 fea9012f7d319dffc5486f322bc353c838cf8baa；本轮基线 c49e46311f3933903b32747ca43095fd2fa5d6be。

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
- 按受影响格式与 GUI 场景验证；本轮依用户授权以 GPUI 模拟验收，真实系统边界另列待验。必要变化同步对应 Spec。

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

## 原裁决写入集（2026-10-08）

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

## 原裁决验证（2026-10-08）

未跑 Cargo。运行行为未改，不为注释重跑现有附件、工具组、i18n 或 task_usage 测试。

- rustc 比较 0 到 16777216 无显示差异；16829645 起出现 0.1 差异。
- 主目录与 tr 文案对照：7 对英中完全相同，但语义不属于任务消耗，不复用。
- rustfmt --check 与 git diff --check。

该轮真实 GUI 未做，当时 RV-01、RV-02、RV-03 和 AC-05 的人工验收仍待完成；未把旧窗口、AX 文本或历史通过记成新的像素验收，也未把任务消耗窗口标成 AC-05 已验收。它们后续的模拟完成记录见下节与对应计划。

```text
Validated: rustc -O size compare（rustc --version 为 1.96.0；0 到 16 MiB mismatches=0；16829645 的 f32 为 16.0 MiB，f64 为 16.1 MiB；未重跑）；i18n/tr 静态对照；rustfmt --check；git diff --check。未跑 Cargo。
Targeted regressions: none（注释与文档，无运行行为变化；既有 tool_group_summary 断言未改）
Full workspace gate: NOT RUN
```

## 原裁决审查记录（2026-10-08）

裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

2026-10-08：按源码、上限和逐值比较保留三处实现，并回写对应说明。完成条件里的真实 GUI 场景没有新的像素验收；显示和文案都没有变，静态证据只证明没有需要修改的舍入或词条缺陷。完成标记留给指挥，本次不把 ROADMAP 标成完成。

同日初审修复：收窄 desktop Spec、任务消耗 Spec 和本计划里过宽的 AX 同源句，并改正「16 MiB 正好是 f32 还能精确表示的最大整数」这句。未改完成条件、Rust 源码、运行行为、AX 节点、词条或 ROADMAP 完成标记。真实 GUI、RV-01、RV-02、RV-03 和 AC-05 仍未验收，B21 不关闭、不标 ✅。

独立 review / 复审（2026-10-08）：同一 `gpt-6.1-sol` / max 审查线程 `01a11ad1-eeee-7530-ab98-6662aeee98e0` 在修复后最终 PASS（turn `01a11ae9-67ef-7620-97ae-2ce42fee43e9`，completed、error=null），阻断与非阻断问题均无。受审快照为 37469 字节，SHA256 `734c12c7bbddbcf3dd66e577c5379e0591584e678a8a15b173d569ea61d2d7fb`，指挥已核对与工作区逐字节一致；随后只同步本计划和 ROADMAP 的审查元数据，受审 Rust / Spec 内容不变。复审不替代真实 GUI 场景或 RV-01/02/03、AC-05 人工验收，原完成条件保留。

## 本轮模拟验收（2026-10-12）

按用户「人工验收使用模拟」授权补齐剩余显示验证。执行对话 `01a12c2e-5f0c-7382-bafb-1e4be35ab356`（`xai/grok-4.7 xhigh`），只在现有 `ui/accessibility/app.rs` 测试模块新增 `format_labels_follow_language_on_projected_timeline_and_composer`；生产格式、词条和依赖均无改动。本轮写入集为该 Rust 文件、Desktop Spec、本计划与 ROADMAP。

测试将明确的历史事件 fixture 经 `apply_timeline_page` 投影进正式 Timeline，使用现有 GPUI 窗口的渲染与 AX 链。文本段 `hello\n` 去掉尾部换行后显示 `note.txt · Text file attachment · 5 B`，中文为 `note.txt · 文本附件 · 5 B`；图片段没有字节头。Composer 的模拟附件 DTO 有 2048 个字节，预览分别为 `draft.bin · Text · 2.0 KiB` 和 `draft.bin · 文本 · 2.0 KiB`。两者是独立的测试输入，不是同一物理文件；本用例不验收文件选择与接收。

单工具成功两种语言均只显示 `write_file src/a.rs`。两工具摘要分别显示 `2 tools · read_file src/a.rs · run_command cargo test · 1 completed · 1 running` 与 `2 个工具 · read_file src/a.rs · run_command cargo test · 1 已完成 · 1 运行中`。点击发布的附件头矩形中心进入正式展开 handler。Timeline AX 矩形非空；Composer 预览的 AX 矩形被 composer-options 裁成零，使用 GPUI 自身 debug_bounds 核对绘制边界，未声称该 AX 几何通过。语言仅改测试线程 i18n 态并通知测试窗下一帧，不写偏好，不模拟主窗语言操作的通知。

同版已审证据按各自范围复用，未重跑：

| 来源 | 复用边界 |
| --- | --- |
| [RV-01](rv-01-image-attachments.md) | 图片草稿解码、预览/移除及归属；69 B 来自产品标签函数，不当作本轮 AX 文案。 |
| [RV-02](rv-02-text-attachments.md) | 正式文本附件包装与折叠；英文 AX `proof.txt · Text file attachment · 31 B`。 |
| [RV-03](rv-03-composer-errors.md) | 类型/超限错误的中英文和清错；不推广为已接收文件的大小显示。 |
| [GUI3-01](gui3-01-tool-summaries.md) | 正式历史工具投影、英文摘要、单工具成功 headline 与展开输入。 |
| [AC-05](accept-task-usage.md) | 统计窗口中英文 `tr`、正式 Host/SQLite 查询、未知值与刷新失败；空标题读数不证明系统标题。 |

附件和观测仍由 f64 `format_byte_size` 格式化，时间线内文仍用 f32 `format_size`；复用原 0..=16 MiB 逐值比较与超限申报保留 f64 的静态裁决，未重跑 rustc 比较。观测的 `observation_meta/observation_progress` 调用点已核，本轮没有新增其上屏断言。原已打开统计子窗标题契约来自源码，本轮没有真实标题或主窗语言通知证据。

### 实际验证与证据

证据：`/tmp/pawork-b21-desktop-format-acceptance/evidence`。

```bash
cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders -- \
  format_labels_follow_language_on_projected_timeline_and_composer
rustfmt --check --edition 2021 apps/desktop/src/ui/accessibility/app.rs
git diff --check
```

最终 `desktop-format-labels-4.log/.exit`：exit 0，1 passed / 300 filtered，35.86s。前三轮 `desktop-format-labels{,-2,-3}.log/.exit` 均为 101：共享引用的 String 被移动、预览 AX 矩形被裁为零、列表行缺 debug_bounds selector。核对实际元素与 GPUI 观测后，Timeline 保留 AX 几何断言，预览改用自身绘制边界；不存在的列表 selector 不作布局证据。格式检查先有换行差异（exit 1），定向格式化后 `rustfmt-check-2.exit` 为 0；两次 diff 检查为 0。辅助补丁参数错误改用直接补丁文本恢复，未把失败登记为成功。

真实 Host/Provider、原生窗口像素、系统语言切换、系统 AX bridge/VoiceOver、系统文件选择器仍待验。本轮只证明测试平台的投影、渲染文案和输入 handler，不把历史 fixture 记为真实工具执行。

Validated: 上述定向 Cargo、rustfmt 与 diff 检查通过；已审同版证据和静态调用点按所列边界复用。

Targeted regressions: `format_labels_follow_language_on_projected_timeline_and_composer`，一条 GPUI 主路径。

Full workspace gate: NOT RUN（当前未设置全量门禁）。

本轮独立 review：`gpt-6.1-sol max` 对话 `01a12c41-e17d-7220-8137-914393212d07`，turn `01a12c41-e5e4-7e01-8cac-25360eeb0b1f` completed / error=null，PASS，阻断与非阻断问题均无。受审四文件快照 23,424 字节，SHA256 `3e389bfb635a3e97ad9184f7f1df94982f6f6024be09eeb1db796cbdb2574a73`；指挥核对逐字节与全部 hash 一致后，仅同步本计划与 ROADMAP 审查元数据，源码和 Desktop Spec 内容保持受审版本。
