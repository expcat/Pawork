# AC-05 · Pawork 任务消耗窗口人工验收

> 状态：✅ 完成（模拟验收）；自动门禁与独立 review PASS；原生系统与消费者分别待验。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

闭合 Pawork 统计窗口的用户验收；两个外部消费者拆为独立任务。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[任务消耗 UI](../spec/task-usage-ui.md)、[control-plane Spec](../spec/crates/control-plane.md)、[GUI 设计](../gui-design.md)。

## 验收范围

- 按统计窗口现行口径核对范围、读数、未知/缺失和错误显示。
- 外部消费者分别跟踪 [AC-08 MoMai](accept-momai-task-usage.md) 与 [AC-09 YingMai](accept-yingmai-task-usage.md)，不把本窗口通过算作消费者已接入。

## 完成条件

- 真实账本/Host 数据与窗口结果一致，缺失不伪装为零。
- 用户验收有记录，消费端状态独立保留。

## 完成记录

2026-10-12，执行对话 `01a12bec-be6a-7b52-9435-4d1dab95a9ad`（`xai/grok-4.7 xhigh`）。新增默认关闭的 `ac05-task-usage` 测试 feature 与一条主路径 `usage_window_matches_seeded_ledger`，复用既有 `ui_fixture`。该 example 的 `fixture_core` 打开 `root/data` 临时 control-plane；正式 Desktop controller/client → GuiServer/GuiHostAdapter → SQLite 账本查询保持现有链路，没有新增生产入口或 Desktop Core 依赖。

测试从正式侧栏鼠标入口打开 GPUI 测试窗口，后续输入使用鼠标、焦点后 Enter 与 AX Press handler。101 条调用记录由测试通过系统 SQLite 种入临时账本，独立于 GUI 投影；足以跨越默认 100 条分页。输入包括有无任务关联、文字成功/失败与未知 Token、图片、视频、查询、下载、原生 Run 和超过 24h 的记录。调用记录是模拟数据，不证明真实供应商调用或扣费。

核对结果：API `1.27`，101 条记录、7 次生成、1 次失败、4 次未知 Token、8 次未知费用，已知输入 144、输出 45、图片 2、计划视频 8 秒。通过文字与失败筛选、清除、Novel 下钻后的任务/分部/章节范围、分页追加、24h 范围、未知值详情、复制 ID 与 Esc 关闭。中英文渲染时可访问节点复用同一次 `tr` 文案；「分组」「消耗类型」「状态」「调用日志」标题不登记 AX，测试也核对其缺席。

关键失败路径在备份后将临时账本一条 payload 改为无效 JSON，正式查询返回 `Internal`，窗口显示 `Query failed`；刷新查询开始时既有 report 和详情已清空，失败不会显示旧统计或 `No records match`。该结论只针对刷新，不推广为分页追加失败语义。

### 命令与证据

证据目录：`/tmp/pawork-ac05-task-usage/evidence`。

```bash
cargo build -p pawork-app --example ui_fixture --features ui-fixture --offline
PAWORK_UI_FIXTURE=/Volumes/SSD/Code/Lib/Pawork/target/debug/examples/ui_fixture \
cargo test -p pawork-desktop --offline --bin pawork-desktop \
  --features ac05-task-usage,gpui/runtime_shaders \
  usage_window_matches_seeded_ledger -- --test-threads=1 --nocapture
rustfmt --edition 2021 --check apps/desktop/src/ui/task_usage.rs
rustfmt --edition 2021 --check apps/desktop/src/ui/product_access.rs crates/app/examples/ui_fixture.rs
git diff --check
```

构建 exit 0；最终扩展测试 `cargo-test-ac05-extend-1.log/.exit` 为 exit 0、1 passed / 300 filtered、24.90s。早期 build 漏 `ui-fixture` exit 101，初次 no-run 编译 exit 101，修复后 no-run 与一记录阶段测试通过；原始工具结果保存于 `initial-fixture-build-tool-original.json`、`phase-one-cargo-tool-original.json`，不把失败记为通过。格式检查第一次为坏引号语法、第二次为格式差异，定向格式化后第三次 exit 0；原始 `.log/.exit` 均保留。指挥补核另外两个 Rust 文件的定向格式与 diff，exit 0。

`result.txt` 留存实际汇总和错误，`usage-ledger.sqlite3` 是损坏前的备份。指挥只读复核 `PRAGMA integrity_check` 为 `ok`、`COUNT(*)` 为 101。此前 Python `mode=ro` 打开备份失败，核文件头、权限与路径后改用 `/usr/bin/sqlite3 -readonly 'file:…?immutable=1'` 成功；它是辅助读证据恢复，不是产品查询失败。

### 模拟边界与待验

GPUI 测试窗口的原生标题读数为 `None` / 空串，不能证明真实系统标题或语言跟随行为；已打开子窗标题契约维持原样。测试主动推进帧以核中英文案，未模拟主窗口语言操作对已开子窗的重绘通知。原生窗口像素、系统 AX bridge、系统剪贴板与真实用量/扣费待相应环境；本轮按用户授权记录模拟验收，不称为真人验收。AC-08 / AC-09 仍独立待办。

Validated: 上述构建、定向测试、格式与 diff 检查均通过；备份完整性与 101 条记录只读核对通过。

Targeted regressions: `usage_window_matches_seeded_ledger`，正式查询主路径及损坏 payload 刷新失败。

Full workspace gate: NOT RUN（当前未设置全量门禁）。

独立 review：`gpt-6.1-sol max` 对话 `01a12c27-4893-74e0-b8c9-88b010661bdd`，turn `01a12c27-4d8e-7c40-83e4-11b2154e1aab` completed / error=null，最终 PASS，阻断与非阻断问题均无。受审快照 50,214 字节，SHA256 `e2d53c4e7e2463d64cd550145f4f42e75941cbc1de37bce55c2820aef82fd1a0`；指挥提交前核对逐字节与九文件 hash 一致，之后只同步本计划、ROADMAP 与任务消耗 Spec 状态行的审查元数据。

