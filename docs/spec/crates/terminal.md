# pawork-terminal

> 终端显示核心纯库：行缓冲解析（CR 覆盖 / 退格 / EL / SGR 16 色）、按键到 PTY 字节映射、面板像素到列行估算。零依赖，不依赖 gpui / tokio / OS API 与任何 pawork-* 包；当前唯一消费者是 `pawork-desktop`（Inspector Terminal 显示层）。

## 1. 职责与边界

- **做什么**：把 Host 保存的原始 PTY 输出（已按 UTF-8 解码）整理成可见行缓冲——CR 覆盖同一行、退格回移光标、EL 擦行、SGR 进入 16 色属性分段；把平台无关按键事件映射成 PTY 字节；按面板像素估算列行并钳制在 PTY 边界。
- **显示边界**：支持 shell 行编辑所需的光标定位、上下左右移动、清屏和自动折行；尚不是完整 VT emulator，不支持 alt-screen / vim。不接触 PTY、协议、连接与 Host；不改原始 output；颜色 / 字体由调用方按主题 token 解析。
- **归属**：GUI4 起显示层在 `apps/desktop/src/ui/terminal_view.rs`；2026-09-15 经用户确认抽为本包，desktop 侧只留 gpui 胶水（主题色 runs、Keystroke 转换、resize 防抖）。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~770 | 全部实现与单元测试：`Screen` 行缓冲状态机、`Attrs` / `Cell` / `Line`、`render_lines` / `plain_output`、`KeyEvent`（含 `KeyEvent::new` 构造器与 Home/End/Esc/Backspace/Shift+Tab（ESC[Z）具体映射）/ `Modifiers` 与 `key_to_pty_bytes`、`size_from_bounds(_scaled)` 与几何常量 |

## 3. 对外 API 面

- **解析**：`Screen::new()` / `Screen::with_size(columns, rows)` / `feed(&str)` / `feed_with_width(&str, width)`（渲染方提供 0/1/2 列宽）；`cursor()` 返回行列，`cursor_byte_offset()` 返回可见文本中的字节偏移，`display_lines()` 保留当前光标行及尾随空格；`cursor_visible` / `bracketed_paste` 跟踪对应私有模式；`render_lines(raw) -> Vec<StyledLine>`（`StyledLine { text, spans }`，`Span { len, attrs }` 按 UTF-8 字节长度分段；空行以单空格占位保行高）；`plain_output(raw) -> String`（AX / 纯文本路径）。
- **属性**：`Attrs { fg, bg, bold, dim, inverse }`，fg / bg 为 0-15 颜色索引（256 色收敛到 16 色、truecolor 忽略），色值映射在调用方。
- **按键**：`KeyEvent { key, key_char, modifiers }` / `Modifiers { control, alt, shift, platform, function }`（平台无关，由调用方从自己的按键类型转换）；`key_to_pty_bytes(&KeyEvent) -> Option<String>`（Enter 输出 CR，Ctrl 字母映射控制字节；方向键 / Tab / Delete 输出终端转义序列）。
- **几何**：`TERMINAL_LINE_HEIGHT`（16px）/ `TERMINAL_CELL_WIDTH`（7.2px）/ `TERMINAL_OUTPUT_PAD_X/Y`（与 Inspector 输出面 px_2 / py_1 同源）；`size_from_bounds(width, height)` / `size_from_bounds_scaled(width, height, rem_scale) -> Option<(u16, u16)>`（钳 20-500 列 × 6-200 行，内容过小返回 None）。

## 4. 核心行为与数据流

1. Host 经 GUI Connection Protocol 流式推送 PTY output；Desktop 投影只累积原始字节（上限 256KiB 裁剪），渲染时由本包对全量缓冲一次性 `feed`。
2. `feed` 逐字符状态机：普通字符按 (row, col) 写格；CR 回列 0；LF 下一行；BS 只回移光标、不擦字符，DEL 忽略；TAB 到下一 8 列；CSI 处理 SGR / EL / ED / CUP(H/f) / CUU(A) / CUD(B) / CHA(G) / CUF(C) / CUB(D)，OSC / DCS / SOS / PM / APC 字符串整体跳过；带中间字节的 ESC（如 `ESC ( B`）整体跳过，畸形 / 私有 CSI 不当作擦行；稀疏水平寻址最多填到 500 列，已有长行仍可回移；支持 `?25` 光标显示与 `?2004` bracketed paste 模式；其余控制序列丢弃。
3. `display_lines` 保留光标所在行和光标格，历史尾部空行可裁剪；纯文本 `render_lines` / `plain_output` 仍裁尾随空白。Desktop 把 span 映射成主题色 `TextRun` 并在实际光标位置渲染光标及 IME 预编辑。
4. 按键路径反向：gpui `Keystroke` 经 desktop 胶水转 `KeyEvent`，映射成 PTY 字节后走 `terminal_write`；普通字符和 IME 仅通过原生输入法提交一次，终端没有命令草稿输入框。

## 5. 契约与不变量

- **光标定位**：寻址限制在当前视口；向下溢出时保留滚动历史；自动折行按当前 PTY 列数计算，不修改 Host 原始字节。
- **SGR 不泄漏进纯文本**：`plain_output` 只含可见字符（AX 与着色渲染共用同一解析结果）。
- **尺寸估算诚实**：内容区小于 40×24 像素返回 None，不臆造尺寸；边界为 20–500 列、6–200 行，界面不提供手动调节按钮。
- **键映射保守**：platform / function 修饰键一律 None（留给 App 快捷键）；alt 组合不映射；可打印字符只在无修饰时经 `key_char` 直通。

## 6. 依赖关系

- **生产依赖**：无（零依赖纯库）。
- **features**：`default = []`，无具名 feature。
- **被依赖**：仅 `pawork-desktop`（生产依赖白名单 `{pawork-client, pawork-terminal}`，由 desktop `platform.rs` deny-list 测试断言）。

## 7. 测试与验证资产

| 资产 | 覆盖点 |
| --- | --- |
| `src/lib.rs` 内 9 个单元测试 | CR 覆盖、退格回移光标、bracketed paste 等控制序列剥离、SGR 不泄漏纯文本、SGR 属性分段（粗体 / 16 色索引）、EL 擦行、字符集与私有序列跳过、稀疏游标内存边界、shell 光标移动、折行、清屏与尾随提示符空格、尺寸钳制与 rem 缩放、Ctrl-C / 方向键 / 可打印字符 / platform 修饰键映射 |

默认验证命令：`cargo test -p pawork-terminal --offline --lib --tests`。

## 8. 注意事项与已知限制

- 显示层快照语义：渲染对整段缓冲重新解析，解析结果不持久化；原始 output 的权威副本在 Host。
- 空行以单空格占位（保持行高与纯文本行数），消费方按整行渲染。
- Desktop 按当前等宽字体测量非 ASCII 字符的 0/1/2 列宽，修复 CJK 编辑回移；复杂 emoji / 组合字素簇尚非完整终端仿真。原始缓冲按当前窗口尺寸重建，历史窗口宽度不记录。
- 256 色 `38;5;n` 收敛到 16 色索引（n>15 截断），truecolor `38;2;…` 忽略；不扩展为完整调色板，除非 Terminal 升级为完整 VT emulator（限制见 [desktop.md](desktop.md) Inspector Terminal）。
- 相关文档：[architecture](../../architecture.md) §2 · [desktop.md](desktop.md)（Inspector Terminal 面板与 gpui 胶水层）· [产品篇 desktop](../desktop.md) · [ROADMAP GUI4](../../ROADMAP.md)。
