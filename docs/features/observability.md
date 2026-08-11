# 可观测性

## 职责

提供结构化日志、指标与诊断包，并在记录、序列化与导出边界执行防御性脱敏，默认降低 Secret 泄漏风险。自动脱敏属于 best-effort 防线，不替代源头禁止记录 Secret，也不构成「导出物绝对无敏感数据」的保证。

## 日志字段

```text
timestamp / level / component / tenant_id / principal_id / workspace_id
session_id / agent_id / run_id / provider / account_id / model
client_kind / tool_call_id / trace_id / duration / error_code
```

## 自动脱敏

API Key；Bearer Token；Cookie；OAuth Code；Authorization Header；用户配置的 Secret Pattern。

规则会持续以回归样本覆盖常见 URL query 参数、嵌套或转义 JSON、自定义敏感 Header 等形态；非标准键名、编码/分片后的值和业务自定义格式仍可能漏报。诊断包应始终按潜在敏感文件处理，分享前由用户人工检查内容与接收方。

控制面日志只记录 `account_id` / `credential_id` 等 opaque ID 与脱敏状态，不记录 `secret_ref` 的可解析内容、credential 明文、prompt/tool output 或 Protected Blob。Tenant-scoped Audit Event 与 OTel exporter 见 P18-13。

## Metrics

Core 初始化时间；数据库时间；Provider 首 Token；Provider 总时长；Tool 执行时长；Context Token；Compaction 次数；Session 打开时间；Diff 生成时间；文件索引时间；内存；活跃 Task；Channel backlog；Blob Store 大小。

## Provider 能力协商诊断（P15-8）

Provider 请求开始前，Agent Engine 发出可持久化、可重放的 `Diagnostic` 事件，稳定 code 为 `provider_capability_negotiated`。details 只包含 canonical requested / supported / unsupported 能力、选定 transport 与显式 fallback，不包含 Provider wire payload、凭据、Protected Blob、prompt 或 tool output。重试复用同一协商记录，避免一次 Run 内 transport 或 fallback 静默漂移。

该事件用于解释“为何选择现代 transport、为何降为 Client Tool、为何拒绝请求”，不是能力探测原始响应的转储。Provider 探测失败只记录归一化错误类别与安全摘要；诊断包沿用 allowlist 与递归脱敏规则。

## Diagnostics Bundle

用户可导出：核心版本；OS；Provider 状态；模型目录；数据库 schema；插件列表；MCP 状态；最近脱敏日志；性能指标；崩溃报告。

默认不包含：Secret；完整用户消息；文件内容；Tool Output（除非用户明确选择包含）。

导出成功只表示 allowlist、裁剪和已知脱敏规则已执行，不表示内容经过完备的 Secret 证明。CLI/GUI 的导出与分享流程必须提示「best-effort 脱敏、分享前人工确认」，不得自动上传或自动发送诊断包。

## Phase 1 实现状态

`diagnostics` 已提供 `tracing` Layer 与有界内存日志尾部，固定记录 component、Workspace / Session / Run、Provider / Model、Tool Call、trace、duration 与 error code 等字段。Redactor 对所有字符串和字段执行 Authorization、Bearer、API Key、Token、Cookie、OAuth、Password、JWT 与自定义模式脱敏；Warn / Error 不采样，低级别支持固定间隔采样。

Metrics Registry 已预注册初始化、数据库、Provider 首 Token / 总时长、Tool、Context Token、Compaction、Session、Diff、文件索引、内存、活跃 Task、Channel backlog 与 Blob 大小共 14 项指标，支持 counter、gauge、histogram、快照与计时器。

诊断包使用显式 allowlist 类型，只接收版本、OS、Provider / Model、schema、Plugin / MCP 状态、脱敏日志元数据、metrics 与崩溃摘要；其中日志会丢弃 `message` 与任意扩展 fields，默认类型中不存在用户消息、文件内容与 Tool Output 字段。导出会再次递归脱敏、按条数和字节预算裁剪，并以 `create_new` 独占创建最终离线 JSON 后执行 fsync；并发写入同一路径时只有首个创建者成功，已有文件不会被覆盖。

## 验收标准

- 日志与诊断包默认不含明文 Secret
- URL query、嵌套 JSON 转义和自定义 Header 具备持续回归样本；新增漏报形态须先补测试再扩展规则
- 导出/分享界面明确标注 best-effort，并要求用户在分享前人工确认
- 关键指标可采集
- 诊断包可离线导出
- route/lease/rebind/policy/agent/client 关键决策可按 tenant/session/agent/provider/account/trace 关联且不跨 tenant 查询

## 相关文档

- [auth（脱敏状态）](auth.md) · [tenant-audit](tenant-audit.md) · [provider-control-plane](provider-control-plane.md) · [CLI Host（doctor）](cli-host.md)
- [ROADMAP P1-9 / P1-10 / P1-11](../../ROADMAP.md)
