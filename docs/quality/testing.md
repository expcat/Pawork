# 测试体系

测试分为开发期快速验证、功能簇收尾门禁和维护/发布门禁。目标是在接口高频变化阶段优先完成实现与主干接线，避免每个任务重复运行 workspace 全量测试；功能稳定后再集中补齐跨 crate、跨 Provider、跨平台和长耗时门禁。

## 运行层级

| 层级 | 何时运行 | 内容 | 缓存策略 |
| --- | --- | --- | --- |
| L0 | 每次编辑后 | 存在性、链接、diff、生成物检查 | 不创建独立构建缓存 |
| L1 | 单任务收尾 | 受影响 crate 的单元/Mock/smoke，必要时 `cargo check -p <crate>` | 复用默认 `target/` |
| L2 | 功能簇基本收尾 | 相关 crates 的 integration/contract/golden/schema、一次 fmt/clippy | 使用 `target/gates`，结束即清理 |
| L3 | 发布候选、依赖/协议升级、主干合并前 | workspace 全量、三平台、安全、性能、fuzz/chaos/差分 | 隔离目录；本地结束即清理，CI 可使用短期缓存 |

任务默认只要求 L0/L1。Secret、Policy/路径、事件持久化/重放、破坏性文件或进程清理、协议兼容等高风险不变量必须随改动执行定向回归，不等待 L2/L3。

## 单元测试

覆盖：状态机；Provider parser；Tool arguments；Token budget；Compaction；Diff；Patch；路径；Policy；Session reducer；Plugin manifest；Event ordering。

开发期按受影响 crate 定向运行，不以 `cargo test --workspace` 作为每个任务的完成条件。

## Provider Contract Tests

每个 Provider 使用相同测试套件：text；tool call；multiple tool calls；image；thinking；usage；stop reason；cancel；timeout；rate limit；malformed stream；partial JSON；reconnect；context overflow。

适配器开发期只运行变更路径对应的最小 contract 子集；基础协议矩阵在对应适配任务收尾执行，现代 hosted tools / reasoning / citation / capability negotiation 的完整矩阵集中在 P15-9 与 L3，不在 P15-1～P15-8 重复跑三家全套。

## Control Plane Contract Tests

P18 使用独立于 Provider wire contract 的测试簇：selector property tests（priority/weighted/fill-first/affinity）、lease concurrency/reclaim、scope-aware error/cooldown、legacy migration、cross-tenant isolation、usage replay/idempotency 与 audit redaction。Agent cancel、`ContextTooLarge`、`ProtocolIncompatible` 必须验证不会误伤 credential health。

Codex/Claude/ACP 的协议 fixture 按 client + protocol version 固定：Codex 覆盖 Thread/Turn/Item/approval/subagent/interrupt；Claude 覆盖 Messages/tool/三类 identity header/signed reasoning；ACP 覆盖 initialize/session/prompt/update/permission/cancel/unsupported method。完整矩阵集中在 P18-15，单 adapter 任务只跑自身 golden 子集。

## Mock Provider

先实现完全可编程的 Mock Provider，绝大部分 Agent 测试不依赖真实 API。

```rust
MockScript::new()
    .text("Starting")
    .tool_call("read_file", json!({...}))
    .tool_call("edit_file", json!({...}))
    .text("Done")
    .complete();
```

Phase 0 的实现位于 `test-support`：脚本可输出 text、多个 tool call、跨 chunk partial JSON、完成或等待取消；`MockProvider` / `MockTool` 均记录调用并提供顺序与参数断言。最小链路测试不访问网络，覆盖 text → tool call → tool result → complete 以及 provider/tool 取消传播。

## Golden Tests

固定：System Prompt；Tool Schema；Context；Session Events；Compaction；Pi Import；Diff；API JSON Schema；Codex/Claude/ACP protocol frames 与 capability snapshots。

Golden 仅在相关序列化或用户可见语义稳定后进入 L2。开发中允许先生成候选快照，但更新基线必须人工审阅；已确认基线不是缓存，不随清理删除。

## Fuzz Tests

重点 Fuzz：SSE；JSON Lines；Tool Partial JSON；Unified Diff；Patch；Session Import；路径；Plugin Manifest；MCP Message；Artifact Metadata。

开发期只对修改过的 parser/patch/path 运行短时 smoke fuzz；持续 fuzz、corpus 扩展与 sanitizer 属于计划任务或 L3。发现的最小复现加入版本化 corpus，不作为临时缓存清理。

## Chaos Tests

模拟：Provider 中途断网；Core 崩溃；数据库锁；磁盘满；Tool 进程不退出；Side process 持有 stdout；文件被用户同时修改；Git Index 变化；Plugin 崩溃；MCP Server 崩溃；lease owner 崩溃；account cooldown/recovery；热切换回滚；session ownership epoch 冲突；跨 tenant 并发访问。

Chaos 默认在功能主干接线完成后的 L2/L3 执行，不阻塞前期领域模型、协议骨架或单个 adapter 的频繁迭代。

## 差分测试

以 Pi 作参考行为（而非运行时依赖）。对同一 Mock Provider 脚本比较：Agent 消息顺序；Tool Call 顺序；Session 分支；Compaction 触发；Cancellation；错误恢复。不要求内部实现一致，只检查产品行为。

差分测试用于协议/引擎升级与发布维护，不要求每个细节任务运行。外部参考实现版本必须记录，避免把上游变化误判为 Pawork 回归。

## 测试后清理

定向 L1 复用默认 `target/`，只清理本任务产生的临时目录、fixture 副本、日志、coverage 与未确认快照输出；测试代码优先使用 RAII/tempfile，确保失败和取消路径也回收。每次 L1 后执行 `cargo clean` 会迫使后续全量重编，默认禁止。

本地 L2/L3 使用隔离目录并在 `finally` 清理：

```powershell
$env:CARGO_TARGET_DIR = "target/gates"
try {
    cargo fmt --all -- --check
    cargo build --workspace --all-targets
    cargo test --workspace
    cargo clippy --workspace --all-targets -- -D warnings
    cargo run -p schema-typegen -- --check
} finally {
    cargo clean --target-dir "target/gates"
    Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
}
```

包含格式或 schema 检查时，把 `cargo fmt --all -- --check` 与 `cargo run -p schema-typegen -- --check` 放入同一个 `try`。默认 `target/` 在功能簇收尾检查体积，达到团队配置阈值或磁盘压力告警时再执行 `cargo clean`。CI 为一次性 runner 时无需为了清理增加额外耗时；持久化 runner 只缓存 lockfile/工具链可复用内容，并设置容量或 TTL。

## 相关文档

- [性能目标](performance-targets.md) · [安全验收](security-acceptance.md)
- [ROADMAP 实施波次与门禁节奏](../../ROADMAP.md#实施波次与门禁节奏) · [plan 测试节奏与缓存清理](../../plan/README.md#测试节奏与缓存清理)
