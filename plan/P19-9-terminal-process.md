# P19-9：Terminal / Process

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-1～P19-3、P11-6、P11-7、P13-5、P13-8

**最终目的**：在不授予 GUI 直接 spawn/shell 权限的前提下提供可重连 PTY，正确处理输入、resize、输出风暴、退出与进程树清理，并让 Terminal 与 Agent Tool output 的归属清晰可见。

**涉及范围**：Terminal panel、xterm renderer adapter、terminal projection、stream/backpressure/reconnect UI、safe link handling

## 细分步骤

1. **Terminal lifecycle** —— create/attach/detach/close、owner/session/run/cwd/sandbox 状态均来自 Core。目的：明确进程归属与隔离级别。
2. **xterm adapter** —— 映射 UTF-8 output、input、resize、selection、search、theme 与 accessibility。目的：稳定终端体验。
3. **有界流** —— bridge 批量传输、bounded buffer、slow renderer dropped/truncated 标记与 Artifact capture。目的：输出风暴不拖垮 GUI/Core。
4. **重连恢复** —— 使用 terminal sequence/cursor 或 snapshot/capture 恢复；无法补齐时明确截断。目的：断线不伪造完整历史。
5. **安全链接/粘贴** —— URL/file link 走 scheme/workspace allowlist，bracketed paste 与控制字符可见策略。目的：终端内容不扩权。
6. **结束与清理** —— exit code/signal/timeout/cancel/process-tree cleanup 事件可见；GUI close 不默认 kill。目的：生命周期符合 P11。
7. **压力/平台测试** —— CJK/emoji/ANSI、resize storm、10MB output、disconnect、Windows/macOS/Linux shell fixture。目的：跨平台验证。

## 主要产出物

- Terminal panel 与 xterm adapter
- PTY lifecycle/stream/reconnect/backpressure UI
- 安全链接、平台、压力与 accessibility tests

## 验收标准

- [ ] Desktop 无通用 shell/spawn capability，Terminal 只经 P13 command/event
- [ ] output storm 下 buffer/DOM 有界，截断和 Artifact capture 可见
- [ ] resize/input/IME/reconnect 不重复或错序，缺失历史明确标记
- [ ] GUI 关闭不杀任务；显式 close/cancel 触发 Core 的进程树清理语义
- [ ] 危险 URL、OSC/link 与任意文件路径不能绕过 allowlist

**相关文档**：[process](../docs/features/process.md) · [sandbox](../docs/features/sandbox.md) · [GUI 连接](../docs/features/gui-connection.md) · [Desktop GUI](../docs/features/desktop-gui.md)

**依赖建议（2026-08）**：采用 `@xterm/xterm` + `@xterm/addon-fit`；终端 session/reconnect/backpressure 语义由 Pawork 自实现。
