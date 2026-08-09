# P8-8：Hot Reload

> Phase 8 · Skills、Prompts 与 Instructions · 状态：🟢已完成 · 依赖：P8-3

**最终目的**：实现资源文件变更热重载（去抖），让开发期修改 Skills/资源即时生效而不频繁重建。

**涉及范围**：`resource-loader`

## 细分步骤

1. **文件监听** —— 目的：感知变更。
2. **去抖重载** —— 目的：避免抖动。
3. **失效与重建** —— 目的：上下文一致。
4. **测试** —— 目的：重载正确。

## 主要产出物

- Hot Reload

## 验收标准

- [x] 文件变更后去抖重载生效

**实现**：`ResourceHotReload/ResourceWatcher` 以 notify debouncer 监听、锁外重建、Arc 原子换代；失败保留旧快照，drop 停止 watcher，真实文件事件测试覆盖重载路径。

**相关文档**：[skills](../docs/features/skills.md) · [ROADMAP](../ROADMAP.md)
