# P8-1：Resource Loader

> Phase 8 · Skills、Prompts 与 Instructions · 状态：🟢已完成 · 依赖：P1-7、P1-8

**最终目的**：实现 Resource Loader（加载 `AGENTS.md` / Skills / Prompt / Profile），保证资源加载错误不崩溃，为确定性上下文提供来源。

**涉及范围**：`resource-loader`

## 细分步骤

1. **加载器抽象** —— 目的：统一加载各类资源。
2. **错误隔离** —— 目的：单个资源错误不影响整体。
3. **来源记录** —— 目的：可诊断生效来源。
4. **测试** —— 目的：错误不崩溃。

## 主要产出物

- `resource-loader` crate

## 验收标准

- [x] 加载错误不导致 Core 崩溃

**实现**：新增 `crates/resource-loader`，以 `ResourceBundle + ResourceIssue` 隔离单资源错误，并用 workspace-relative request 约束所有模型可影响的路径。

**相关文档**：[skills](../docs/features/skills.md) · [context](../docs/features/context.md) · [ROADMAP](../ROADMAP.md)
