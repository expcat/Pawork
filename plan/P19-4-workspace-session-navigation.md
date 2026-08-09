# P19-4：Workspace / Session 导航

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-2、P19-3、P1-7、P5-1～P5-4、P13-8

**最终目的**：让用户从连接实例进入工作区并可靠定位 Session/Branch/Run，在大量历史会话、崩溃恢复和多 GUI 更新下保持导航上下文稳定。

**涉及范围**：Desktop connection/workspace/session routes、sidebar tree、session search/recent、navigation preference

## 细分步骤

1. **Instance/Workspace switcher** —— 展示 locality、identity、连接/健康状态，添加/打开/信任工作区走 AppCommand。目的：明确当前权威边界。
2. **Session/Branch tree** —— 分页/虚拟化展示 Session、Fork、active run、unread/error 状态。目的：承载长期历史而不一次加载。
3. **创建/打开/恢复** —— create/open/fork/rename/archive/compact 入口与 crash-recovery 提示。目的：覆盖主生命周期。
4. **搜索与过滤** —— query Core 的 session search，支持 workspace/model/status/date，不复制全文索引到 GUI。目的：大规模导航。
5. **路由与恢复** —— 保存无敏感信息的 last workspace/session/panel/scroll anchor；目标不存在时降级到安全页面。目的：重启后可继续。
6. **并发更新** —— Session 被 CLI/其他 GUI 修改、删除或换 branch 时按 Event 更新并显示 actor。目的：多客户端一致。

## 主要产出物

- Instance/Workspace switcher 与 Session/Branch sidebar
- Session lifecycle/search/recovery flows
- Navigation reducer 与大列表/多客户端 tests

## 验收标准

- [ ] 10,000 Session 使用分页/虚拟化，不把全文数据复制进 renderer
- [ ] create/open/fork/recovery/compact 等命令带 source/revision 并由 Event 确认
- [ ] 其他客户端变更能更新当前导航，目标失效时不显示陈旧可写页面
- [ ] 本地只持久化无敏感导航 preference
- [ ] 键盘树导航、搜索、焦点恢复和空/错/加载状态通过测试

**相关文档**：[sessions](../docs/features/sessions.md) · [GUI 连接](../docs/features/gui-connection.md) · [Desktop GUI](../docs/features/desktop-gui.md)
