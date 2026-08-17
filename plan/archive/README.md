# archive/ — 已归档的「按域迁移」计划（M0–M8）

本目录原定保存 V2 第一版「按功能域整体迁移」的 9 个里程碑文档（M0 骨架与基座 → M8 Release Hardening）。当前仓库与可见 git 历史中均没有 M0–M8 正文；这里只保留编号索引，以及实际存在的 [S10-extensions-deferred.md](S10-extensions-deferred.md)。

**这套计划已被 [增量式阶段计划（S0–S12）](../../ROADMAP.md) 取代，不再作为执行依据。** 取代原因：按域迁移要到原 M4 才产出第一个可运行物，之前的 M0–M3 全部是「库先行、无真实消费者」，无法逐步做真实测试与评估——这正是 V1 暴露的「组件齐全、主干未通电」病灶在计划层面的重演。2026-08-16 后又把原 S10 扩展生态移出排期（见 [S10-extensions-deferred.md](S10-extensions-deferred.md)），S7 改为最小 Agent GUI。

## 缺失正文的回退规则

M0–M8 只作历史编号，不得把 README 中的概括扩写成不存在的细则。V1→V2 的包级来源、目录映射与迁移方式统一以 [v1-migration-reference.md §4.1](../../docs/v1-migration-reference.md) 为事实源；若阶段任务书引用下列缺失文件，按该映射表回退并登记文档债务。

| 历史编号（正文缺失） | 原计划覆盖范围 / 当前阶段线索 |
| --- | --- |
| `M0-skeleton-foundation.md` | S0（domain/net/config 最小化）、S1（sqlite）、S6（diagnostics）、S7（protocol 最小帧）、S10（protocol/typegen 收口） |
| `M1-execution-security.md` | S2/S3（tools）、S3（policy）、S4（exec）、S8（blob-store）、S2（workspace） |
| `M2-providers.md` | S0/S2（provider trait 与最小适配）、S5（provider-core/usage/registry）、S6（providers + auth） |
| `M3-storage-session.md` | S1（session 最小核）、S5（compaction）、S8（git/diff）、S9（导入器） |
| `M4-engine-closed-loop.md` | S2–S5（engine 逐步长成）、S10（app/cli 正式化） |
| `M5-connectivity-clients.md` | S7（最小 gui-server/client/local transport）、S10（transport/sdk/channels/protocol-probe 补齐） |
| `M6-extensions.md` | S9（mcp/resources/compat）；wasm-host/plugin/hooks/lsp 待决策，现有材料见 [S10-extensions-deferred.md](S10-extensions-deferred.md) |
| `M7-workflow-control.md` | S11（workflow/memory/review/orchestration/control-plane） |
| `M8-release-hardening.md` | 不再映射当前 S12；历史门禁/发布清单保留于 [V1 迁移参考 §6.3](../../docs/v1-migration-reference.md#63-release-hardening-一次性清单原-m8)，未来发布时另立任务 |

> 注意：不要创建空壳 M0–M8 文件来“修复”引用；只有从可信历史或用户提供的原文恢复时才可补回正文。
