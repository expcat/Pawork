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

注册工具；注册命令；接收生命周期事件；修改上下文；提供 Compaction Strategy；注册 Provider；保存插件状态；请求用户交互；访问受限文件；访问受限网络。

## Plugin Manifest

```toml
id = "example.plugin"
version = "1.2.0"
core_api = ">=1,<2"

[capabilities]
filesystem_read = ["workspace"]
filesystem_write = []
network = ["api.example.com"]
process = false
secrets = ["example-token"]
```

## 生命周期事件

```text
core_start / workspace_open / session_create / session_open
run_start / context_build / provider_request / assistant_delta
tool_call / tool_result / compaction / run_end / session_close / core_shutdown
```

## 插件安全

内存限制；Fuel 限制；执行时间限制；能力检查；无默认文件访问；无默认网络；无默认进程；Secret 使用引用而不是明文；崩溃隔离；插件签名；插件 API 版本。

## 验收标准

- 恶意或崩溃插件不能终止 Core
- 插件不能越权访问文件、网络和 Secret
- Plugin API 具有版本兼容测试

## 相关文档

- [mcp](mcp.md) · [policy](policy.md) · [sandbox](sandbox.md)
- [ADR-012 WASM 第一插件](../adr/ADR-012-wasm-first-plugin.md) · [ADR-013 不公开 Native dylib](../adr/ADR-013-no-native-dylib-plugin.md)
- [ROADMAP Phase 10](../../ROADMAP.md)
