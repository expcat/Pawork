# ADR 索引与归档检索

> 2026-10-04（R-10）建立。仓库内大量注释与文档以裸编号引用历史 ADR（如 ADR-038 D8、P14-7 step 2）。统一口径：**不改写既有裸编号引用，按本索引检索正文位置**；本目录不复制归档正文。

## 现行（正文在当前分支）

- **ADR-053 起**：正文为 [docs/spec/](../spec/README.md) 各产品篇的「决策：ADR-0XX」节（settings / desktop / model-gateway 等），`rg "ADR-0XX" docs/spec` 定位。
- **ADR-042～ADR-052**：无独立正文文件，决策语义已内联在现行 [architecture.md](../architecture.md) 与对应 Spec 段落（如 ADR-046 `ApiKeySecret` 脱敏、ADR-050 终端设置）；编号为历史出处标记，`rg` 可定位语义段落。
- **ADR-040 / ADR-041**（分支模型 / 沙箱隔离）：`git show v2-final:plan/R6-session-branching.md`、`git show v2-final:plan/R7-sandbox-isolation.md`。

## 归档（正文不在当前分支）

| 编号段 | 正文位置 | 说明 |
| --- | --- | --- |
| ADR-001～ADR-036 | `git ls-tree --name-only b7b3d5af docs/adr/` 定位文件，再 `git show b7b3d5af:<完整路径>` | V1 决策目录完整快照；b7b3d5af 是归档收缩（ca24df16）前最后版本 |
| ADR-037 | `git show v2-final:docs/adr/ADR-037-s13-wave-b-contracts.md` | V2 S13 wave B 契约 |
| ADR-038 | `git show v2-final:plan/R0-inventory-decisions.md` | V3 R0 库存与产品形态（D1～D16 决策清单） |
| ADR-039 | `git show v2-final:plan/R1-package-consolidation.md` | V3 R1 包布局合并 |
| P12～P18 阶段计划 | `git ls-tree --name-only b7b3d5af plan/` 定位文件，再 `git show b7b3d5af:<完整路径>` | V1 阶段计划（如 P14-7 = `plan/P14-7-quota-local-usage-budget.md`）；`git show` 不展开路径通配符 |

## 约定

- 新增架构决策不再新开裸编号：直接在对应产品 Spec 落「决策：ADR-0XX（日期）」节，编号顺延。
- 引用归档 ADR / P 编号时无需附路径；读者按本索引检索。涉及冻结契约的归档决议以 [architecture.md](../architecture.md) §3 现行清单为准。
