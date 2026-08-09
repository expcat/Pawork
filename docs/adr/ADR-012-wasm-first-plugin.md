# ADR-012：WASM 是第一代码插件机制

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

需要代码级插件（注册工具/命令、Hook、Compaction 策略、Provider），但不能引入 Node/V8/TS 或不安全的 native dylib。

## 决策

以 WASM Component 为第一代码插件机制，采用 capability-based 设计：内存、Fuel、执行时间限制，无默认文件/网络/进程，Secret 用引用而非明文，崩溃隔离，签名与 API 版本。

## 后果

- 代码插件跨平台、沙箱化、能力受限。
- WASM API 需保持小而稳定，并提供版本兼容测试。
- 早期插件生态有限，缓解见 [ROADMAP 风险](../../ROADMAP.md)。

## Phase 10 落地约束

- Plugin API v1 使用 `invoke(string) -> string` 的最小 Component ABI，payload 为 versioned canonical JSON；
  扩展字段通过协议版本演进，不把 Rust ABI 暴露给插件。
- Ed25519 签名同时绑定 canonical manifest 与 component BLAKE3 摘要，公钥只能来自宿主 trust store；
  未签名、未知 key、内容篡改与 API 不兼容均 fail closed。
- 每插件独立 Wasmtime Store；无 WASI、无默认 host import，并同时限制组件/输入/输出、Fuel、线性内存
  与 wall-clock 时间。插件 trap、超时和取消只形成结构化错误。
- 工具统一以 `ExternalPlugin` 接入 canonical Tool Runtime；lifecycle hook 由独立 `hook-runtime`
  确定性派发，P17-1 User Hook 走另一 trust boundary。
- `PluginRuntime` 在 lifecycle stopped 边界内原子协调 component load/unload 与工具、命令、hook 注册；
  unload 等待 snapshot→invoke→state apply 完整事务，并阻止同 ID 提前重载。
- 插件状态由宿主 backend 按 plugin/scope 隔离并执行 revision 与容量检查，WASM 组件不直接访问 SQLite。
- v1 WIT、生成的 guest binding 与 canonical JSON golden 共同构成兼容门禁；仅 semver range 匹配不足以
  证明 wire contract 未漂移。

## 相关

- [plugins](../features/plugins.md) · [ADR-011 MCP 第一](ADR-011-mcp-first-extension.md) · [ADR-013 无 native dylib](ADR-013-no-native-dylib-plugin.md)
