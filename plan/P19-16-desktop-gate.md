# P19-16：Desktop Contract / E2E / Visual / Performance Gate

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-1～P19-15

**最终目的**：集中证明 Desktop Client 在协议一致性、主工作流、三平台 WebView、accessibility、visual、性能、安全和发布供应链上可交付，并生成可复核的 Phase 19 退出证据。

**涉及范围**：Desktop test harness/fixtures、Mock GUI Server、WebdriverIO Tauri、visual/a11y/performance/security gates、release evidence

## 细分步骤

1. **Projection contract gate** —— Snapshot/Event/command fixtures 覆盖 duplicate/gap/out-of-order/replay/resnapshot/revision/reauth/unsupported version。目的：客户端状态可证明。
2. **主流程 E2E** —— connect → workspace/session → composer/run/stream/tool approval → diff/rollback → terminal；并覆盖 restart/reconnect。目的：Coding Agent GUI 闭环。
3. **扩展流程 E2E** —— account/auth/quota、resources/MCP/plugin、plan/background/automation、multi-agent 按 capability 分组。目的：页面与真实契约接线。
4. **三平台原生壳** —— Windows WebView2、macOS WKWebView、Linux WebKitGTK 执行 WebdriverIO Tauri；browser Mock 只作快速 L1。目的：OS 差异可见。
5. **Visual/accessibility gate** —— 稳定主题/viewport/font baseline、键盘全路径、focus、ARIA、contrast、zoom/reduced-motion。目的：交互质量可审查。
6. **Performance gate** —— cold start、snapshot-to-interactive、event-to-paint、10k Timeline、100k Diff、stream/terminal storm、memory/DOM budget。目的：大仓库可用。
7. **Security/supply-chain gate** —— crate boundary、capability/CSP、XSS/link/path、Secret storage/log/screenshot、deep link、sign/update tamper。目的：Desktop 不扩权。
8. **退出与回滚证据** —— 汇总版本、平台、产物 hash、测试/visual/perf/security 结果、已知限制与回退步骤；通过后记 `MaintenanceGated`。目的：可发布、可撤回。

## 主要产出物

- Desktop contract fixtures 与 Mock GUI Server scenarios
- 三平台 E2E/visual/a11y/performance/security workflows
- Phase 19 gate report、signed artifact manifest 与 rollback evidence

## 验收标准

- [ ] Projection/command contract 对所有 sequence/revision/reconnect 边界通过
- [ ] P0 主流程在 Windows/macOS/Linux 原生壳通过，browser mode 不作为替代证据
- [ ] visual 变更经人工确认，键盘/读屏/对比/缩放/reduced-motion gate 通过
- [ ] [Desktop 性能目标](../docs/quality/performance-targets.md#desktop-gui-指标phase-19) 达标或有批准的分平台例外
- [ ] [安全验收](../docs/quality/security-acceptance.md) Desktop 项与签名更新篡改测试通过
- [ ] 发布证据记录 protocol/host/app version、产物 hash、已知限制与回退演练

**相关文档**：[Desktop GUI](../docs/features/desktop-gui.md) · [测试体系](../docs/quality/testing.md) · [性能目标](../docs/quality/performance-targets.md) · [安全验收](../docs/quality/security-acceptance.md)

**依赖建议（2026-08）**：renderer L1 用 Vitest/Testing Library；Desktop E2E 用 WebdriverIO + `@wdio/tauri-service`，参考 [Tauri WebDriver](https://v2.tauri.app/develop/tests/webdriver/)。
