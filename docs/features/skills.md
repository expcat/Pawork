# Skills、Prompts 与 Instructions

## 职责

加载并按确定优先级组合 Skills、Prompt 模板与项目指令（`AGENTS.md` 等），为上下文构建提供可诊断的来源。

## Skills 格式

```text
skill/
├── SKILL.md
├── manifest.toml
├── prompts/
├── scripts/
└── assets/
```

Manifest：

```toml
id = "rust-review"
version = "1.0.0"
description = "Review Rust code"
permissions = ["filesystem.read", "process.cargo"]
```

功能：全局 Skills；工作区 Skills；激活和禁用；参数；资源文件；脚本；权限声明；版本；依赖；冲突检测；热重载。

## Prompt Templates

支持：Markdown；参数；文件引用；默认模型；默认 Thinking；默认 Tools；默认预算；工作区覆盖。

## Instructions

支持：全局 Instructions；工作区 Instructions；路径层级 `AGENTS.md`；文件相关 Instructions；Agent Profile；单次运行 Instructions。必须显示最终生效来源，方便诊断。

## 验收标准

- 能显示所有有效指令来源
- 相同配置始终产生确定性上下文
- Resource 加载错误不导致 Core 崩溃

## 相关文档

- [context](context.md) · [workspace-index](workspace-index.md)
- [ROADMAP Phase 8](../../ROADMAP.md)
