# R9 — 一致性收口

> V3 终局阶段：全仓一致性核对、三类关键回归全量复跑、V2 唯一未收口项（S6 OAuth 真实 refresh）人工验收、文档与登记收口。只读核对 + 文档为主，发现的缺陷按小任务就地修复（超出小任务规模的登记候选，不在本阶段扩张）。状态指针见 [ROADMAP.md](../ROADMAP.md) §1。
>
> 波 A1 已于 2026-08-25 收口（P/S/R 编号谱系确认 + `docs/spec/` 产品 Spec 基线建立 + 常设导航同步 + 候选计数纠正 28 项），细节存档于 [docs/history.md](../docs/history.md)「R9 已完成部分」。**2026-08-25 文档体系重构**（阶段外任务）已将常设文档改组为：architecture/design 拆分、ROADMAP 吸收 task-guide/v3_plan、包级 Spec 取代 MODULE.md/code-map、历史入 history.md——波 A2 的核对对象以重构后的文档集为准。

## 1. 任务清单

### 1.1 遗留收口（波 C）

- **K-01**：config 仓库根路径闭环核对——`.pawork/config.toml` 于 git 仓库根/子目录/非 git 目录三态的发现与合并行为，与 [workspace 包 Spec](../docs/spec/crates/workspace.md) 配置层级描述逐条对照；偏差即修。
- **S6 OAuth 自然临期 refresh**：ChatGPT/xAI 通道等待真实 token 临期（或用户提供临期账号），验证自动 refresh → 重试 → 成功链路与 `invalid_grant` 清理语义；人工验收记录写入本文件附录。F10（两 GUI 冒烟）并入 §1.3 回归。

### 1.2 一致性核对（只读，波 A2）

- **文档一致**：README、AGENTS、ROADMAP、[architecture.md](../docs/architecture.md)（布局/契约表）、[design.md](../docs/design.md)（功能映射/候选池）、[gui-design.md](../docs/gui-design.md)、[spec/](../docs/spec/README.md)（产品篇 + 包级 Spec + flows）、[history.md](../docs/history.md) 相互引用与事实陈述一致；ADR-038~041 决议全部反映到常设文档；无指向已删除文档（task-guide/v3_plan/v2-summary/v1-migration-reference/code-map/research/reviews）的死链。
- **登记项复核**：ROADMAP §5 候选池每项的「复活条件/资产位置」可考（tag `v2-final`、references 附录、history 链接有效）；§4 未决事项逐行核对仍然开放。
- **红线断言在位**：desktop deny-list、engine domain-only、rmcp 模块隔离、policy 成环防护（R1 波 E 建立的断言仍然有效且覆盖 21 包布局）。
- **`let _` 复查**：R4 建立的「副作用 Result 禁静默」清单在 host 域外（providers/storage/tools）抽查，命中即修或登记。
- **包级 Spec 抽查**：`docs/spec/crates/` 21 篇与源码实态抽样核对（模块树、API 面、feature、红线），漂移即回写。

### 1.3 三类关键回归全量复跑（波 B；V2 验证模型的终局执行）

| 类别 | 内容 |
| --- | --- |
| 安全红线 | policy/exec/tools/auth 全部安全种子：路径越界、symlink、`.git` 写、审批 deny、探测 fail-closed、Secret 脱敏（trace 0 泄漏）、外部源 Secret 拒绝 |
| 持久化与重放 | 信封 v1、schema v12 升级链与检入旧库 fixture（v9→v10→v11→v12）、branch lineage/compaction、PWB1、checkpoint、export v3 往返、投影 golden、CommandLedger 崩溃回归 |
| 协议与解析 | GUI 帧 golden、headless-json、ACP 往返、MCP 契约、registry fail-closed、config 六层矩阵、usage dedup |

- 执行方式：`cargo test -p` 逐包（21 包全部过一遍冒烟级；三类回归所在包全量）；**仍不引入 workspace 全量门禁**——那是发布任务的事。
- 真实冒烟：低消耗矩阵（[ROADMAP](../ROADMAP.md) §7.4）四通道各一轮 chat；`gui serve` + desktop probe-smoke；Zed ACP；headless json-stdio；`pawork doctor --json`。
- 顺带复查 ROADMAP §4 挂账：probe `snapshot-reconnect` flake、usage 哨兵口径、Seatbelt 真机探针补强、shell wrapper 收紧评估、`canonical_within` 残余、`cargo tree -d` 上游多版本残留。

### 1.4 收官登记（波 C）

- ROADMAP §1 全 🟢；已完成细节压缩迁入 [docs/history.md](../docs/history.md)；候选池与未决事项终态化。
- 新增 `docs/v3-summary.md`：只压缩已实现范围、冻结契约/ADR、实际验证、人工验收与明确延期项；不得把候选池写成已交付。
- 若用户届时要求：候选「发布/全量门禁」任务书草案（License 前置）——默认不做。

## 2. 波次拆分

| 波 | 内容 | 并行度 | 状态 |
| --- | --- | --- | --- |
| A1 | 编号谱系 + 产品 Spec 基线（存档见 [history.md](../docs/history.md)） | 串行（主代理） | 🟢 2026-08-25 |
| A2 | §1.2 文档/登记/断言核对 + 发现项修复 | 并行 ×2（文档面 / 断言与代码面） | ⚪ |
| B | §1.3 三类回归全量复跑 + 真实冒烟矩阵 | 串行（主代理，结果归档） | ⚪ |
| C | §1.1 K-01 + S6 refresh 人工验收（用户参与）+ §1.4 收官登记 | 串行 | ⚪ |

## 3. 退出标准

- [ ] K-01 闭环一致；S6 refresh 人工验收通过并记录（或显式改判登记）
- [ ] 三类回归全绿；冒烟矩阵四通道 + 三客户端通道通过
- [ ] 文档一致（含包级 Spec 抽查）；候选池与未决事项终态化；红线断言覆盖 21 包布局
- [ ] ROADMAP R0–R9 全 🟢；V3 收官报告（Validated/Targeted regressions/Full gate NOT RUN 格式沿用）
