# GUI3-02 · Composer 元信息与模型入口人工验收

> 状态：✅ 完成（模拟验收）；独立 review PASS；真实平台边界待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收项目信息、模型 chip、Context 与无项目首页文案。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 验收模型触发器、项目只读信息与无项目正向入口；首页与已有任务的文案符合真实状态。
- Context 只在目录提供 context_window_tokens 时出现，缺失数据不虚构；空闲态不常驻故障文案。
- 宽窄窗口和三档字号下元信息自然换行、模型菜单与发送 / 取消仍可操作。

## 完成条件

- 用户确认元信息、入口与当前任务 / 模型目录一致。
- 缺数据、无项目和宽窄字号场景有真实像素与交互证据。

## 完成记录

2026-10-11：按用户「人工验收使用模拟」授权执行。基线 `322f50969a56c74d630736418639d0ef8db80e55`；执行对话 `01a128d7-929c-7a92-9381-ed3e0f0b17d5` 使用 `xai/grok-4.7 xhigh`，完成 turn `01a128d7-9531-7d43-b7cc-d648565a8ea8`，error=null。本次仅在四条既有 GPUI 回归中补断言，同批更新 Desktop 包级 Spec；生产行为没有变化，没有新增测试体系。

| 模拟输入与事实 | 正式产品路径与结果 |
| --- | --- |
| 1440 / 1080 宽度 × 100% / 125% / 150%，当前任务无项目且目录 window 为正数 | 实际元信息布局与 AX 同源：无项目 chip、绑定项目入口与 `Context · — / 128000` 均在元信息框内且互不重叠。无会话首页显示正向新建入口，不画 Context 或文件工具故障行。 |
| 已选模型，目录 window 分别为 `0`、`None`，刷新布局 | `composer_context_meter_visible` 同时控制 render / AX，不发布 `composer-context`；这些缺数据断言取代表场景，不声称逐组合覆盖。 |
| 项目入口 Enter 打开，第二次 Enter 选定项目 | 正式 `ProjectTask` handler 改筛选并尝试 `create_task`；当前任务仍无归属、草稿不变、无项目 chip 保留且出现筛选隐藏提示。fixture 无 client，不登记 Host 新建成功。 |
| 将任务归属投影为项目，再清掉 active session | 已有任务归属优先；无会话时回落筛选项目，显示 `Workspace · Example` 与新建任务入口。0 window 只隐藏 Context，不抹掉项目名。 |
| 运行中点模型 chip，再 AX Press Cancel | 正式菜单 gate 阻止打开；Cancel 可激活，触发取消 handler 后仍保留 `run-keep` 与草稿、Send 不出现。无 Host 回执，不能宣称取消完成。 |
| 复跑已存在的首页 / 任务切换 / Send 与跨面板路径 | 项目 chip、草稿归属、模型与供应商管理入口、离线禁发及本地发送锁保持；Send 的本地状态不证明 Host RunStart。 |

验证只运行一次 Cargo：

```text
cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders -- project_task_guidance_preserves_context_and_wraps composer_layout_keeps_controls_in_card_and_ax_aligned welcome_and_rail_follow_actual_layout conversation_bubbles_and_model_navigation_preserve_draft workbench_home_rail_projection_ax_and_keyboard main_path_keeps_draft_ownership_across_surfaces
```

退出码 0，6 passed / 0 failed / 288 filtered out，测试 26.02s，构建 1m 32s。`rustfmt --check --edition 2021 apps/desktop/src/ui/accessibility/app.rs` 最终退出码 0；指挥 `git diff --check` 与相对链接检查通过（121 项，缺失 0）。已有同版路径可以复用，本轮计数仅上述六条。

证据：`/tmp/pawork-gui3-02-composer-metadata/evidence/` 的 `cargo-test.log/.exit`、`rustfmt-check.log/.exit`、`mapping.md`、`source-boundary.md`、`recovery.md`。首次格式检查因新增 assert 换行退出 1，执行者修复后覆盖了原日志；指挥已从原 turn 恢复该次命令、完整输出与子进程退出码，保存在 `rustfmt-initial-failure-recovered.json`。这不是再次制造失败或产品回归。

模拟只证明正式 GPUI 渲染布局、输入 handler 与本地投影；真实窗口像素、原生 AX、系统输入、真人确认、真实 Host / Provider、建会话 / RunStart / 取消回执仍待验。无 client 的异步 OperationFailed 不被当作必达或成功状态。

独立 review（2026-10-11）：`gpt-6.1-sol max` 对话 `01a128eb-265f-7a90-a438-49b7d4cb590f` 的 turn `01a128eb-2ab5-7813-aa6f-278af023c0df` completed、error=null，最终 PASS，阻断 / 非阻断问题均无。受审快照 20794 字节，SHA256 `4c9e3ac0c5e41a38817b2121290534e36ca6d69a60f630446116f7e23ee1f657`；reviewer 核对四文件和原始工具输出。PASS 后仅同步本计划与 ROADMAP 的审查 / 完成状态，源码与包级 Spec 不变。审查结果保存为证据目录 `REVIEW-PASS.json`，精确提交记录保存为 `COMMIT.json`；本项按授权关闭模拟验收，真实平台边界继续保留。

```text
Validated: 上述定向 Cargo（exit 0，6 passed）；定向 rustfmt（修复后 exit 0）；git diff --check；相对链接检查（121，missing=0）
Targeted regressions: 缺 Context、项目归属 / 筛选与首页文案、宽窄字号元信息几何、模型管理与运行中 gate、取消不伪造终态、Send 本地状态与草稿归属
Full workspace gate: NOT RUN
```
