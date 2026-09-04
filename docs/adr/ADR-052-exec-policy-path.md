# ADR-052：exec 复用 policy 路径 helper（删除 crate 内复制）

- **状态**：Accepted（用户于 2026-09-04 确认 CLN 内部收敛计划，含本决策；不采取向前兼容或双轨 shim）
- **日期**：2026-09-04

## 背景

ADR-039 D2 将 `pawork-exec` 列为不合并包，并在当时以「零内部 `pawork-*` 依赖」隔离进程/沙箱面。因此 [`crates/exec/src/path.rs`](../../crates/exec/src/path.rs) 复制了 `pawork-policy` 的 `canonicalize_platform` / `path_within_root` / `relative_to_root`（约 37 行）。与此同时 [architecture.md](../architecture.md) §3.3 禁止再长第四套路径判断；`workspace/resources/io.rs` 的 `canonical_within` 是已登记残余。CLN 内部收敛要求删除这些复制，而不是再包一层兼容转发。

`CancellationToken` 仍保持 exec 与 domain 双类型：那是取消原语隔离，不是路径安全内核。

## 决策

### D1 — 允许 `pawork-exec` → `pawork-policy`

- `crates/exec/Cargo.toml` 增加对 `pawork-policy` 的生产依赖。
- 删除 `crates/exec/src/path.rs`。调用点改为 `pawork_policy::{canonicalize_platform, path_within_root, relative_to_root}`。
- exec **仍然不直接依赖** `pawork-domain`；domain 仅经 policy 传递。
- `CancellationToken` 继续使用 exec 自有类型，不改为 domain 令牌。

### D2 — 不合并包

本 ADR 不推翻 ADR-039 D2 的不合并清单。policy 与 exec 仍是独立包；只修正「exec 零内部依赖」这一条过时隔离理由。

### D3 — 无兼容层

- 不保留 `exec::path` 模块、不 re-export 旧路径、不留 `#[deprecated]` 别名。
- 沙箱 within-root 语义必须与 policy 实现逐字节同一代码路径；不允许「看起来一样」的第二份。

### D4 — 闭包不膨胀

`cargo tree -p pawork` 生产闭包不得因本边明显膨胀（policy/domain 本已在 `pawork` 闭包内）。`cargo tree -p pawork-exec` 将新增 policy/domain，这是预期。

## 否决支

- **继续复制 37 行**：与 §3.3「禁止第四套」冲突，且会与 resources 收敛不同步。
- **exec 依赖 domain 直接使用其路径类型**：路径内核在 policy，不在 domain。
- **新建共享 crate**：违反「当前不新增包」。

## 后果与回滚

- 正向：路径 helper 单源；沙箱与资源加载不再各维护一份 canonicalize。
- 代价：exec 不再是零 `pawork-*` 依赖；policy 的 dunce 语义成为沙箱路径事实源。
- 回滚：恢复 `src/path.rs` 并去掉 Cargo 依赖（不作为本波目标）。

## 实施状态

- 2026-09-04：已实现。`crates/exec/src/path.rs` 删除；`pawork-exec` 生产依赖 `pawork-policy`；sandbox 与 `os/linux.rs` 调用 `canonicalize_platform` / `path_within_root`；本包去掉直接 `dunce` 依赖。`CancellationToken` 仍为 exec 自有类型。定向门禁 `cargo test -p pawork-exec --offline --lib --tests` 64 绿。
