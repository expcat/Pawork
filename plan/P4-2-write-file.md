# P4-2：write_file

> Phase 4 · 核心工具与权限 · 状态：🟡未开始 · 依赖：P4-9、P4-11

**最终目的**：实现原子 write_file（建父目录、保留权限/换行、覆盖审批、写入前 checkpoint），让 Agent 写文件安全且可回滚。

**涉及范围**：`builtin-tools`、`checkpoint-service`

## 细分步骤

1. **原子写（tmp + rename）** —— 目的：不产生半写文件。
2. **建父目录、保留权限/换行** —— 目的：尊重环境。
3. **覆盖审批** —— 目的：防误覆盖。
4. **写入前 checkpoint** —— 目的：可回滚。

## 主要产出物

- write_file 工具

## 验收标准

- [ ] 写入可经 checkpoint 回滚
- [ ] 覆盖需审批

**相关文档**：[tools](../docs/features/tools.md) · [checkpoint](../docs/features/checkpoint.md) · [policy](../docs/features/policy.md) · [ROADMAP](../ROADMAP.md)
