# P18-1：Control Plane 契约与迁移基线

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P1-13、P0-4、P0-8、ADR-033

**最终目的**：冻结 Provider、Account、Session、Agent 与 Client Protocol 的职责边界、versioned schema 和回滚策略，使后续账号池与客户端适配任务可以独立合并而不修改现有 `ModelProvider` contract。

**涉及范围**：`agent-domain` / `provider-api` / `core-api` 的纯类型草案；`app-database` migration 设计；`docs/architecture` / `docs/features`

## 细分步骤

1. **现状勘察与兼容矩阵** —— 列出已有 trait/type/test/schema 与 legacy credential/session 形态；目的：避免把文档设计误判为已实现。
2. **五类状态机冻结** —— 分别定义 client session、Agent、route、account/credential lease、tenant policy 的 ownership 与事件边界；目的：禁止万能 Router。
3. **纯类型落位与依赖审计** —— 通用 opaque ID/value object 留在 `agent-domain`，携带 Provider contract 的 Pool/Routing/Error trait 留在 `provider-control` / `provider-api`，Client frame/capability 类型留在 `client-adapter-api`；用依赖测试确认现有 `provider-api → agent-domain` 不被反转。目的：避免 crate 循环依赖和 Provider 概念污染 domain。
4. **版本与 feature flag** —— 定义 schema/event version、`account-control-v1` 与各 client adapter 开关；目的：支持灰度与快速回退。
5. **Migration / rollback 草案** —— legacy 用户映射 `local/default`、单 credential 映射 synthetic account，优先 side table 或 nullable/versioned column；目的：保持旧数据库可用。
6. **契约测试骨架** —— 为后续 property/concurrency/golden/migration/isolation 测试预留 fixture 目录和命名；目的：让每项计划有统一证据入口。

## 主要产出物

- versioned control-plane 类型与状态机说明
- schema migration / rollback 设计和 feature flag 清单
- 后续 P18 任务的 contract test 目录约定

## 验收标准

- [x] `ModelProvider` / `EmbeddingProvider` contract 未扩张账号、租户或客户端职责
- [x] 五类状态机和所有权边界均有明确输入、输出与失败域
- [x] `agent-domain` 不依赖 `provider-api` / Client / IO crate，依赖方向测试无新增环
- [x] legacy migration 与 feature-off runtime fallback 可独立执行
- [x] 所有新增持久化实体和 canonical event 明确 version 字段

## 验证记录（2026-08-12）

- Validation Level: L1
- Affected crates: `agent-domain`、`provider-control`、`core-api`、`app-database`；关键直接消费者 `orchestration`
- Validated: `cargo fmt -p agent-domain -p provider-control -p core-api -p app-database -- --check`；`cargo test -p provider-control`；`cargo test -p provider-control --no-default-features`；`cargo test -p app-database -p core-api -p agent-domain`；`cargo check -p orchestration`；`cargo run -p schema-typegen -- --check`；`git diff --check`
- Targeted regressions: `account-control-v1` feature-off fallback、`local/default` synthetic account、versioned serde fail-closed、migration 幂等/备份回滚/整批原子性、credential 表无 secret 列、关键消费者编译
- Full workspace gate: NOT RUN（未命中升级条件）
- P18-15 follow-up: 对 `core-api` / `provider-control` / `app-database` 的 control-plane schema version 常量增加跨 crate 一致性断言。

**相关文档**：[ADR-033](../docs/adr/ADR-033-control-plane-separation.md) · [Provider Control Plane](../docs/features/provider-control-plane.md) · [ROADMAP](../ROADMAP.md)
