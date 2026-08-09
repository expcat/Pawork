# Plugin 系统（WASM）

## 职责

在不引入 Node / V8 / TypeScript Extension Host 的前提下提供代码级插件能力，采用 capability-based 的 WASM Component 设计。

## 插件路线

不使用：Rust `dylib` 插件作为公开扩展；嵌入 Node；嵌入 V8；TypeScript Extension Host。

使用：

1. 声明式 Skills
2. MCP
3. WASM Component Plugins
4. 外部受控进程工具

## 插件能力

注册工具；注册命令；接收生命周期事件；修改上下文；提供 Compaction Strategy；注册 Provider；保存插件状态；请求用户交互；访问受限文件；访问受限网络；声明 Monitor（监视循环）。

## Plugin Package（Phase 17 / P17-2）

一个可安装包（manifest + 归档）可聚合六种扩展类型：Skills、Agents（profile）、Hooks（[用户钩子](../../plan/P17-1-user-hooks.md)）、MCP server 声明、LSP server 声明、**Monitors**。Package 只做聚合 / 校验 / 作用域绑定，复用各类型既有子 manifest，不重定义运行时语义。其中 Monitors 复用 [P16-6](../../plan/P16-6-persistent-process-monitor.md) 运行时——package manifest 只声明 monitor 配置 / trigger / permissions / lifecycle / required capability，实际执行统一进入 `monitor-service` / `task-manager`，插件不自带运行时。

Marketplace 安装 / 更新 / 卸载必须把六类资源作为一个事务化 package 生命周期处理：校验完成后再分别注册到 Skills / Agents / User Hooks / MCP / LSP / Monitor loader，失败则整体回滚；卸载逐类注销并停止该 package 拥有的 Monitor。Marketplace 只管理包与注册，不执行 Hook、MCP、LSP 或 Monitor。

> 边界：WASM lifecycle hook（进程内、沙箱化、capability 门控，P10-3）与 User Hook（用户配置驱动外部桥接，P17-1）是不同 trust boundary，二者共享 trigger point 词汇但走独立 dispatcher，不重复执行。

## Plugin Manifest

```toml
id = "example.plugin"
name = "Example"
version = "1.2.0"
api_version = ">=1,<2"
capabilities = ["register_tool", "lifecycle_hook", "persistent_state"]
tool_capabilities = ["read_only"]
lifecycle_hooks = ["workspace_open", "run_start", "run_end"]

[permissions]
filesystem_read = ["workspace"]
filesystem_write = []
network = ["api.example.com"]
process = false
secret_refs = ["example-token"]

[[tools]]
name = "lookup"
description = "查询插件索引"
input_schema = { type = "object" }
default_timeout_ms = 5000
max_output_bytes = 65536
```

`PluginManifest` 是被签名的能力与注册事实源。工具、命令、hook 与 persistent state
必须同时有对应 `PluginCapability`；文件权限只接受 workspace-relative scope，网络权限只接受
host allowlist，`secret_refs` 只保存引用名。重复声明、绝对路径、`..`、URL 形式 host、保留
`ExternalPlugin` capability 或无对应 capability 的注册都会在编译组件前被拒绝。

## 签名与信任

发布物由 `SignedPluginManifest + WASM Component bytes` 组成。Ed25519 签名消息为带域分离前缀的
canonical manifest JSON 与组件 BLAKE3 摘要，因此替换 manifest 或 component 任一侧都会导致验签失败。
签名仅携带 opaque `key_id` 和 Base64 signature；公钥来自宿主 trust store，不允许插件自带公钥建立
信任。未知 key、错误算法、畸形签名、未签名组件或不兼容 API 一律 fail closed。

## Component ABI v1

当前宿主 API 版本为 `1.0.0`，组件必须提供固定的顶层导出：

```wit
package pawork:plugin@1.0.0;

world plugin {
  export invoke: func(request: string) -> string;
}
```

版本化事实源位于 [`schemas/plugin-api/pawork-plugin-v1.wit`](../../schemas/plugin-api/pawork-plugin-v1.wit)。
CI 会用 `wit-bindgen` 编译 guest binding，并校验冻结的 WIT 文本与 v1 JSON invocation/output golden；
未同步升级 API major 的字段或 ABI 漂移会直接失败。

输入是 `PluginInvocation` JSON（tool / command / lifecycle 三种 operation、调用上下文与当前 state
snapshot），输出是 `PluginInvocationOutput` JSON（success/error、result 与 state mutations）。宿主在边界
校验 UTF-8、JSON、plugin id、API version、输出长度与 state revision；插件错误、trap 与畸形输出只终止
本次插件调用，不终止 Core。

同 ID load 的校验/编译/实例化/登记全路径串行，避免双重加载竞态。`unload` 会先从宿主登记移除并
阻止新调用，再等待旧实例完整的 snapshot→invoke→state apply 事务结束并释放 Store；load/unload 共用
串行锁，因此同 ID 新实例不能抢在旧事务结束前注册。其他调用方仍持有的旧 `LoadedPlugin` handle
只能得到 `NotLoaded`，不能继续执行。

宿主不链接 WASI，也不为组件注入文件、网络或进程 import；因此 manifest 声明只表示可被宿主策略
考虑的上限，不会自动获得能力。未来增加 host import 时仍必须同时通过 manifest、Policy 与 capability
检查。

## 注册与派发

- `PluginRuntime` 是 P10 子系统的统一入口：一次 mutation lock 内完成验签/load、工具/命令/hook
  发布和失败回滚；unload 会注销 hook、停用 component，并完整撤销工具与命令。注册集合只在
  lifecycle stopped 时可变，确保 Start/Stop 与同一插件快照对应。
- 工具和命令使用 `<plugin_id>::<local_name>` 命名；重复名或覆盖已有注册会被拒绝。
- 插件工具进入 canonical `ToolRegistry` 时一律标记为 `ExternalPlugin`、不可在未信任 workspace 自动运行，
  插件自报 `read_only` 不能降低审批与串行级别。
- lifecycle hook 只接收 manifest 明确订阅的事件；派发按 plugin id 确定性排序。单插件失败/取消记录为
  可序列化 outcome 并继续隔离其他插件，调用方可将 report 持久化为 Core 事件。
- P10 的 lifecycle hook 是进程内 WASM dispatcher；P17-1 User Hook 是用户配置驱动的外部 dispatcher。
  二者共享 trigger point 词汇但不互相调用，也不重复派发。

P10 的交付边界是可组合的插件运行时 crate；把它注入 `app-service` / `core-runtime` 并由唯一正式
`pawork` 宿主管理进程生命周期，属于 P13-1/P13-2。该后续接线不改变 P10 的签名、注册与沙箱契约。

## Plugin state

状态通过宿主注入的 `PluginStateStore` 保存，key 由 `plugin_id + scope` 隔离；scope 按
session → workspace → global 选择。每次调用读取带 revision 的 snapshot，只在组件成功返回后原子应用
mutation；同插件的 snapshot→invoke→apply 整体串行，陈旧 revision、跨插件访问、非法 key、单值/总量
超限均被拒绝。默认内存 backend 用于宿主与
测试闭环，正式组合层可注入 durable backend 而不让 WASM Host 反向依赖 SQLite。

没有 `PersistentState` capability 的插件只能看到空 snapshot，任何 mutation 都会被拒绝。宿主不会解析
`secret_refs` 或把 credential 明文注入插件；P10 默认 backend 只在内存保存，未接 SQLite。未来 durable
adapter 必须继续经过 Secret/Policy 边界，plugin state 不能被当作凭证存储。

## 生命周期事件

```text
core_start / workspace_open / session_create / session_open
run_start / context_build / provider_request / assistant_delta
tool_call / tool_result / compaction / run_end / session_close / core_shutdown
```

## 插件安全

每个插件使用独立 Wasmtime Store，加载与调用均受组件字节、输入/输出、线性内存、Fuel 和 wall-clock
时间限制；取消与超时通过 epoch interruption 中止正在运行的组件。宿主不暴露 WASI/文件/网络/进程，
Secret 使用引用而不是明文；所有 trap、Fuel 耗尽、内存增长失败、超时和畸形 ABI 都映射为结构化
`PluginError`。

API 兼容由 manifest 的 semver `VersionReq` 与 host API version 共同判定：范围包含当前版本才允许
加载；minor 兼容由声明范围显式表达，跨 major 默认拒绝。兼容矩阵、冻结 JSON golden 与 WIT guest
binding 编译共同作为 contract，由 CI 的 workspace test 三平台执行。

## 优先级（P0–P2）

- **P0**：Phase 10 的签名 manifest、Component load/unload、工具/命令、lifecycle hook、隔离状态、
  capability 与资源预算、API v1 contract。
- **P1**：Phase 17 的 Plugin Package / Marketplace、团队 trust policy、安装更新回滚与 Monitor 聚合。
- **P2**：WASM Provider、Compaction Strategy、Context 修改与复杂用户交互等高级扩展；不得绕过
  canonical domain 或 Policy。

## 验收标准

- 恶意或崩溃插件不能终止 Core
- 插件不能越权访问文件、网络、进程和 Secret
- manifest 或 component 被篡改时验签失败
- 状态跨调用保存且插件/scope 隔离、配额生效
- 工具/命令注册与 lifecycle hook 派发可验证
- load→注册→派发→unload 可由统一运行时完整协调并撤销
- Plugin API 具有 semver、JSON golden 与 WIT binding 兼容 contract 测试

## 相关文档

- [mcp](mcp.md) · [policy](policy.md) · [sandbox](sandbox.md)
- [ADR-012 WASM 第一插件](../adr/ADR-012-wasm-first-plugin.md) · [ADR-013 不公开 Native dylib](../adr/ADR-013-no-native-dylib-plugin.md)
- [ROADMAP Phase 10](../../ROADMAP.md)
- [ROADMAP Phase 16–17（Monitor / Plugin Package / Hooks）](../../ROADMAP.md)
