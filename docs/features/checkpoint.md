# Checkpoint 与回滚

## 职责

每次可能修改文件的 Agent Run 创建逻辑 Checkpoint，支持单次 Tool Call 与整个 Run 的回滚。

## Checkpoint 内容

```text
Run ID
开始时 HEAD
Git Index fingerprint
修改文件列表
修改前内容 Blob
新增文件
删除文件
文件权限
时间戳
```

## 功能

回滚单个 Tool Call；回滚整个 Run；查看修改；恢复删除文件；恢复权限；防止覆盖用户在 Run 后的修改；检测冲突；导出 Patch；将 Checkpoint 固化为 Git Commit。

**不能默认自动执行 `git reset --hard`。**

## 验收标准

- 所有文件写操作建立 Checkpoint
- 可回滚单次 Tool Call 与整个 Run
- 能检测用户在 Run 后的修改并避免覆盖
- Checkpoint 可固化为 Commit

## 相关文档

- [git-diff](git-diff.md) · [tools](tools.md) · [artifacts（Blob）](artifacts.md) · [sessions](sessions.md)
- [ADR-010 写操作 Checkpoint](../adr/ADR-010-checkpoint-all-writes.md)
- [ROADMAP P4-11](../../ROADMAP.md)
