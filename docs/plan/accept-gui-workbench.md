# GUI2-01 · 工作台壳、首页与侧栏人工验收

> 状态：✅ 完成（模拟验收）；独立 review PASS。真实 Host、真窗口、系统缩放、真实像素与真实 Provider 待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收首页、Header 和侧栏在真实任务、项目与连接状态下的可达性。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 验收首页新建入口、项目 / 无项目任务切换、侧栏分组与折叠；任务标题、草稿和当前选择按现行合同保持。
- 在宽窄窗口与 100% / 125% / 150% 字号下观察 Header、侧栏和首页的真实像素，入口可达、文本无意外裁切。
- 用真实 Host 连接状态核对 Local / Settings、断线说明和重试入口，不把正常空闲状态呈现为错误。

## 完成条件

- 上述壳层路径有用户验收结果，窗口状态与 Host / 任务事实一致。
- 发现的残留问题登记具体复现条件；未解决项不标完成。

## 完成记录

2026-10-11 在真实 AppView 投影、GPUI `TestAppContext`、AX 分发与键盘路径中模拟验收，基线 `c2ed48a69571f46d2b7312dbfdef01acc567c2fe`。产品代码无改，新增 `workbench_home_rail_projection_ax_and_keyboard` 并同步 desktop Spec。

- **模拟验收通过**：首页 Ghost New task 的 AX Press 在无 Host 回执时不建会话，焦点进入 Composer；Snapshot 在无项目 Free notes 与 Alpha 间切换时标题、composer workspace 与草稿保留；分组往返不改当前选择；项目头 AX Press 与 ←/→ 可折叠 / 展开；Settings 往返后标题、选择与草稿保留。已连接空闲不出现重试或错误；Failed 底行为 Local · Disconnected，主区 notice 保留测试 Host 拒绝原因；Connecting 清旧错且不显示重试。
- **布局边界**：150% 窄窗下连接状态底边越过 AX sidebar 内容框，但左栏渲染铺满窗口、StatusBar 只占右侧，底行仍在窗口内、不越栏宽、不盖项目头；断言按真实几何改写。
- **自动门禁通过**：定向 cargo 过滤集 6 passed / 0 failed / 283 filtered，exit 0；`rustfmt --edition 2021 --check apps/desktop/src/ui/accessibility/app.rs` 与 `git diff --check` exit 0。
- **待验边界**：真实 Host、真窗口、系统缩放、真实像素与真实 Provider 未验；本记录不登记为真人窗口验收。

证据：`/tmp/pawork-gui2-01-workbench/evidence/`（命令、输出、结论、diff 与 SHA256）。独立 review（Sol max，thread `01a1277c-be1c-7a12-a358-94a9175befdd`）turn `01a1277c-c159-7653-a9e6-caba5d28292b` PASS。本地提交由指挥用 `/tmp/pawork-final-commit.py` 精确执行；提交哈希记录在证据 `COMMIT.json`，不推送。
