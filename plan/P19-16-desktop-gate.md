# P19-16：Desktop Contract / E2E / Visual / Performance Gate

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-1～P19-15

**最终目的**：在 P19-1 GPUI 准入 Gate 之后，集中证明完整 Desktop Client 在协议一致性、主工作流、三平台原生壳、accessibility、visual、性能、安全和发布供应链上可交付，并生成可复核的 Phase 19 退出证据。

**涉及范围**：纯 Rust Desktop test harness/fixtures、Mock GUI Server、GPUI component context、OS 原生启动/输入/截图 harness、visual/a11y/performance/security gates、release evidence

## 细分步骤

1. **Projection contract gate** —— Snapshot/Event/command fixtures 覆盖 duplicate/gap/out-of-order/replay/resnapshot/revision/reauth/unsupported version。目的：客户端状态可证明。
2. **主流程 E2E** —— connect → workspace/session → composer/run/stream/tool approval → diff/rollback → terminal；并覆盖 restart/reconnect。目的：Coding Agent GUI 闭环。
3. **扩展流程 E2E** —— account/auth/quota、resources/MCP/plugin、plan/background/automation、multi-agent 按 capability 分组。目的：页面与真实契约接线。
4. **三平台原生壳** —— 在 Windows/macOS/Linux 启动真实签名候选二进制，驱动连接、输入、窗口、clipboard/dialog/notification 与重连；GPUI component/headless test 只作快速 L1。目的：GPU、字体、IME 与 OS adapter 差异可见。
5. **Visual/accessibility gate** —— 稳定主题/窗口/font baseline、键盘全路径、focus、可访问树与状态通知、contrast、200% scaling、reduced-motion；用 Windows Narrator、macOS VoiceOver、Linux Orca 取得真实证据。目的：交互质量可审查。
6. **Performance gate** —— cold start、snapshot-to-interactive、event-to-visible、10k Timeline、100k Diff、stream/terminal storm、CPU/RSS/GPU、活跃渲染行与 frame budget。目的：大仓库可用且不以 DOM 指标替代原生数据。
7. **Security/supply-chain gate** —— crate/source dependency boundary、`DesktopPlatform` allowlist、Markdown/OSC/link/path/双向控制符、Secret state/log/screenshot、deep link、license inventory、sign/update tamper。目的：Desktop 不扩权且来源可审计。
8. **退出与回滚证据** —— 汇总版本、平台、产物 hash、测试/visual/perf/security 结果、已知限制与回退步骤；通过后记 `MaintenanceGated`。目的：可发布、可撤回。

## 主要产出物

- Desktop contract fixtures 与 Mock GUI Server scenarios
- 三平台 E2E/visual/a11y/performance/security workflows
- Phase 19 gate report、signed artifact manifest 与 rollback evidence

## 验收标准

- [ ] Projection/command contract 对所有 sequence/revision/reconnect 边界通过
- [ ] P0 主流程在 Windows/macOS/Linux GPUI 原生壳通过，component/headless mode 不作为替代证据
- [ ] visual 变更经人工确认，键盘/读屏/对比/缩放/reduced-motion gate 通过
- [ ] [Desktop 性能目标](../docs/quality/performance-targets.md#desktop-gui-指标phase-19) 达标或有批准的分平台例外
- [ ] [安全验收](../docs/quality/security-acceptance.md) Desktop 项与签名更新篡改测试通过
- [ ] 发布证据记录 protocol/host/app version、产物 hash、已知限制与回退演练
- [ ] 记录实际 GPUI revision、Rust toolchain、GPU/驱动/窗口系统与平台 adapter；不把 Zed 产品证据当作 Pawork 验证

**相关文档**：[Desktop GUI](../docs/features/desktop-gui.md) · [测试体系](../docs/quality/testing.md) · [性能目标](../docs/quality/performance-targets.md) · [安全验收](../docs/quality/security-acceptance.md)

**依赖建议（2026-08）**：L0/L1 使用 Rust unit/property test 与所 pin GPUI 提供的 test context；L2/L3 使用 OS 原生窗口/输入/截图与读屏 harness。自动化工具必须在 P19-1 验证后精确锁定；没有稳定 GPUI WebDriver 等价物时保留平台 adapter 与人工验收步骤，不用浏览器测试冒充真实壳。
