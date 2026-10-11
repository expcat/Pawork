# GUI3-08 · 层次 token 与有限动效人工验收

> 状态：✅ 完成（模拟验收）；初审 P2 修复后独立复审 PASS，真实系统动效待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收深色层次、交互反馈与 Switch / Inspector 的实际动态效果。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 在真窗口观察 canvas、panel、raised、menu 和 hover 的层次，以及正文、辅助文字和焦点的可见性。
- 动态观察 Switch 120ms 位移、Inspector 180ms 开合，以及首帧和跨窄窗时的既定行为；静态截图不能证明动效。
- 验收即时 hover / pressed / focus、Changes / Resources 静态加载骨架和流式末尾静态标记；不把循环动画或系统 Reduce Motion 分支计为已有能力。

## 完成条件

- 用户实际操作并确认有限动效，记录观察环境与结果。
- 层次和动态结果分别有证据；未观察动效时继续待验。

## 完成记录

本轮按用户 2026-10-09 授权模拟人工验收。执行：2026-10-11，xai/grok-4.7 xhigh，对话 01a129e7-c49f-7e33-a5ea-ad72ad1f0663；基线 7834370f73a22123ea836aaaa613047e413ffe94。证据目录：/tmp/pawork-gui3-08-surfaces-motion/evidence/。

- **实际缺口与修复**：正式 Settings 的 Subagents 已加载回执令 Switch 首帧停在右侧 18px，同值重绘却跳到左侧 2px（第二轮原日志与 motion-samples-failed-2.json）。以窗口内、按 id 的 GPUI 状态替换旧进程静态首帧集合；首帧直接到位，同值保留当前运动状态，变值从当前位置继续 120ms ease-out。未改开关的 Host 写入、焦点、禁用或审批语义。
- **动态模拟**：既有 AppView GPUI 测试窗口经正式 Inspector toggle 和模拟 SubagentSettingsLoaded / Confirmed 回执驱动；真实 Instant 加主动 refresh 取得绘制记录。初审补正后的综合第四轮 Switch 首帧与同值均为 18px；切换首帧为 18px，实际采样 62.4ms 为 5px，112.2ms 和 154.3ms 为 2px。采样时刻包含绘制耗时，不将计划中的 40 / 90 / 140ms 当作精确显示时刻，也不据此测定系统帧率。
- **Inspector**：正式 render 宽度与 shell-inspector 几何一致时使用几何：打开中 271.5px、反向 335.5px、收起中 89px、再开 274.5px。反向前后的正式运动样本夹住续接值，未用上一帧与稍后时刻比较来判跳变。开场、收起后与窄窗侧栏 render 值为 0；缺失 / 不匹配的容器 bounds 不作为侧栏宽度，窄窗中央 840px 明确不计侧栏动画。Inspector 的采样方法与 Clone / Copy 仅在测试构建中存在。
- **静态层次与状态**：主题、按钮、焦点绘制代码核对与 [GUI3-05](gui3-05-svg-icons.md) 同版一致；复用其 2026-10-11 三档真窗像素及代表 hover / pressed / focus 输入。正式 AppView 的空列表 Fetching 分支绘制 Changes / Resources 四条骨架，实际布局框为 220 / 168 / 196 / 132 × 8px，Changes 短时重绘保持尺寸；正式流式正文 caret 布局框为 2 × 14px。外部状态由 begin_refresh 和时间线条目注入，没有 Host 请求，也没有真实像素。终态由 apply_files / apply_servers 后的 Ready 空列表、Idle diff 与流式结束后的 assistant_is_streaming 为假，结合唯一挂载条件确认；GPUI 缓存的旧 debug_bounds 不用于节点消失结论。初审补正后，两个面板按现有骨架 id 分开 selector；首次切页前 Resources selector 无缓存，切页后读取本页新框。static-geometry.json 分别记录 source、selectors 与实际 bounds。
- **失败与证据来源**：动态第一轮 exit 101 为不同时刻的 Inspector 宽度比较；第二轮 exit 101 复现同值 Switch 重启；第三轮 exit 101 为编译错误；第四轮 exit 101 将额外重绘的第二帧当作切换首帧。第五轮动态通过。静态第一轮 exit 101 为临时 selector 生命周期编译错误；第二轮 exit 101 为以陈旧 bounds 误判骨架残留；第三轮综合补证 exit 0，但初审指出两个面板的共用 selector 不能区分旧框；分开后第四轮 exit 0。原日志均保留。motion-samples.json 是最新综合轮；误名的 motion-samples.round5.json 实际来自静态第二轮，NOTES 已纠正。第五轮原采样文件被后续运行覆盖，仅从原日志恢复语义记录 motion-samples-cargo-motion-test-5.recovered.json，来源哈希见 root-round5-recovery.json，不声称保留原文件字节。
- **验证边界与格式**：真实系统显示调度、原生窗口动效像素与真人操作仍待验，本轮不改变 Reduce Motion 或循环动效范围。四个独立修改的 Rust 文件定向格式检查通过；mod.rs 的整文件检查仍有五处历史差异，指挥比较当前与 HEAD 的格式差异一致，新增代码没有扩大差异。
- **独立初审**：gpt-6.1-sol max，对话 01a12a40-a1f9-7663-b3fa-3b646b859d89，turn 01a12a40-a54c-7250-88b1-a0ebc5ece36e，ISSUES（P2 一项，无非阻断项）。Switch 修复与调度核对通过；两个面板共用 skeleton-bar-* selector，而 GPUI 保留旧 bounds，Resources 读数可能来自 Changes 缓存，尚不能独立证明本页新绘制。原 Grok 在 turn 01a12a48-193b-74d3-999e-c198cf7e9d77 已按现有骨架 id 区分测试 selector、核首次 Resources 无缓存并重跑同一综合主路径，同一 Sol 复审已 PASS。初审原文见 REVIEW-ISSUES-r1.json。

- **独立复审与收口**：同一 gpt-6.1-sol max，对话 01a12a40-a1f9-7663-b3fa-3b646b859d89，turn 01a12a4e-c181-7443-ad48-31e3e171bfce，PASS，阻断与非阻断均无。复审核当前八文件与 PENDING-R2 冻结一致，确认两个面板独立几何证据有效；修后主路径与格式、链接、diff 记录成立。结论限于获授权的 GPUI 模拟，系统调度、真实动效像素与真人操作仍待验。原结果见 REVIEW-PASS.json；指挥只同步审查状态后本地提交，不推送或发布。

Validated: cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders surfaces_motion_draws_intermediate_switch_and_inspector -- --nocapture（cargo-static-geometry-4.log，1 passed、295 filtered、8.18s、exit 0；第三轮 13.60s / exit 0 保留为补正前记录）；第五轮动态主路径（1 passed、13.33s、exit 0）与同一测试二进制的 settings_models_menu_ax_pins_gates_switches_and_empty_state（1 passed、295 filtered、1.70s、exit 0）保留。定向 rustfmt、基线格式比较、链接与 git diff --check 通过；mod.rs 整文件 rustfmt --check 的历史失败单独保留。

Targeted regressions: 上述正式动态绘制主路径、静态加载 / 流式布局与终态门控，以及既有模型管理 Switch 门控 / 键盘 / 焦点回归。

Full workspace gate: NOT RUN（当前未设置全量门禁）。
