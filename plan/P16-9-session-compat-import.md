# P16-9：Session 兼容导入（Claude / Codex / Grok / Cursor）

> Phase 16 · Modern Agent Workflow · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P16-1～P16-8、P5-8、P5-9、P0-3、P7-3

**最终目的**：把来自其他智能体工具（Claude / Codex / Grok / Cursor）的外部会话导入为 Pawork 的 canonical event，使既有对话与产物可被重放、检索与续接，而**绝不破坏 canonical event 模型**——外部格式始终是输入侧的适配/投影，导入产物是规范事件，不污染、不覆写既有事件。本任务同时承担 P16 功能簇的兼容/重放收尾门禁。

**涉及范围**：扩展 `session-store` 导入路径（P5-8 export-import / P5-9 pi-jsonl-import 的同类扩展）；复用 `agent-events`（canonical 目标）、`diff-service`（P7-3，外部 patch 锚点）、`artifact-store`、`review-engine`（P16-8，导入评审项锚点）

## 细分步骤

1. **格式探测与 schema** —— 目的：识别 Claude（导出 JSON/对话）、Codex（rollout/JSONL）、Grok、Cursor 四种来源，各自定义只读解析 schema；无法识别或字段缺失时失败可读，不猜测填充。
2. **字段映射到 canonical event** —— 目的：把外部消息/工具调用/产物映射到 `agent-events` 的 canonical 类型（user/assistant/tool/tool_result/usage 等），不可映射的字段保留为 `raw metadata`（遵循 ADR-002 Provider 解耦的 raw 保留原则），**不新增非规范事件类型**。
3. **不可破坏 canonical event** —— 目的：导入只生成新的 canonical event（新 Session / 新 event id），绝不修改、覆盖或删除任何既有 event；导入产物可被 compaction/checkpoint/replay 等既有机制正常消费，导入是「事件生产者」而非「事件改写者」。
4. **patch / 产物锚点** —— 目的：外部会话中的文件改动经 `diff-service`（P7-3）解析为带行锚点的 patch，挂到对应 tool_result；评审类意见经 `review-engine`（P16-8）锚点化，保证位置可定位。
5. **重放与校验** —— 目的：导入后立即对生成的 canonical event 序列做 replay 校验（状态机可推进、无悬空引用、Secret 不落库），失败则整批回滚不入库。
6. **查询面与去重** —— 目的：导入记录来源标识与原始 id，支持按来源去重（同一外部会话重复导入不产生重复 event）；`core-api` 暴露导入入口与导入历史。
7. **簇兼容/重放收尾门禁（独立构建目录）** —— 目的：作为 P16 功能簇的收尾门禁，定向验证「Plan / Goal / Background / Automation / Monitor / Memory / Review / 兼容导入」的 canonical event 可被一致重放；该门禁使用**独立的 `CARGO_TARGET_DIR`**（避免污染主 `target/`），跑完后清理该目录。

## 主要产出物

- 四来源（Claude / Codex / Grok / Cursor）的只读解析器 + 字段映射
- canonical event 导入产物 + replay 校验 + 去重
- P16 簇兼容/重放收尾门禁脚本（独立 `CARGO_TARGET_DIR` + 清理）
- 定向 / Mock smoke 测试

## 验收标准

- [ ] 四来源外部会话可解析并映射为 canonical event，不可映射字段进 raw metadata，不新增非规范事件类型
- [ ] 导入只新增事件，绝不修改/覆盖/删除既有 canonical event
- [ ] 导入产物可通过 replay 校验（状态机可推进、无悬空引用、Secret 不落库），失败整批回滚
- [ ] 同一外部会话重复导入不产生重复 event
- [ ] P16 簇兼容/重放门禁在独立 `CARGO_TARGET_DIR` 下通过，且构建产物在跑完后被清理

**相关文档**：[sessions](../docs/features/sessions.md) · [agent-events (P0-3)](P0-3-event-model.md) · [git-diff](../docs/features/git-diff.md) · [review-engine (P16-8)](P16-8-review-engine.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；JSON/JSONL 解析复用基线 `serde_json`（P2-3/P5-9 同款），patch 锚点复用 `diff-service`。导入扩展在 `session-store` 内完成，不新建 crate。簇门禁脚本约定：

```powershell
$env:CARGO_TARGET_DIR = "target/gates"
try {
    cargo fmt --all -- --check
    cargo test -p plan-service -p goal-service -p task-manager
    cargo test -p automation-service -p monitor-service -p memory-service
    cargo test -p review-engine -p session-store -p agent-events
    cargo clippy -p plan-service -p goal-service -p task-manager -p automation-service -p monitor-service -p memory-service -p review-engine -p session-store --all-targets -- -D warnings
    cargo run -p schema-typegen -- --check
} finally {
    cargo clean --target-dir "target/gates"
    Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
}
```
