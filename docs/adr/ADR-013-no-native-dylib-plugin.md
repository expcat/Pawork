# ADR-013：不公开 Native dylib Plugin API

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

Native dylib 插件以进程内 native 代码扩展核心，破坏内存安全与崩溃隔离边界，且与平台 ABI 强耦合。

## 决策

不公开 Native dylib 插件作为扩展机制。代码级扩展统一走 WASM（[ADR-012](ADR-012-wasm-first-plugin.md)），外部扩展走 MCP（[ADR-011](ADR-011-mcp-first-extension.md)），声明式扩展走 Skills。

## 后果

- 核心进程内存安全边界得以保持，插件崩溃不致 Core 崩溃。
- 牺牲部分 native 性能与直接集成能力。
- 生态从零起步，但安全模型清晰。

## 相关

- [plugins](../features/plugins.md) · [ADR-001 纯 Rust](ADR-001-pure-rust-core.md) · [ADR-012 WASM](ADR-012-wasm-first-plugin.md)
