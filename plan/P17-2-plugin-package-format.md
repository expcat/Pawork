# P17-2：Plugin Package Format（扩展包格式）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P10-1、P8-3、P9-6、P17-1、P17-4、P8-5

**最终目的**：定义统一的 Plugin Package 格式——一个可安装包（manifest + 归档）可聚合多种扩展类型：Skills、Agents（profile）、Hooks（用户钩子）、MCP server 声明、LSP server 声明、Monitors（监视器声明）。让一次安装即可交付一个完整能力组合，避免用户手动逐项配置。Package 仅做**聚合、校验、作用域绑定**，复用各类型既有的子 manifest，不重定义其语义；Monitor 复用 [P16-6](P16-6-persistent-process-monitor.md) 运行时语义，Package manifest 只声明其配置/trigger/permissions/lifecycle/required capability，实际执行统一进入 `monitor-service` / `task-manager`。

**涉及范围**：新增 `plugin-package`；复用 `resource-loader`（子资源加载）、`agent-domain`（类型）

## 细分步骤

1. **Package manifest schema** —— 目的：定义 package 清单（id / version / license / entrypoint），子段声明 skills / agents / hooks / mcp / lsp / monitors 各自的相对路径或内联清单，复用各子类型既有 schema 不重定义；monitors 子段只声明 monitor 配置 / trigger / permissions / lifecycle / required capability，不重新定义运行时语义。
2. **归档格式与完整性** —— 目的：定义打包归档（目录树 + manifest + 资源），支持内容寻址清单（hash 校验），为 [P17-3](P17-3-plugin-marketplace.md) marketplace 的签名 / 校验提供基础。
3. **类型聚合与冲突检测** —— 目的：加载时合并多种扩展类型到统一资源表，检测跨类型 / 跨包冲突（重名 skill、重复 hook trigger、MCP server 名冲突），冲突可解析或报错。
4. **作用域与依赖声明** —— 目的：声明 package 的依赖（其他 package / provider / runtime 约束）与 workspace / global 作用域，与 `resource-loader` 作用域一致。
5. **与各 loader 集成** —— 目的：解包后把子资源分发到对应 loader（skills→P8-3、hooks→P17-1、mcp→P9、agents→P8-5、lsp→P17-4、monitors→P16-6 `monitor-service` / `task-manager`），单一入口安装。
6. **定向 / Mock 测试** —— 目的：聚合包加载、冲突检测、hash 校验失败报错、子资源分发到正确 loader、作用域正确。仅定向 + Mock。

## 主要产出物

- `plugin-package`：manifest schema + 归档读写 + 内容 hash 校验 + 冲突检测
- 定向测试

## 验收标准

- [ ] 一个 Package 可包含 Skills / Agents / Hooks / MCP / LSP / Monitors 六类并一次安装
- [ ] Monitors 子段只声明配置/trigger/permissions/lifecycle/required capability，执行统一进入 `monitor-service` / `task-manager`（P16-6），不重定义运行时语义
- [ ] 归档带内容 hash 校验，损坏 / 篡改可检测
- [ ] 跨类型 / 跨包冲突可检测并报错
- [ ] 子资源正确分发到各 loader，作用域与 `resource-loader` 一致

**相关文档**：[plugins](../docs/features/plugins.md) · [skills](../docs/features/skills.md) · [P17-1 User Hooks](P17-1-user-hooks.md) · [mcp](../docs/features/mcp.md) · [P17-4 LSP Runtime](P17-4-lsp-runtime.md) · [P16-6 Monitor](P16-6-persistent-process-monitor.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；复用 `resource-loader` / `agent-domain`。新 crate `plugin-package` 依赖方向：`agent-domain → plugin-package → resource-loader`，被 [P17-3](P17-3-plugin-marketplace.md) marketplace 依赖。
