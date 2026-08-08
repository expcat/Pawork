# P1-13：Phase 1 评审修复（REVIEW remediation）

> Phase 1 · 基础设施 · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P1-1 ~ P1-12

**最终目的**：消除 [REVIEW.md](../REVIEW.md) §1（Phase 1）评审发现的安全红线、健壮性缺陷与基线卫生问题——让「Secret 不落库」红线被 Event Store 序列化边界的脱敏与契约测试守护，关闭 `trust_workspaces` 的自我提权面，收敛 file-index/artifact-store 的阻塞与无界增长隐患，并使 workspace 基线声明与实际依赖一一对应。

**涉及范围**：`config-service`、`session-store`、`artifact-store`、`diagnostics`、`file-index`、`workspace-service`、根 `Cargo.toml`、ROADMAP「依赖选型基线」

## 细分步骤（分组）

### A. 安全红线（V1 / V2）

1. **V2 Event Store 脱敏**：在 `session-store/src/event_store.rs` 序列化 `AgentEventEnvelope` 入 `payload_json` 的边界增加 redaction guard（对 options/headers/token 类字段白名单或掩码），断言落库 JSON 不含明文 secret。目的：守住「Secret 不写入数据库」红线（[ADR-014](../docs/adr/ADR-014-secret-os-keychain.md)）。
2. **V1 trust_workspaces 收口**：在 `config-service` 将 `trust_workspaces` 限定为仅全局层可读，workspace 层覆盖直接忽略并告警，补回归测试。目的：趁字段未消费时消除自我提权攻击面。

### B. 健壮性与安全加固（V3 ~ V8）

3. **V3 verbatim 路径**：在 `workspace-service` 出口统一 `dunce::simplified`，消除 Windows `\\?\` 前缀流入子进程 cwd。目的：与 P7-9 V6 / P11-8 同根，本任务收口 workspace 出口。
4. **V4 临时文件残留**：`artifact-store` GC 扫描附带清理 mtime 超阈值（24h）的 `.tmp-` 孤儿文件。目的：修复崩溃残留的磁盘泄漏。
5. **V5 诊断包 TOCTOU**：`diagnostics` bundle 落位改用 create-new 语义或带序号/时间戳命名，关闭「先 exists 再 rename」的覆盖窗口。目的：兑现「不覆盖已有文件」注释承诺。
6. **V6 redaction 残余风险文档化**：在 `docs/features/` 与 `docs/quality/security-acceptance.md` 写明诊断包脱敏为 best-effort、分享前需人工确认，并把典型漏报形态（URL query、嵌套 JSON 转义、自定义 header）纳入回归样本。目的：让残余风险可见、可测。
7. **V7 file-index 阻塞回调**：将 notify 回调的 `blocking_send` 改为 `try_send`，满时合并/丢弃并计数（或按基线统一改用 debouncer）。目的：消除事件风暴下 watcher 线程阻塞与 OS 事件缓冲溢出。
8. **V8 file-index 错误无界**：给 `errors` 列表设上限（1024）并环形淘汰，导出标注截断。目的：修复错误风暴下内存无界增长。

### C. 基线与包清理

9. **零引用声明**：从根 `Cargo.toml` 移除 `uuid`、`tracing-appender`（全仓库零引用；`similar`、`parking_lot`/`tempfile` 归 P7-9，`base64`/`rand`/`sha2`/`url` 归 P6-14，避免跨任务改同一行）。目的：基线声明与实际依赖一致。
10. **debouncer 归属订正**：在 ROADMAP 基线把 `notify-debouncer-full` 关联任务补 P7-6（真实首用为 git-service 缓存失效器），并在 plan 记录 file-index 与 git-service 去抖统一决策。目的：基线描述名副其实。

### D. 文档同步

11. 同步 ROADMAP「依赖选型基线」本任务所辖行；按 V6 更新诊断/security 文档。目的：文档与代码一致。

## 主要产出物

- Event Store 脱敏 guard + 契约测试；`trust_workspaces` 全局层限制 + 回归测试
- artifact-store `.tmp-` 清理、diagnostics bundle create-new、file-index try_send + 有界 errors
- 根 `Cargo.toml` 基线清理（uuid/tracing-appender）；ROADMAP 基线同步

## 验收标准（保留 REVIEW 追踪编号）

- [ ] **V2**：构造携带假 token 的事件写入 Event Store，断言 `payload_json` 不含明文 token（契约测试通过）
- [ ] **V1**：workspace 层 `trust_workspaces = true` 被忽略并告警，仅全局层生效（回归测试）
- [ ] **V3**：workspace-service 出口对 Windows verbatim 路径应用 `dunce::simplified`（路径测试覆盖）
- [ ] **V4**：GC 清理 mtime > 24h 的 `.tmp-` 文件（构造孤儿文件验证回收）
- [ ] **V5**：diagnostics bundle 落位不再覆盖既有文件（create-new/序号命名，TOCTOU 测试）
- [ ] **V6**：`docs/features/*` 与 `security-acceptance.md` 写明诊断包 best-effort 脱敏与人工确认要求
- [ ] **V7**：file-index watcher 回调改 `try_send`，风暴场景不阻塞（并发测试）
- [ ] **V8**：file-index `errors` 上限 1024 环形淘汰并标注截断（测试）
- [ ] **基线**：`uuid`、`tracing-appender` 从根 `Cargo.toml` 移除（或补豁免理由），ROADMAP 基线表同步
- [ ] **归属**：`notify-debouncer-full` 基线关联 P7-6，去抖统一方案记录于 plan
- [ ] **快速验证**：只运行本任务涉及 crate 的定向测试与必要 `cargo check -p <crate>`；Phase 1～7 remediation 全部收尾后统一执行 Core 主干 L2，不在本任务重复 workspace 全量门禁

**相关文档**：[REVIEW.md](../REVIEW.md) §1 · [ADR-014 Secret 走 OS Keychain](../docs/adr/ADR-014-secret-os-keychain.md) · [security-acceptance](../docs/quality/security-acceptance.md) · [ROADMAP 依赖选型基线](../ROADMAP.md#依赖选型基线)

> 基线去留决策（2026-08 review）：`uuid`/`tracing-appender` 暂无消费者，移出基线；未来需要全局唯一 ID 或日志落盘时再按基线流程重新引入。
