# P0-1：仓库与 workspace 骨架

> Phase 0 · 架构与协议冻结 · 状态：🟢已完成 · 依赖：—

**最终目的**：建立「仓库根 = Cargo workspace 根」的物理骨架，使后续所有 crate 有落点，`cargo metadata` 与空构建即可通过。这是整条关键路径的第一个动作，没有它任何 crate 都无处安放。

**涉及范围**：根 `Cargo.toml`、`crates/`、`apps/`、`schemas/`、`fixtures/`、`benches/`、`docs/`、`.gitignore`、CI 占位

## 细分步骤

1. **创建根 Cargo.toml** —— 声明 `[workspace]`、`members`、`resolver = "2"`、共享的 `[workspace.package]` 与 `[workspace.dependencies]`。目的：单一 workspace 根，统一版本与依赖来源。
2. **创建目录骨架** —— 按 workspace 结构建 `crates/ apps/ schemas/ fixtures/ benches/`。目的：为每个 crate 预留 home，命名一致。
3. **配置 .gitignore** —— `target/`、本地 SQLite/`*.db`、secrets、OS 元数据。目的：避免构建产物与本地数据库、密钥入库。
4. **CI 占位** —— GitHub Actions 跑 `cargo metadata` + `cargo build --workspace` + `clippy` + `fmt --check`。目的：尽早建立门禁，每个 PR 都可验证。
5. **构建可复现校验** —— 提交 `Cargo.lock`，确认空 workspace 能构建。目的：保证后续任务的基线可复现。

## 主要产出物

- 根 `Cargo.toml`（workspace 清单）
- 目录结构（crates/apps/schemas/fixtures/benches）
- `.gitignore`、CI yaml

## 验收标准

- [x] `cargo metadata` 成功
- [x] `cargo build --workspace`（空）通过
- [ ] CI 在三平台（或至少 ubuntu）绿（yaml 已就位，待首次 push 触发验证）

**验证状态**：本地 `metadata`、`fmt`、`clippy -D warnings`、全 targets 构建与 workspace 测试已通过；三平台 GitHub Actions 仍需在首次 push 后验证，未把远程运行结果误记为已通过。

**相关文档**：[workspace 结构](../docs/architecture/workspace-layout.md) · [总体架构](../docs/architecture/overview.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：`[workspace.dependencies]` 一次性纳入 [ROADMAP「依赖选型基线」](../ROADMAP.md#依赖选型基线) 中「直接采用」的包清单，统一版本基线。
