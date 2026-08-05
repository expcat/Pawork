# P4-5：run_command

> Phase 4 · 核心工具与权限 · 状态：🟡未开始 · 依赖：P4-11、P4-12

**最终目的**：实现非 PTY 的 run_command（流式 stdout/stderr、cwd、timeout、env 白名单、cancel、exit code），让 Agent 能运行测试与构建命令并安全终止。

**涉及范围**：`builtin-tools`、`process-runtime`

## 细分步骤

1. **非 PTY 执行 + 流式输出** —— 目的：实时反馈。
2. **cwd / env 白名单 / timeout** —— 目的：可控环境。
3. **cancel + 进程树终止** —— 目的：可安全停止。
4. **exit code 与退出归一** —— 目的：结果可判定。

## 主要产出物

- run_command 工具

## 验收标准

- [ ] 流式 stdout/stderr 正确
- [ ] cancel 清理进程树

**相关文档**：[tools](../docs/features/tools.md) · [process](../docs/features/process.md) · [sandbox](../docs/features/sandbox.md) · [ROADMAP](../ROADMAP.md)
