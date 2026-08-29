# R5 Wave B — 输入交互与 U2 场景（收口 notes，2026-08-29）

## 范围

W-B1 输入交互：shift 选择（SelectLeft/Right/ToLineStart/ToLineEnd）、鼠标点选/拖选、Copy/Cut/SelectAll、Undo/Redo（真实快照栈）、overflow scroll（TextElement 全内容高布局 + 父容器 max_h/overflow_y_scroll/track_scroll，caret 滚进视口）、IME composing 闸门（Send click 与 AX press 先判 is_composing）、can_send 空 trim 禁用、composer tab_stop(true)、per-session 草稿（composer_drafts + 无 session 独立槽 + reset_text 恢复）、Terminal 解耦（terminal-input，28–220 独立预算）。W-B2：U2 九场景脚本 `scripts/ui-r5-wave-b-states.sh` + `scripts/test_ui_r5_wave_b_states.py`（18 用例）。零 wire 变更（crates/ 无 diff）。

## 代码门禁（2026-08-29 实跑）

- `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`：129 passed / 0 failed。
- `cd scripts && python -m unittest test_ui_r4_wave_b_states test_ui_r5_wave_b_states`：40 tests OK（venv python）。
- `cargo check` warnings = 15，与 Wave B 前基线持平。

## 评审与处置（两轮 grok_reviewer）

第一轮：P0×2 + P1×2 + P2×1，修复项 F1–F5：

- F1（P0）：AX composer-input value 恒为 text()，不再回退 placeholder（保 R4 composer-cleared 契约）；r5-1 断言同步。
- F2（P0）：MessageSent 回执仅 active session 清可见 Composer；发送方草稿条目恒清。闸门表达式收敛为生产调用的 `message_sent_clears_visible_composer`。
- F3（P1）：overflow scroll 死症修复——TextElement 不再自我 clamp（全内容高布局），视口由父容器兑现；`scroll_caret_into_view` 视口改取 `scroll.bounds()` 容器高（误用完整内容高会让「caret 超视口」永不成立）；offset 变更后 cx.notify() 补帧。死代码 clamp_input_height/clamp_composer_height 删除。
- F4（P1）：R4 U2 检查器适配单槽 AX 契约（running=cancel 单节点 enabled=1 且 send 缺席；idle/terminal=send 单节点）；r5 脚本 probe 超时干净退出。
- F5（P2）：测试去水分——IME 走真实 EntityInputHandler 路径（中间态不入栈、commit 单次入栈、undo 直达 commit 前）、copy/cut 真实剪贴板、shift 选择驱动真实 action、删 include_str! tautology（键位改由 u1_probe keystroke→keymap→action 链路覆盖）、desktop.md 回写。

第二轮：P1×1 + P2×1：

- P1（鼠标映射坐标）：评审指出 GPUI 0.2.2 滚动容器子元素 bounds 含 scroll offset，`index_for_mouse_position` 再减 offset 构成双重计入。主代理实测（真窗口 prepaint 逐帧打点）：element_offset 在常规 prepaint 恒 0、仅事件驱动帧含 offset——last_bounds 平移状态随帧时序不定，稳态下评审成立、过渡态下原代码成立。**根因修复**：prepaint 归一化 `content_bounds`（origin − element_offset），鼠标与 IME `character_index_for_point` 统一为「归一化原点 − scroll offset」语义，任何帧态一致；另发现 IME 行高误取 window.line_height()（paint 外默认字阶 26px ≠ paint 时 21px），改取 last_line_height（与鼠标路径同一回退）。
- P2（测试带宽不足）：点击测试从 55..=78 带宽收紧为精确行号断言 + IME/鼠标一致性断言。

## U2 九场景状态

2026-08-29 解锁后实跑九场景全部通过：22 份结构断言全 PASS、21 张阶段截图、`run-manifest.json` 与 action trace 齐全，证据见 [u2/](u2/)。覆盖空输入、发送→running→取消、多行 timeline 回显、断线草稿、task 草稿隔离、8 KiB CJK paste、running Cancel 槽、1080×720、Return / Shift-Return / Cmd-. 与 model 菜单选择。

实跑同时修正六处 driver 假设：r5-2 以 `fixture:hang` 首行进入可取消态；取消后 Composer 已按 MessageSent 语义清空（Send 恢复为 disabled）；r5-5 先清 session A 已恢复草稿再 paste；macOS bash 3.2 用 `while read` 替代 `mapfile`；键盘阶段临时钉到 ASCII 输入源并在 fixture 退出后恢复原输入源；r5-7 的跨 provider model 选择移到所有 fixture 发送场景之后，避免污染 r5-9。首次 r5-2 失败证据保留于 [u2-failed-1/](u2-failed-1/)，锁屏非回归证据保留于 [u2-locked-1/](u2-locked-1/)。

## 移交

- composer 区域 SSIM ≥0.99：同 R3/R4/Wave A 先例移交 R8，待用户拍板。
- `bounds_for_range`（IME marked-text 弹层定位）在滚动态下使用注册时 bounds，存在同类一帧滞后；当前测试未覆盖，列入 R6/R8 观察项。
