# P9-8：Phase 9 评审修复（REVIEW remediation）

> Phase 9 · MCP · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P9-1 ~ P9-7

**最终目的**：按 [p9-review.md](../docs/review/p9-review.md) §3/§5 收口 `mcp-client` 的冗余与过度设计——删除单变体枚举与 1:1 镜像策略类型、合并双 `RestartPolicy`、收敛重复的 `is_loopback_url` 与 URL 校验、合并文件碎片，同时完整保留全部可观测语义、脱敏不变量与 adapter 五道门禁逻辑（§4.1 取舍决策显式延后）。净减约 +315/−376 行（reviewer 实测 diff：7 文件）。

**涉及范围**：`crates/mcp-client` 仅（config.rs、capabilities.rs、manager.rs、transport.rs、lib.rs；error.rs + session.rs 已删除）

## 细分步骤（分组）

### A. 类型去冗余

1. **A1（§3.6）删 `McpConfig::merge`**：删除 `McpConfig::merge` 与其唯一测试（config.rs）。目的：该方法 test-only、与 config-service 的递归合并语义冲突，属第二套合并语义。
2. **A2（§3.3）删 `SecretValue` 单变体 enum**：env/headers 改用 `BTreeMap<String, SecretRef>`（`SecretRef` 已是 Ser/De/Debug-safe 的纯 locator）。目的：消除单变体枚举，脱敏不变量不变。
3. **A3（§3.2）合并双 `RestartPolicy`**：收敛为单一可序列化 `config::RestartPolicy { max_attempts, base_delay_ms, max_delay_ms }`，去掉 +1/×16 魔法换算（runtime_options 直接 clone）；manager reconnect 循环以 `max_attempts` 为界、backoff 用 `Duration::from_millis`、reset 窗口 = `max_delay_ms * 4`，保留原 attempt-count 与 timing 语义；`Default { 1, 200, 10000 }` = 默认不重连（安全）。
4. **A4（§3.5）收敛 `is_loopback_url`**：收敛至 config.rs 一份 `pub(crate)`，删除 transport.rs 副本；`build_http_transport_config` 去掉重复的 scheme/userinfo/fragment 校验（config 解析已校验），保留 auth_token 空/conflict/loopback-HTTPS/header 有效性等 runtime guard；URL 拒绝测试迁移到 config。
5. **A5（§3.4）删 `McpInvocationPolicy`**：该类型与 `McpPermissions` 1:1 镜像 + trusted bool + 2 个转换器；adapter 改存 `permissions: McpPermissions` + `trusted: bool`；execute() 五道门禁（workspace allowlist / tool allowlist / PolicyEngine.decide / McpApproval / output cap）的逻辑、顺序、消息不变——§4.1 取舍推迟。

### B. 文件合并

6. **B（§3.8）error.rs + session.rs 并入 lib.rs**：无逻辑变更并入，删除两文件，修正 manager.rs 导入为 `crate::{McpError, McpPeer, McpServerCapabilities}`。目的：去文件碎片化。

### C. 文档标记

7. **C（§3.1）DEFERRED-CONSUMER 标记**：给 `list_resources` / `list_resource_templates` / `list_prompts` / `read_resource` / `get_prompt` 及 discover 的 resources/prompts 分支加 DEFERRED-CONSUMER 文档标记（仅 tools 被适配；resources/prompts 读取在 P15/P19 接入时决策）；`McpServerCapabilities::default()` 全 true 标注为 test-peer 便利。目的：避免死 API 误导。

## 主要产出物

- config.rs：`McpConfig::merge` 删除、`SecretValue` → `SecretRef` 化、单一 `config::RestartPolicy`、`is_loopback_url` 单一 `pub(crate)`
- capabilities.rs：`McpInvocationPolicy` 删除，adapter 改存 `McpPermissions` + `trusted: bool`，五道门禁不变
- lib.rs：error.rs/session.rs 内容并入、DEFERRED-CONSUMER 标记
- error.rs / session.rs 删除；manager.rs 导入修正

## 验收标准（保留 REVIEW 追踪编号）

- [x] **§3.6**：`McpConfig::merge` 与其测试已删
- [x] **§3.3**：`SecretValue` 已删，env/headers 用 `SecretRef`，脱敏不变量保留，无悬挂引用
- [x] **§3.2**：单一 `config::RestartPolicy`，无 +1/×16；manager reconnect 语义（attempt 界、backoff、reset 窗口、成功复位）保留；默认 `max_attempts=1` = 不重连；三边界校验 + 回归测试
- [x] **§3.5**：`is_loopback_url` 单一 `pub(crate)`；transport 去重；runtime guard 保留；URL 拒绝测试迁移到 config
- [x] **§3.4**：`McpInvocationPolicy` 已删，adapter 用 `McpPermissions` + `trusted`；五道门禁不变（§4.1 推迟）
- [x] **§3.8**：error.rs + session.rs 并入 lib.rs，两文件删除，导入修正
- [x] **§3.1**：DEFERRED-CONSUMER 标记到位
- [x] **§4.1 / §4.3 / §4.4 / §3.7 / §4.2**：未越界实现（显式延后，见下）
- [x] **定向验证**：`cargo test -p mcp-client`（48 passed）、clippy `--all-targets -D warnings`、fmt `--check` 全绿

### Deferred items（建议/跟踪，本任务不做）

- **§4.1** adapter 门禁 vs 调度器门禁取舍（P0，接入时与 P15-1 Canonical Tool v2 + ADR 协同）
- **§4.3** stdio 进程纳入 Sandbox/Process Runtime（接入时，P9-1 plan 已记）
- **§4.4** `McpPeer` canonical DTO（接入前评估）
- **§3.1** Resources/Prompts 半套 API 接入前明确 deferred-consumer 或下线（已加标记）
- **§3.7** 输出截断上移 tool-runtime（与 P15 协同）
- **§4.2** OAuth 双 bearer 解析合并（P3，零风险小优化，接入时可顺手）

## 验证记录（2026-08-10）

- `cargo test -p mcp-client`：48 passed / 0 failed（含迁移后的 restart-validation、URL-rejection、改名后的 secret 解析测试）。
- `cargo clippy -p mcp-client --all-targets -- -D warnings`：通过。
- `cargo fmt -p mcp-client -- --check`：通过。
- 按本任务门禁节奏只执行受影响 crate 的定向门禁；workspace 全量、三平台与发布门禁留待 Core 主干 L2/L3。
- reviewer 观察记录（非缺陷）：默认不重连配置的 cooldown 由 64s（旧 delay_ms×16）变为 40s（新 max_delay_ms 10s×4），单次 attempt-per-window 语义不变；旧 `{enabled, max_restarts, delay_ms}` JSON 键被 serde 静默忽略（按 `max_attempts=1` 解析），零消费者下可接受。

**相关文档**：[REVIEW.md](../REVIEW.md) §P9 · [p9-review.md](../docs/review/p9-review.md) · [mcp](../docs/features/mcp.md) · [ADR-011](../docs/adr/ADR-011-mcp-first-extension.md) · [ROADMAP 依赖选型基线](../ROADMAP.md#依赖选型基线)

> 跨任务协调（2026-08-10 review）：不与任何在研任务写集合重叠——写入集仅 `crates/mcp-client/src/`；resource-loader/context-engine/REVIEW.md/ROADMAP.md 的并行未提交改动属 P8 任务，未触碰。
