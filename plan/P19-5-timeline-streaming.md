# P19-5：Timeline / Streaming 渲染

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-2～P19-4、P3-1～P3-11、P5-8、P13-8、P15-5、P15-7

**最终目的**：把可重放的 Agent/Tool/Provider 事件呈现为稳定、高性能、可追溯的 Timeline，在流式增量、历史前插、大型 Artifact 与用户滚动之间保持正确锚点。

**涉及范围**：Timeline projection/selectors、content block renderers、virtualized viewport、Artifact viewer、message actions

## 细分步骤

1. **内容块渲染** —— user/assistant/system、text/image、thinking summary、tool/server-tool、citation/source、error/cancel/usage。目的：覆盖 canonical event 而非 Provider 特例。
2. **增量合并** —— 按 message/item ID 合并 delta，完成事件封口；重复/迟到 delta 遵循 reducer contract。目的：流式不闪烁不重复。
3. **虚拟滚动** —— keyed dynamic-height list、end anchoring、加载更早历史保持视口、用户离尾部时不抢滚动。目的：长 Session 可用。
4. **大型内容** —— Tool/Server Tool/Terminal capture 只显示摘要和 Artifact 分块读取，支持取消加载。目的：renderer 内存有界。
5. **安全 Markdown/链接** —— raw HTML 禁用，code/text 不执行；URL/图片/Artifact scheme allowlist。目的：不可信模型输出不扩权。
6. **消息动作** —— copy、retry、fork、compact、inspect raw-safe metadata，全部通过 Query/AppCommand。目的：主工作流闭环。
7. **流式/a11y/perf tests** —— scripted 30 token/s、tool output storm、10k items、读屏节流。目的：验证真实负载。

## 主要产出物

- Canonical Timeline renderers 与 virtualized viewport
- Artifact/citation/source viewer、message actions
- Streaming/scroll/security/accessibility/performance fixtures

## 验收标准

- [ ] Timeline 不按 Provider 名称分支，未知版本内容有安全 fallback
- [ ] 历史前插、动态高度与流式追加不破坏阅读锚点
- [ ] 10,000 条 Timeline 与持续流达到 Desktop 性能目标，DOM 节点有界
- [ ] raw HTML/危险 URL/任意本地路径无法执行或加载
- [ ] Artifact 读取分块、可取消，Protected Blob 只显示安全摘要/引用
- [ ] copy/retry/fork/compact 由 Core 命令/事件确认

**相关文档**：[agent-engine](../docs/features/agent-engine.md) · [artifacts](../docs/features/artifacts.md) · [Desktop GUI](../docs/features/desktop-gui.md) · [性能目标](../docs/quality/performance-targets.md)

**依赖建议（2026-08）**：长列表采用 `@tanstack/react-virtual`；Markdown 采用 `react-markdown` + `remark-gfm`，不启用 raw HTML 插件。
