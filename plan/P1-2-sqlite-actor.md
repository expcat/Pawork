# P1-2：SQLite Actor

> Phase 1 · 基础设施 · 状态：🟢已完成 · 依赖：P0-3

**最终目的**：建立专用 SQLite Actor，所有 DB 访问经单一 Actor 串行化，避免 Tokio Task 并发写导致锁竞争与损坏，为 Event Store 提供可靠底层。

**涉及范围**：`app-database`

## 细分步骤

1. **选定 SQLite 绑定与连接策略** —— 目的：确定存储底层。
2. **实现 DB Actor（mpsc 命令通道）** —— 目的：串行化所有 DB 操作。
3. **启用 WAL / foreign keys / busy timeout** —— 目的：并发读 + 一致性。
4. **备份与只读恢复接口** —— 目的：崩溃后可恢复。

## 主要产出物

- `app-database` crate + DB Actor

## 验收标准

- [x] 不在任意 Tokio Task 直接并发操作 DB
- [x] WAL 启用

**相关文档**：[ADR-003 Event Store](../docs/adr/ADR-003-sqlite-event-store.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：选 rusqlite 而非 sqlx——sqlx 的异步池 + 编译期 SQL 检查与「单连接串行 Actor」设计不匹配，集成成本高于收益；rusqlite 采用率高，配合专用线程 / spawn_blocking 契合本设计。
