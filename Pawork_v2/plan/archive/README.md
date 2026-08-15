# archive/ — 已归档的「按域迁移」计划（M0–M8）

本目录保存 V2 第一版计划：按功能域整体迁移的 9 个里程碑文档（M0 骨架与基座 → M8 Release Hardening）。

**这套计划已被 [增量式阶段计划（S0–S12）](../../ROADMAP.md) 取代，不再作为执行依据。** 取代原因：按域迁移要到原 M4 才产出第一个可运行物，之前的 M0–M3 全部是「库先行、无真实消费者」，无法逐步做真实测试与评估——这正是 V1 暴露的「组件齐全、主干未通电」病灶在计划层面的重演。2026-08-16 后又把原 S10 扩展生态移出排期（见 [S10-extensions-deferred.md](S10-extensions-deferred.md)），S7 改为最小 Agent GUI。

## 归档文档的保留价值

各文档中的**包级迁移细则**（V1 合并来源、模块清单、行数实测、拆分动作、冻结契约与 golden 清单、feature 门控设计）依然是执行 V1→V2 代码迁移时的权威参考。新阶段计划在涉及对应包时会直接引用本目录文档，不重复展开细节：

| 归档文档 | 包级细则被以下新阶段引用 |
| --- | --- |
| [M0-skeleton-foundation.md](M0-skeleton-foundation.md) | S0（domain/net/config 最小化）、S1（sqlite）、S6（diagnostics）、S7（protocol 最小帧）、S10（protocol/typegen 收口） |
| [M1-execution-security.md](M1-execution-security.md) | S2/S3（tools）、S3（policy）、S4（exec）、S8（blob-store）、S2（workspace） |
| [M2-providers.md](M2-providers.md) | S0/S2（provider trait 与最小适配）、S5（provider-core/usage/registry）、S6（providers 全厂商 + auth） |
| [M3-storage-session.md](M3-storage-session.md) | S1（session 最小核）、S5（compaction）、S8（git/diff）、S9（导入器） |
| [M4-engine-closed-loop.md](M4-engine-closed-loop.md) | S2–S5（engine 逐步长成）、S10（app/cli 正式化） |
| [M5-connectivity-clients.md](M5-connectivity-clients.md) | S7（最小 gui-server/client/local transport）、S10（transport/sdk/channels/protocol-probe 补齐） |
| [M6-extensions.md](M6-extensions.md) | S9（mcp/resources/compat）；wasm-host/plugin/hooks/lsp **待决策**（[S10-extensions-deferred.md](S10-extensions-deferred.md)） |
| [M7-workflow-control.md](M7-workflow-control.md) | S11（workflow/memory/review/orchestration/control-plane 三包） |
| [M8-release-hardening.md](M8-release-hardening.md) | S12（一次性门禁清单、发布波次、V1 归档，基本原样沿用） |
| [S10-extensions-deferred.md](S10-extensions-deferred.md) | 原增量计划 S10 全文；已移出排期 |

> 注意：归档文档内的相对链接（`../../ROADMAP_V2.md` 等）已失效，属预期：一是目录下移一层，二是 `ROADMAP_V2.md` 已删除、其内容现存于 [../../docs/v1-migration-reference.md](../../docs/v1-migration-reference.md)（原 §1–§6 对应该文 §1–§6、原 §8→§7、原 §10→§8，原 §7/§9 已被新路线图正文取代/吸收）。阅读时以仓库根为基准自行定位。文档内的「里程碑退出标准」「依赖 M×」等表述均指旧计划编号，不映射到新的 S 阶段。
