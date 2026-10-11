# GUI2-06 · 设置查找与行式信息组织人工验收

> 状态：✅ 完成（模拟验收）；初审 P2 / P3 修复后独立复审 PASS；真实平台边界待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收设置页面 / 行查找、定位、权限控件和供应商信息层级。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 查找中英文页面标题、设置行与别名，核对页面 gate、无结果、切页定位和独立输入；离开 Settings 后查询按合同清空。
- 验收分组导航、供应商行式概览与认证详情层级，当前值和错误来源准确。
- 用鼠标与键盘操作现有权限 radio 和 Switch；三档字号下内容、滚动与焦点可达，关闭态 hover 不误示为开启。

## 完成条件

- 用户确认页面 / 行定位与真实设置一致，未绕过断线或能力 gate。
- 供应商、权限和控件结果有实际交互证据；模型目录专项结果可引用 [AC-02](accept-ime-model-navigation.md)。

## 完成记录

独立 review 初审（2026-10-11）：`gpt-6.1-sol` / max 线程 `01a1288b-be47-7b02-9c94-0e9b5270cd9d`，turn `01a1288b-c444-7aa3-b1f6-704793677dd9`，completed、error=null，结论 ISSUES。P2 为测试修改全局语言可能污染并行回归；P3 为滚动 helper 复制产品算法。原 Grok 线程修复 turn `01a12891-a76e-7b33-b689-52bd2e880897` 已 completed、error=null：仅 `cfg(test)` 语言状态改为线程局部，生产仍是全局 `AtomicU8`；滚动 helper 改调产品 `try_scroll_settings_row`，该方法只扩大到 `pub(crate)` 可见性，算法未改。同一审查者复审 turn `01a1289f-bd74-7780-b66b-c07ee3fcf1d0` completed、error=null，结论 PASS，阻断与非阻断均无。受审快照 `REPAIR-PENDING.diff` 为 73,026 bytes，SHA256 `f9bf5865e512d952db255ed5b43c7e4ae1524c8f232444e6734aa7f9e218a093`；通过后仅同步审查状态。下方四次旧日志为初审前证据，修后五次回归另列。

2026-10-11：按用户授权以模拟方式执行人工验收。执行线程 `01a1283f-2e06-7e81-8486-f211e14e67e7` 使用 `xai/grok-4.7` / xhigh，初次实现 turn `01a1283f-316d-7d71-afe6-5242963412f0` 为 completed、error=null。产品行为无改；新增一条 GPUI 回归及测试内辅助函数，更新 Desktop 包级 Spec；修复与最终复审记录见上文。

模拟证据位于 `/tmp/pawork-gui2-06-settings-search/evidence/`。使用当前 AppView、实际布局与 AX 树，Host 边界由连接、权限和供应商投影代替：

- 查找中文「字号 / 审批 / 模型」和英文别名 `font size / proxy`；切页定位不修改字号或审批值。中英无结果文案、断线隐藏不可用页面、查询独立于 Composer、带非空查询离开 Settings 后清空并保留草稿均通过。
- 三档字号的导航选中前后几何一致；1440×1024 / 100%、1080×720 / 125% 与 150% 下设置布局、滚动后 AX 裁剪与断线写 gate 复用既有回归。新增测试调用产品 `try_scroll_settings_row` 并等待布局落定，随后程序聚焦各审批控件，只证明滚动布局和 focus handle 可用，不证明 Tab 遍历或自动焦点路径。审批 radio、供应商连接与详情行不在查找目录；查找结果的定位焦点另由产品 `finish_settings_locate` 路径验证。
- radio 鼠标点击与 Enter / Space、代理 Switch 鼠标点击与 Enter 在无 Host 时验证焦点及本地 pending；stale 后不发布 AX Press，键盘不留下 pending。当前值未乐观更新，不据此推定命令已派发或设置已生效。
- 供应商折叠概览不暴露认证详情、masked credential、endpoint 或 raw catalog error；展开详情区分认证错误与已连接但目录不可用。数据是模拟投影，不证明真实账号和额度。
- 关闭态 hover 只静态核对现行 Switch：关闭态使用 `surface.hover`，开启态使用 `accent.hover`，禁用时不安装 hover；实际像素与动效待验。

实际四次命令均为 `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders <filter>`，各 `1 passed / 0 failed / 290 filtered out`、退出码 0。过滤器与原始日志分别为：

| Filter | 日志 |
| --- | --- |
| `settings_search_gates_focus_controls_and_error_layers` | `main-path-exit.out` |
| `settings_search_locates_rows_without_writes` | `settings_search_locates_rows_without_writes.out` |
| `settings_nav_ax_frames_stay_put_across_selection_change` | `settings_nav_ax_frames_stay_put_across_selection_change.out` |
| `settings_controls_follow_layout_and_scroll` | `settings_controls_follow_layout_and_scroll.out` |

修后五次命令均为 `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders <filter>`，各 `1 passed / 0 failed / 291 filtered out`、退出码 0：

| Filter | 修后日志 |
| --- | --- |
| `language_stays_on_the_calling_thread` | `repair-language_stays_on_the_calling_thread.out` |
| `default_language_is_english` | `repair-default_language_is_english.out` |
| `localize_covers_both_languages_and_unknown_key` | `repair-localize_covers_both_languages_and_unknown_key.out` |
| `language_from_identifier_round_trips_and_rejects_unknown` | `repair-language_from_identifier_round_trips_and_rejects_unknown.out` |
| `settings_search_gates_focus_controls_and_error_layers` | `repair-settings_search_gates_focus_controls_and_error_layers.out` |

非 test 分支经 `cargo check -p pawork-desktop --offline --features gpui/runtime_shaders`，退出码 0，日志 `repair-check-notest.out`。该检查只证明生产分支可编译；并发回归只证明测试线程间语言隔离，不声称验证生产跨线程语言传播。

过程失败与恢复保留在 `NOTES.txt` 和 `first-attempt.log`：首次格式检查失败后格式化；错误的 `|` 过滤器在测试编译阶段因非 static 字符串而 exit 101，改为 String；诊断 sample 重定向失败及后续取消；`main-path.out` 中测试已通过，但 zsh 保留变量 `status` 赋值导致外层 exit 1，不能登记为成功命令，之后精确复跑取得 exit 0。中途误撤指挥活动文档已原样恢复。

真实 Host、真窗口、原生 AX bridge、真实像素、供应商认证 / 额度及真实权限写盘外部效果待验；模型目录专项仍归 [AC-02](accept-ime-model-navigation.md)。本轮没有真人确认、真实 Provider 或全 workspace 门禁。

```text
Validated: 修后五次精确 Cargo filter，各 exit 0、1 passed；非 test Desktop cargo check exit 0；初审前四次定向回归与 rustfmt；git diff --check；Switch hover 静态核对
Targeted regressions: 测试语言线程隔离与中英目录；查找 / gate / 定位 / 草稿；导航几何与三档字号；控件焦点 / pending / stale；供应商错误分层
Full workspace gate: NOT RUN（当前未设置全量门禁）
```
