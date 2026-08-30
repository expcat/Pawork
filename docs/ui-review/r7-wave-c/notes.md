# R7 Wave C — 响应式、长内容与平台偏好

> 状态：🔵 进行中（2026-08-30 已完成真实窗口耐久子集；字号放大与主动平台偏好态仍未完成）

## 目标与边界

- 目标：在真实 Host + Desktop 窗口中验证 1080×720、CJK/emoji/超长内容、千级 Timeline、反复 resize、断线重连与单次性能基线，只修复证据能证明的当前缺口。
- 非目标：不改 GUI wire、Host、Policy、正式 `fixtures/ui/seed.json` 业务数据或 1440×1024 视觉 reference；不以一次机器采样冻结性能阈值；不把默认平台偏好快照冒充高对比 / reduced motion 主动态通过。
- 完成口径：当前子集必须有可重放 driver、AX/几何断言、真实窗口截图与 manifest；Wave C 只有在字号放大和主动平台偏好态也完成后才能关闭。

## 已实现

1. 新增 [`ui-r7-wave-c-resilience.sh`](../../../scripts/ui-r7-wave-c-resilience.sh) 真窗口 driver：串行覆盖 1440×1024、1080×720、ActivityPopover 边界、三轮宽窄 resize、Composer 焦点保持、断线与重连。
2. 只在隔离临时数据库中从正式 seed 的 64 条可投影行派生 960 条消息，得到 1024 条 Timeline；正式 fixture 不变。CJK/emoji/超长内容与末尾哨兵 `R7C 千级列表末尾 🐾🧪` 经真实 Host / 协议 / projection 进入 Desktop。
3. 扩展真滚轮注入支持纵向滚动，验证千级列表虚拟化、离底与回底；同时记录启动、加载、滚动、输入、resize 与 screenshot 的单次机器基线。
4. 读取 macOS Accessibility Display 偏好并归档，不修改用户设置。当前样本四项均为 `false`，只代表默认态环境。
5. 首轮断线窄窗截图发现长断线原因像素越过 `connection-status` 与 `add-task` 之间的合同间隙。第一次修复只加 `min_w_0 + overflow_hidden + truncate`，解锁后复跑仍 FAIL：该 flex 子项没有定宽，实际被外层硬裁剪且占满「+」前剩余空间。根因修复把连接槽定为「rail 内容宽 − 28px 全局「+」 − 8px 间隔」，8px 间隔升为 render / AX 共享 `RAIL_CONNECTION_ADD_GAP`，文案在定宽槽内显示省略号，AX 仍提供完整状态值。

## 验证与真实证据

- [`run-manifest.json`](u2-reviewfix-pass-20260830/run-manifest.json)：修复后最终 U2 的 15 个 manifest 相位全部 `structural_pass=true`（14 个结构 / 虚拟化运行相位 + 断连截图 paint 相位）。
- 修复后 1080×720 截图：[`Connected`](u2-reviewfix-pass-20260830/connected-1080x720.png) · [`ActivityPopover`](u2-reviewfix-pass-20260830/activity-popover-1080x720.png) · [`Disconnected`](u2-reviewfix-pass-20260830/disconnected-1080x720.png)；另有 [`1440×1024`](u2-reviewfix-pass-20260830/wide-1440x1024.png)。人工复核未见主操作遮挡或 Popover 越界；断连长文案显示为 `Disconnected · tra…`，paint 门禁 `lit=0`。
- [`dataset.json`](u2-reviewfix-pass-20260830/dataset.json)：64 + 960 = 1024 条逻辑行，作用域为 `temporary_fixture_database_only`；AX 可见切片小于逻辑总数，末尾哨兵在回底后可见。
- [`performance-baseline.json`](u2-reviewfix-pass-20260830/performance-baseline.json)：Desktop ready 7058ms、1024 行加载 2107ms、离底 1051ms、回底 252ms、输入 237ms；窄窗 resize 249–284ms（中位 267.5ms），宽窗 265–317ms（中位 288ms），截图 99–131ms（中位 102ms）。分类为 `baseline_only`，阈值仍为 `null`。
- [`platform-preferences.json`](u2-reviewfix-pass-20260830/platform-preferences.json)：macOS 26.6.2 / `zh_CN`，reduce motion、increase contrast、reduce transparency、differentiate without color 均未开启。
- 自动门禁：Desktop **144/144**；Wave C Python **4/4**；既有 Wave B Python **17/17 + 22/22**；shell、Python 与 Swift 编译 / 语法检查通过。

## 审查补强与复跑记录

独立审查发现四项问题并已修复：

1. **P1 视觉溢出为真，且第一次修复不充分**：旧截图 `connection-status` 与 `add-task` 间隙内有 12 个亮像素，仅凭 AX「within rail」无法发现绘制溢出。新增截图级 `paint-assert` 门禁后，首轮负样本与第一次修复后的解锁复跑均判 FAIL（`lit=12`），后者保留于 [`u2-reviewfix-paint-fail-20260830`](u2-reviewfix-paint-fail-20260830/)；根因修复后的最终样本 [`assert-r7c-disconnected-rail-paint.json`](u2-reviewfix-pass-20260830/assert-r7c-disconnected-rail-paint.json) 判 PASS（`lit=0`）。
2. **P1 重连态未门禁**：新增 `r7c-narrow-reconnected` 相位，要求 `reconnect` 缺席、`connection-status` 以 `Local · Connected` 开头且 Composer 焦点保持。
3. **P1 千行断言只回显 CLI**：`states-assert` 现在必须读取真实 `timeline_stable` barrier（`entry_count >= 1000` 且 `session_id == fx-ses-beta-long`），缺失 / 损坏 barrier 即失败；坏 barrier 失败路径已有测试。
4. **测试命名过度声明**：inflate 测试改为「拒绝被修改的基线行」，不再宣称覆盖中途插入回滚。

另用首轮真窗口 `geometry` + AX tree 回放新几何合同：[`assert-r7c-disconnected-rail-reviewfix.json`](u2-resilience-20260830/assert-r7c-disconnected-rail-reviewfix.json) 全绿（connection-status 宽 164、reconnect 宽 200，均在 240px rail 内）。

**锁屏阻断与最终复跑**：审查后四次完整 driver 复跑曾在任何目标动作之前的 Desktop AX 注册阶段超时。第四次在 `caffeinate -u` 保持显示器唤醒下进行——全屏截图证实当时为 macOS 锁屏界面（含「本人確認」登录提示），窗口在 CGWindowList 中 onscreen 但 AX 树只有递归 `AXApplication`，属会话锁定的环境阻塞，不构成产品失败或通过。代表性样本保留于 [`u2-reviewfix-ax-blocked-locked-20260830`](u2-reviewfix-ax-blocked-locked-20260830/)；其余三次重复空样本与 11 次手动取证尝试（裁剪图实为壁纸，无效）已清理。会话解锁后先复跑得到第一次修复的 paint FAIL 样本，再完成根因修复与最终全绿复跑。

## 未完成与下一步

- 当前字体 token 与组件使用固定 px 字号，没有已批准的应用内字号放大机制；本轮没有通过改变系统设置或缩放截图伪造支持。
- 高对比、reduced motion 等主动态未切换；当前应用仍是固定深色主题，也没有订阅这些平台偏好。需要先确定产品支持边界，并由用户授权 / 人工切换真实系统态后复跑同一 driver。
- VoiceOver 仍未执行，屏幕朗读措辞 / 顺序未验证；一次性能样本不能证明无回退或冻结阈值。
- 因以上缺口，Wave C 保持进行中，ROADMAP 不前移至 R8。

Validated: `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`（144/144）；`/tmp/pawork-wave-d-venv/bin/python scripts/test_ui_r7_wave_c_resilience.py`（4/4）；`python3 scripts/test_ui_r3_wave_b_tools.py`、`/tmp/pawork-wave-d-venv/bin/python scripts/test_ui_r4_wave_b_states.py`（17/17 + 22/22）；`PAWORK_WAVE_D_PYTHON=/tmp/pawork-wave-d-venv/bin/python scripts/ui-r7-wave-c-resilience.sh run --out docs/ui-review/r7-wave-c/u2-reviewfix-pass-20260830 --label r7-wave-c-reviewfix-final`（15 相位 / paint 门禁全绿）；shell / Python / Swift 定向语法与编译检查。

Targeted regressions: 1080×720 connected / popover / disconnected、三轮宽窄 resize 与焦点、1024 行虚拟化 / 纵向滚动 / CJK-emoji 哨兵（含 barrier 硬化与坏 barrier 失败路径）、连接长文案截图级 paint 门禁（旧截图负证 + 合成图回归）、重连相位与单次性能基线。

Full workspace gate: NOT RUN（当前未设置全量门禁）
