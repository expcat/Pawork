# GUI2-03 · 任务与操作快捷查找人工验收

> 状态：✅ 完成（模拟验收）；独立 review 初审 P2 / 复审 P3 修复后最终 PASS。真实 Host、真窗口、原生 AX bridge、系统快捷键真实像素与外部页面导航待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收当前 Snapshot 范围内的任务、页面和安全导航查找。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 通过侧栏入口和既有快捷键打开查找；输入任务标题 / 项目名、选择结果并跳转，结果只来自当前 Snapshot。
- 验收独立查找输入、方向键 / Enter 选择、Esc 关闭回焦和模态键盘隔离，查找不写入 Composer 草稿。
- 在断线与无匹配状态核对范围文案和导航 gate；只允许现行合同开放的页面，不把查找呈现为全历史搜索。

## 完成条件

- 用户完成任务与页面查找，结果身份与当前 Snapshot 一致。
- 输入、关闭和回焦路径有实际交互证据，范围与断线限制无误导。

## 完成记录

2026-10-11 在真实 AppView 投影、GPUI `TestAppContext`、键盘 / AX 分发路径中模拟验收，基线 `d7c694220eba2bee644d10781b6447dd6b0274b0`。生产改动为断线范围文案与对应 Spec / 测试补齐，不引入测试专用产品分支。

- **模拟验收通过**：侧栏 AX 入口与 `Cmd+K` 打开同一查找浮层；空查询按最近更新列出当前 Snapshot 任务，项目 / 无项目结果与已开放页面分组，且不包含设置行；任务标题 / 项目名、中文与大小写过滤、方向键滚动、Enter 跳转、Esc 回焦与模态写操作隔离均走既有产品路径，Composer 草稿保持。
- **范围与断线**：连接范围标明当前任务列表，无结果标明当前列表没有结果，不呈现全历史搜索；断线结果收敛为 Appearance、Advanced 与不依赖 Host 的本地浏览器，双语文案同步为仅本地外观、诊断与浏览器，陈旧任务与 AX 动作执行前重新核对并拒绝。
- **实现修正**：既有断线文案漏列结果集中保留的本地浏览器，已按真实结果集修正；新增断言补齐侧栏入口、范围 / 无结果 / 断线文案与空查询页面集合。
- **自动门禁通过**：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders quick_find_ -- --test-threads=1` exit 0，2 passed / 288 filtered；定向 rustfmt 与 `git diff --check` exit 0。
- **Review 记录**：Sol max 初审发现新增文案断言依赖全局英文语言（P2），复审又发现基线一处已保存提示即时读取 `t()`（P3）；Grok xhigh 将相关断言改为捕获同一 label 后接受明确中英文全文，并复跑通过。同 reviewer 最终 PASS，阻断与非阻断均无。
- **待验边界**：真实 Host、真窗口、原生 AX bridge、系统快捷键真实像素与外部页面导航未验；本记录不登记为真人窗口验收。

证据：`/tmp/pawork-gui2-03-quick-search/evidence/`（命令、输出、diff 与 SHA256）。独立 review（Sol max，thread `01a127ed-9f20-7f41-ae62-945358c94302`）turn `01a127f8-58c7-7601-919a-f711d39aece6` PASS。本地提交由指挥用 `/tmp/pawork-final-commit.py` 精确执行；提交哈希记录在证据 `COMMIT.json`，不推送。
