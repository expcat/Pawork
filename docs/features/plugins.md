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
- [ROADMAP Phase 16–17（Monitor / Plugin Package / Hooks）](../../ROADMAP.md)
