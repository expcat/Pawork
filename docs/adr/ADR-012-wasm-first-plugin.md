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

## 相关

- [plugins](../features/plugins.md) · [ADR-011 MCP 第一](ADR-011-mcp-first-extension.md) · [ADR-013 无 native dylib](ADR-013-no-native-dylib-plugin.md)
