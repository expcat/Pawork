# 测试体系

测试分为开发期快速验证、功能簇收尾门禁和维护/发布门禁。目标是在接口高频变化阶段优先完成实现与主干接线，避免每个任务重复运行 workspace 全量测试；功能稳定后再集中补齐跨 crate、跨 Provider、跨平台和长耗时门禁。

## 运行层级

| 层级 | 何时运行 | 内容 | 缓存策略 |
| --- | --- | --- | --- |
| L0 | 每次编辑后 | 存在性、链接、diff、生成物检查 | 不创建独立构建缓存 |
| L1 | 单任务收尾 | changed crates、必要关键 reverse dependents 与定向 regression | 复用默认 `target/` |
| L2 | 功能簇基本收尾 | 相关 crates 的 integration/contract/golden/schema、定向 fmt/clippy；必要时经明确升级执行 Workspace Full Gate | 使用 `target/gates`，结束即清理 |
| L3 | 发布候选、Maintenance/Release Gate、重大依赖/协议升级 | workspace 全量、三平台、安全、性能、fuzz/chaos/差分 | 隔离目录；本地结束即清理，CI 可使用短期缓存 |

任务默认只要求 L0/L1。Secret、Policy/路径、事件持久化/重放、破坏性文件或进程清理、协议兼容等高风险不变量必须随改动执行定向回归，不等待 L2/L3。

## Affected-crate 判断

每个任务先形成验证集合，而不是先选固定 Cargo 命令。推荐步骤如下：

1. **取得任务 diff**：结合 `git status --short`、任务基线到 `HEAD` 的 committed diff、staged/unstaged diff 与本任务新增文件。脏工作区中排除用户原有且与任务无关的改动，不能把整个工作区变化都算成本任务影响面。
2. **映射 changed crates（A0）**：`crates/<dir>/`、`apps/<dir>/` 可先按目录定位，再用 `cargo metadata --format-version 1 --no-deps` 的 `packages[].manifest_path` 和 package name 校准。文档、fixture、schema、根配置按实际消费者归属，不能只按最近目录猜测。
3. **判定接口扇出**：检查是否改变 `pub` API、Cargo feature、shared/canonical domain、GUI Connection Protocol、序列化/持久化格式、schema/typegen 或共享测试夹具。crate 私有实现通常令验证集合 `A = A0`。
4. **选择关键反向依赖（A1）**：需要时运行 `cargo tree --workspace --invert <crate> --depth 1`，或读取不带 `--no-deps` 的 `cargo metadata` 中 `resolve.nodes[].deps`。公共接口改动令 `A = A0 +` 实际消费该接口的关键直接 reverse dependents；不要把所有传递反向依赖无差别加入。
5. **加入定向回归（R）**：contract、golden、schema、Secret、Policy、路径边界、事件重放、破坏性操作、协议兼容等按语义加入测试 target。最终 L1 范围是 `A0 + 必要 A1 + R`。

根 `Cargo.toml`、`Cargo.lock`、`.cargo/config*`、`rust-toolchain*`、build script 或共享生成配置变化需要单独分析：普通 package 依赖增删通常仍可落到相关 crates；workspace members/resolver/profile、toolchain 或关键依赖的重大变化才是 Full Gate 候选。文件在根目录不是自动全量的理由。

canonical domain / protocol 变化优先选择 changed crate、主要 producer/consumer、serializer/typegen 与 contract crate；只有这些仍不足以覆盖兼容面时才扩大一层。单个 Provider 只验证对应 adapter/runtime/contract，GUI 只验证实际 projection/controller/protocol 消费链及必要视觉/平台回归，平台模块只验证相关 target/harness。Provider、GUI、平台标签本身都不是 workspace 全量理由。

## 最小命令选择

以下命令是候选，不是必须顺序；多个相关 crate 使用多个 `-p`：

```bash
cargo check -p <crate-a> -p <crate-b>
cargo test -p <crate-a> -p <crate-b>
cargo clippy -p <crate-a> -p <crate-b> --all-targets -- -D warnings
```

- 只需类型、feature 或条件编译反馈时选 `cargo check`。
- 需要行为证据时选定向 `cargo test`；它已经完成所需测试产物编译，没有 binary/link/build-script/特定 profile 或发布产物行为需要验证时，不再追加 `cargo build`。
- Rust 代码的 lint 风险、all-targets 测试代码或任务验收明确需要时选定向 `cargo clippy`；不要为了凑齐命令矩阵机械运行。
- 优先运行具体 test target、test filter、contract、golden、schema 或 regression；schema/typegen 未受影响时不运行 `schema-typegen --check`。
- 文档或不改变构建行为的配置任务可以不运行 Cargo 编译，只做链接、格式、配置解析、命令一致性与 diff 检查。
- 不允许因相关 crate 数量变多就把多个 `-p` 换成 `--workspace`。

`check + build + test + clippy` 不是完整性的定义。验证集是否有效取决于它覆盖了实际改动及风险，而不是命令数量。

## 单元测试

覆盖：状态机；Provider parser；Tool arguments；Token budget；Compaction；Diff；Patch；路径；Policy；Session reducer；Plugin manifest；Event ordering；Desktop Snapshot/Event reducer、command reconciliation 与组件交互。

开发期按受影响 crate 定向运行，不以 `cargo test --workspace` 作为每个任务的完成条件。

## Provider Contract Tests

每个 Provider 使用相同测试套件：text；tool call；multiple tool calls；image；thinking；usage；stop reason；cancel；timeout；rate limit；malformed stream；partial JSON；reconnect；context overflow。

适配器开发期只运行变更路径对应的最小 contract 子集；基础协议矩阵在对应适配任务收尾执行，现代 hosted tools / reasoning / citation / capability negotiation 的完整矩阵集中在 P15-9 与 L3，不在 P15-1～P15-8 重复跑三家全套。

## Plugin Contract Tests

Phase 10 使用统一的 Plugin API v1 contract：manifest canonical signing payload 必须同时绑定 metadata 与
component bytes；Ed25519 未知 key/篡改签名 fail closed；host API semver 覆盖兼容范围、minor 演进与跨
major 拒绝；冻结 JSON golden 与 `wit-bindgen` guest binding 锁定 v1 WIT/JSON；Component ABI 覆盖
`invoke(string) -> string`、畸形 JSON、trap、Fuel、memory、timeout 与 cancel；注册覆盖统一
`PluginRuntime` 的 load/回滚/unload、namespace/冲突/`ExternalPlugin` 调度；state 覆盖 revision、
plugin/scope 隔离、配额与 unload/apply 竞态；lifecycle hook 覆盖确定性顺序、订阅、注销和单插件
error/panic/cancel 隔离。

兼容矩阵与断言辅助位于 `test-support`，实现测试位于 `plugin-api`、`wasm-plugin-host` 与
`hook-runtime`。它们自动进入手动触发的 L3 三平台 Workspace Full Gate CI；开发期仍按实际改动使用
多个 `-p` 定向运行，无需额外 wasm target 或外部 runtime。测试组件使用 Wasmtime 的 Component text
fixture 在进程内构造，WIT guest binding 只做 host-native compile gate。

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

## Desktop GUI Tests

Phase 19 使用四层证据：纯 Rust unit/property tests 覆盖 projection、controller 与 command reconciliation；GPUI component/test context 覆盖 Element、键盘、焦点、选择和虚拟列表；OS 原生场景 harness 覆盖 Windows/macOS/Linux 的连接、审批、Timeline、Diff、Terminal、重连、窗口和通知；visual baseline 只固定稳定窗口尺寸、字体 fixture 与主题。Component/headless harness 不能替代真实平台的 IME、GPU/驱动、窗口、系统通知、AccessKit 读屏、签名包与更新验证。

协议 projection 必须对乱序、重复、sequence gap、Snapshot 与 Event 竞态、陈旧 command response、重连回放和 unsupported version 做 property/fixture 测试。Visual 变更须人工审阅；accessibility gate 覆盖全键盘路径、焦点恢复、Windows Narrator/macOS VoiceOver/Linux Orca、状态通知节流、对比度、200% scaling 与 reduced-motion。

## Fuzz Tests

重点 Fuzz：SSE；JSON Lines；Tool Partial JSON；Unified Diff；Patch；Session Import；路径；Plugin Manifest；MCP Message；Artifact Metadata。

开发期只对修改过的 parser/patch/path 运行短时 smoke fuzz；持续 fuzz、corpus 扩展与 sanitizer 属于计划任务或 L3。发现的最小复现加入版本化 corpus，不作为临时缓存清理。

## Chaos Tests

模拟：Provider 中途断网；Core 崩溃；数据库锁；磁盘满；Tool 进程不退出；Side process 持有 stdout；文件被用户同时修改；Git Index 变化；Plugin 崩溃；MCP Server 崩溃；lease owner 崩溃；account cooldown/recovery；热切换回滚；session ownership epoch 冲突；跨 tenant 并发访问；Desktop controller 断线/重连、事件缺口、慢视图消费、多窗口竞争、GPU/backend 降级与更新中断。

Chaos 默认在功能主干接线完成后的 L2/L3 执行，不阻塞前期领域模型、协议骨架或单个 adapter 的频繁迭代。

## 差分测试

以 Pi 作参考行为（而非运行时依赖）。对同一 Mock Provider 脚本比较：Agent 消息顺序；Tool Call 顺序；Session 分支；Compaction 触发；Cancellation；错误恢复。不要求内部实现一致，只检查产品行为。

差分测试用于协议/引擎升级与发布维护，不要求每个细节任务运行。外部参考实现版本必须记录，避免把上游变化误判为 Pawork 回归。

## Cargo Profile

日常 dev/test 采用 line-tables-only 且第三方依赖 debug=false（见根 Cargo.toml），降低 DWARF/PDB 与链接成本；需要完整调试器时显式 `cargo build --profile debugging` / `cargo test --profile debugging`；不以默认关闭 L1 incremental compilation 为手段。

## Workspace Full Gate

以下任一明确条件成立时，才可把验证升级为 Workspace Full Gate：

- 功能簇整体收尾或专门的 L2 Gate 任务；
- 大规模跨 crate 重构，无法用 changed crates + 关键消费者充分界定；
- workspace members/resolver/profile、toolchain 或关键依赖发生重大变化；
- canonical protocol/domain 大范围变更，影响多组 producer/consumer/serializer/contract；
- Maintenance/Release Gate；
- 用户明确要求 workspace 全量验证。

单个 crate 的公共 API 变化、高风险修改或多个相关 crate 都不自动满足升级条件：前者先加入主要消费者，后者执行对应定向 regression。“保险”“最终确认”“确保没有回归”“改动较多”或“任务已经完成”不是升级理由。Agent 在执行前必须指出命中的具体条件；否则 Full Gate 保持 `NOT RUN`。

Workspace Full Gate 保留以下命令：

```bash
cargo build --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

这三条属于 L2/L3 Maintenance/Release/Full Gate，不是普通任务验收步骤。

## 测试后清理

定向 L1 复用默认 `target/` 增量缓存，只清理本任务产生的临时目录、fixture 副本、日志、coverage 与未确认快照输出；测试代码优先使用 RAII/tempfile，确保失败和取消路径也回收。每次 L1 后执行 `cargo clean` 会迫使后续重编，默认禁止。

定向 L2 仍使用相关 crates 的多个 `-p`，可放入隔离 `target/gates` 并在 `finally` 定向清理；不能因为使用隔离目录就改成 `--workspace`。只有上一节升级条件成立时，才使用以下 Workspace Full Gate 脚本：

```powershell
$env:CARGO_TARGET_DIR = "target/gates"
$env:CARGO_INCREMENTAL = "0"
try {
    cargo build --workspace --all-targets
    cargo test --workspace
    cargo clippy --workspace --all-targets -- -D warnings
} finally {
    cargo clean --target-dir "target/gates"
    Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
    Remove-Item Env:CARGO_INCREMENTAL -ErrorAction SilentlyContinue
}
```

本地 Gate 只有在 Rust 格式或 schema/typegen 确实可能受影响时，才加入 `cargo fmt --all -- --check` 或 `cargo run -p schema-typegen -- --check`；普通 L1 可使用定向 fmt 或 schema check。手动三平台 L3 CI 是固定 Maintenance/Release Gate，始终包含 fmt 与 schema drift check。默认 `target/` 仅在达到团队配置阈值、磁盘压力告警或用户明确要求时清理，不得把任务收尾当作清理触发器。CI 为一次性 runner 时无需为了清理增加额外耗时；持久化 runner 只缓存 lockfile/工具链可复用内容，并设置容量或 TTL。

## 任务验收报告

普通任务完成时明确报告实际验证范围；没有运行 Workspace Full Gate 不是缺口：

```text
Validation Level: L1
Affected crates: <changed + selected reverse dependents，或 none>
Validated: <实际命令 / tests / checks>
Targeted regressions: <实际覆盖，或 none>
Full workspace gate: NOT RUN (<未命中升级条件>)
```

如实际运行 L2/L3 或 Workspace Full Gate，替换层级、范围和结果，并写明触发条件。不得把未运行的命令列入 `Validated`。

## Integration Test 组织

当前每个 crate 至多 1 个 tests/*.rs；新增大量顶层 tests/*.rs 前应评估独立 test executable 带来的链接与磁盘成本。对依赖树较大或 integration test 较多的 crate 默认优先「少量 test target + 多 module」组织；不以减少 production crate 数量作为控制 target/ 的手段。

## 相关文档

- [性能目标](performance-targets.md) · [安全验收](security-acceptance.md)
- [Desktop GUI](../features/desktop-gui.md) · [P19-16 Desktop Gate](../../plan/P19-16-desktop-gate.md)
- [ROADMAP 实施波次与门禁节奏](../../ROADMAP.md#实施波次与门禁节奏) · [plan 测试节奏与缓存清理](../../plan/README.md#测试节奏与缓存清理)
