# R5 Wave A — Composer 结构与 F-09 收口记录（2026-08-28）

> 状态：🟢 结构门禁通过。SSIM 记录值远低于 0.99，按 R3/R4 先例移交 R10 终局门禁（待用户在 R5 收口时拍板追认同一安排）。

## 范围

F-09 Composer 高度漂移收口 + 两行结构重排（输入区 + footer）+ Send/Cancel 同槽互换 + 诚实缺省（reasoning / 附件 / 进度条 / queue 不画）+ hint 行拆除（placeholder 状态机 + footer 瞬态 status_hint）。零 wire 变更。

## 关键结果

| 项 | 之前 | 现在 | 证据 |
| --- | --- | --- | --- |
| Composer 常态总高（真窗口 AX） | 156（88 被当输入框 min） | **91.0（合同 88–94，PASS，blocking）** | state-a-3/assert-final.json |
| footer 控件 | 上行 ≈34 且 Send/Cancel stretch ≈88 | model 28（max_w 220 truncate）/ workspace Label（max_w 180 truncate）/ ContextMeter 同行 / Send 32×32 圆形 | state-a-3/current.png |
| Send/Cancel | 两宽按钮并列 | 单槽 32×32 互换（idle Send ↑ / running Cancel ✕），单 composer_action_focus，element id composer-action；AX 节点 send/cancel 随态互换（U2 兼容） | input_area.rs、accessibility/app.rs |
| 提示行 | 独立第三行（占高） | 拆除：状态机文案入 placeholder；status_hint 为 footer 瞬态 Label（max_w 360 truncate） | input_area.rs |
| Terminal 输入 | 与 Composer 共用 88–220 clamp 与 element id | TextInput 参数化：terminal-input + 28–220 独立 clamp | text_input.rs、mod.rs |

## 测试

- `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` → **119 passed / 0 failed**（worker 两轮：初版 118 + 评审修复后 119）。
- 新增真实断言：placeholder 状态机表、panel clamp（输入预算 = 220−1−16−8−32）、terminal clamp 独立、Send 32、单 tab stop composer-action、AX 面板高公式。
- 评审（grok_reviewer）P1×3（焦点 stranded / footer 鲁棒性与 tautology 测试 / Terminal 耦合）与 P2×2（zones coverage / Spec 不同源）全部修复后复绿。

## 真窗口结构门禁（主代理执行）

| 轮 | label | 结果 | 说明 |
| --- | --- | --- | --- |
| state-a | r5a-1 | 结构全 PASS；visual exit 2 | composer current h=88 vs reference 110 → coverage 0.80 < 0.85 输入错误，触发 zones 修正 |
| state-a-2 | r5a-2 | 结构全 PASS；visual exit 1 | zones 修正后首轮完整记录 |
| state-a-3 | r5a-3 | 结构全 PASS；visual exit 1 | 评审修复后复跑，**本轮为准** |

结构 PASS 含：composer-height 91.0 ∈ [88,94]（blocking）、composer-above-statusbar、focus-start=composer-input、focus-composer-after-select、三栏/StatusBar 骨架。

## SSIM 记录值（r5a-3，移交 R10，不得追认为通过）

- composer-left 0.423 / composer-right 0.619（coverage 0.8273，min_coverage 0.80）。
- 全屏辅助 0.658；其余 zone 与 R4 记录一致（taskrail 0.694 / header-left 0.940 / header-right 0.883 / timeline 0.679 / inspector-body 0.599 / inspector-right 0.740 / statusbar 0.617）。
- 主因：fixture 演示内容形状差（模型名、Context unavailable、placeholder 文案 vs 定稿「Ask a follow-up…」、定稿纸夹/reasoning/进度条为 capability 诚实缺省）。fixture 重塑已在 R3 拍板 c 移交 R10。

## zones.json 变更（state-a / state-b）

- state-a composer-left/right：current y 844→909、h 110→91（真窗口实测几何）；min_coverage 0.85→0.80（>0.50 硬下限；reference h=110 含定稿图面板下 12px 背景带，合同几何 88–94 无此带——属合同差，非放水；SSIM 阈值 0.99 未动）。
- state-b composer-left/right：补 current 矩形（y=909 h=91；91/104=0.875 ≥0.85，min_coverage 不变）。

## 已知限制（移交 Wave B / R7 / R10）

- running 态 Composer 只有 AX/布局同源保证，真窗口 running 截图与 Cancel 槽取证属 Wave B U2（hang-cancelable）。
- 输入内部滚动（>220 面板预算后）与选择/复制/撤销/拖选属 Wave B。
- 真 IME composition 取证走 R7/R10 U3（U1 已覆盖 is_composing 闸门逻辑）。
- placeholder 文案（「Message Pawork…」）与定稿演示文案（「Ask a follow-up…」）差异为已知文本差，R10 统一裁决。
