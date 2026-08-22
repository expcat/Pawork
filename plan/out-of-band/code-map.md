# 阶段外任务：三层代码地图

> 对应 [ROADMAP.md](../../ROADMAP.md) §3.1b。用户已确认方案。纯文档任务：不改 Rust 业务源码、不改冻结契约、不函数级铺开。

## 1. 目标与非目标

**目标**

1. 总索引 [`docs/code-map/README.md`](../../docs/code-map/README.md)：按依赖自底向上列出 21 个 workspace 成员，并链到各 `MODULE.md`。
2. 每个 crate / app 一份 `MODULE.md`（crate/app 根目录；函数级不铺开，公开细节以 rustdoc / 源码为准）。
3. 热点深描 [`docs/code-map/hotspots/`](../../docs/code-map/hotspots/)：模块写完后再补；可先空目录。
4. ROADMAP §3.1b 登记为进行中；收尾移入 §3.1，并回写 [AGENTS.md](../../AGENTS.md) §6 与 [README.md](../../README.md) 文档导航各一行。

**非目标**

- 不改任何 `.rs` / `Cargo.toml` / golden / schema / wire。
- 不把已归档代码（tag `v2-final`）写成现行 API。
- 不编造类型、trait、子命令或依赖边；对照实态源码、`Cargo.toml`、[docs/design.md](../../docs/design.md) §2。
- 不把本任务并入 R0–R9 阶段排期。

## 2. 三层结构

| 层 | 位置 | 职责 |
| --- | --- | --- |
| 1 总索引 | `docs/code-map/README.md` | 包清单、依赖自底向上顺序、何时加载哪份 `MODULE.md` |
| 2 模块图 | `crates/<短名>/MODULE.md`、`apps/{pawork,desktop}/MODULE.md` | 职责 / 模块树 / 对外入口 / 依赖与被依赖 / 红线 / 相关文档 |
| 3 热点深描 | `docs/code-map/hotspots/` | 跨包热路径（Agent loop、审批、GUI 帧、存储重放等）；模块完成后补 |

`MODULE.md` 固定小节（顺序不可改）：**职责 · 模块树 · 对外入口/API 面 · 依赖与被依赖 · 红线与注意事项 · 相关文档**。短、可扫，给 Agent 按需加载。

## 3. 提交队列

每一个模块一个独立 git commit；脚手架单独一 commit。信息英文，风格 `docs(code-map): …`。禁止 push / force / 改 main。缺 identity 时用一次性 `-c user.name` / `-c user.email`，禁止 `git config`。

| # | 内容 | 路径 |
| --- | --- | --- |
| 0 | 脚手架：本任务书 + 总索引 + ROADMAP §3.1b | `plan/out-of-band/code-map.md`、`docs/code-map/README.md`、`ROADMAP.md` |
| 1 | `pawork-domain` | `crates/domain/MODULE.md` |
| 2 | `pawork-exec` | `crates/exec/MODULE.md` |
| 3 | `pawork-transport` | `crates/transport/MODULE.md` |
| 4 | `pawork-protocol` | `crates/protocol/MODULE.md` |
| 5 | `pawork-testkit` | `crates/testkit/MODULE.md` |
| 6 | `pawork-policy` | `crates/policy/MODULE.md` |
| 7 | `pawork-auth` | `crates/auth/MODULE.md` |
| 8 | `pawork-storage` | `crates/storage/MODULE.md` |
| 9 | `pawork-providers` | `crates/providers/MODULE.md` |
| 10 | `pawork-workflow` | `crates/workflow/MODULE.md` |
| 11 | `pawork-control-plane` | `crates/control-plane/MODULE.md` |
| 12 | `pawork-workspace` | `crates/workspace/MODULE.md` |
| 13 | `pawork-git` | `crates/git/MODULE.md` |
| 14 | `pawork-tools` | `crates/tools/MODULE.md` |
| 15 | `pawork-engine` | `crates/engine/MODULE.md` |
| 16 | `pawork-orchestration` | `crates/orchestration/MODULE.md` |
| 17 | `pawork-client` | `crates/client/MODULE.md` |
| 18 | `pawork-app` | `crates/app/MODULE.md` |
| 19 | `pawork-cli` | `crates/cli/MODULE.md` |
| 20 | `pawork`（bin） | `apps/pawork/MODULE.md` |
| 21 | `pawork-desktop`（bin） | `apps/desktop/MODULE.md` |
| 22 | 热点 + AGENTS.md §6 / README 文档导航回写；ROADMAP §3.1b → §3.1 | `docs/code-map/hotspots/`、`AGENTS.md`、`README.md`、`ROADMAP.md` |

顺序 = 依赖自底向上（[docs/design.md](../../docs/design.md) §2）。#22 必须在 #1–#21 全部落地后。

## 4. 验证

文档任务，不为形式完整跑编译。每份 `MODULE.md` 对照：

- 该包 `Cargo.toml` 的 `pawork-*` 依赖与 feature
- `src/` 模块树与 `lib.rs` / `main.rs` 对外 `pub use` / 入口
- [docs/design.md](../../docs/design.md) §2 依赖方向与备注
- [AGENTS.md](../../AGENTS.md) §2 红线（domain 纯净、Secret、Engine 不按 Provider 名分支、GUI 不直连 Core）

总索引链接必须指向真实已落地的 `MODULE.md`（#1–#21 完成后满足）。

## 5. 退出标准

- [ ] #0–#21 完成且各有 commit
- [ ] 总索引链接指向真实 `MODULE.md`
- [ ] ROADMAP §3.1b 进度正确（进行中 → 完成后移入 §3.1）
- [ ] #22 热点目录有说明；AGENTS.md §6 / README 文档导航各增一行
- [ ] 工作区干净；不改 Rust 源码与冻结契约
