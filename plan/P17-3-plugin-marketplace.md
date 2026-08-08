# P17-3：Plugin Marketplace（扩展市场）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P17-2、P2-1、P9-5、P4-9、P8-1

**最终目的**：实现 Plugin Marketplace——从可信源（source）发现、安装、更新、卸载 Plugin Package，并提供版本管理、签名校验、版本 pin、trust 等级与 team policy 控制，让组织可控地引入第三方扩展。所有越权 / 越级安装须可被组织策略拦截。

**涉及范围**：新增 `marketplace`；复用 `plugin-package`（[P17-2](P17-2-plugin-package-format.md)）、`http-runtime`（源拉取）、`policy-engine`（trust / team policy）、`resource-loader`（安装落位）

## 细分步骤

1. **Source 模型与发现** —— 目的：定义 source（registry URL / git / local path），从 source 索引列发现可装 package 及版本清单，复用 `http-runtime` 拉取，支持多 source 聚合。
2. **安装 / 更新 / 卸载** —— 目的：install 拉取归档→校验→解包到 workspace / global→事务化注册 Skills / Agents / Hooks / MCP / LSP / Monitors 六类资源；update 对六类资源做版本切换并在失败时整体回滚；uninstall 先停止 package-owned Monitor，再从各自 loader 注销并清理资源。全程受 `resource-loader` 作用域约束，Marketplace 本身不执行任何子资源。
3. **版本与 pin** —— 目的：版本语义（semver）、pin（锁定具体版本 / 哈希）、依赖解析与版本范围，pin 记录写入 workspace 配置可重放。
4. **签名与校验** —— 目的：package 与 source 支持签名（公钥 / 签名清单），安装时校验签名与内容 hash（基于 [P17-2](P17-2-plugin-package-format.md) 完整性清单），校验失败拒绝安装。
5. **Trust 等级** —— 目的：source / package 的 trust 等级（trusted / verified / untrusted），不同等级需不同审批（与 [P9-5](P9-5-mcp-approval.md) MCP approval、`policy-engine` 协作），untrusted 默认拒装或需显式确认。
6. **Team policy** —— 目的：组织 / team 维度的安装策略（允许 / 禁止 source、允许的 trust 等级、强制签名、版本白名单），策略在 `policy-engine` 统一评估，优先于用户个人选择。
7. **定向 / Mock 测试** —— 目的：安装 / 更新 / 卸载六类资源的注册/注销与失败回滚、Monitor 停止、签名失败拒绝、pin 生效、team policy 拦截越权安装、多 source 聚合。仅定向 + Mock（用 mock registry）。

## 主要产出物

- `marketplace`：source / discovery / install / update / uninstall / sign / trust / team-policy
- 定向 + mock registry 测试

## 验收标准

- [ ] 支持多 source 发现与安装 / 更新 / 卸载
- [ ] 签名与内容 hash 校验失败时拒绝安装
- [ ] 支持 version pin 与版本范围解析
- [ ] trust 等级与 team policy 生效，越权安装被拦截
- [ ] 六类 package 资源随安装/更新/卸载完整注册、回滚、注销；Monitors 只由 P16-6 runtime 执行

**相关文档**：[plugins](../docs/features/plugins.md) · [policy](../docs/features/policy.md) · [skills](../docs/features/skills.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖（签名走标准库 / 既有实现，semver 视仓库已有）；复用 `plugin-package` / `http-runtime` / `policy-engine` / `resource-loader`。新 crate `marketplace` 依赖方向：`plugin-package → marketplace → app-service`。
