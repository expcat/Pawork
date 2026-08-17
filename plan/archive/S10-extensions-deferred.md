# 归档：扩展生态（原 S10，已移出排期）

> **2026-08-16 起不再是执行阶段。** WASM 插件 / 市场 / 用户 Hooks / LSP 整族待设计与决策，登记于 [../../ROADMAP.md](../../ROADMAP.md) §4。本文保留原实现草案，供日后纳入排期时引用。必要预留接口（`PluginId`、`ExternalPlugin`、`pawork-api` `plugin` feature 门、GUI 未知 capability）仍按现行计划保留，但不在 S0–S12 激活实现包。
>
> 原依赖：S9（mcp/resources 在位）、S10（app 注册入口正式化）· 规模：大

## 目标（本阶段结束时用户能做什么）

第三方扩展能力落地：WASM 插件「安装 → 注册 → 调用 → 撤销」全闭环（Ed25519+blake3 验签、市场源可选）；用户 Hooks（pre/post-tool、turn 边界）在宿主进程内注入执行并可短路；LSP 客户端把语义查询（定义/引用/诊断）作为工具提供给 Agent。全部包合入即接线，不产生零消费者库存。

## 涉及包与 V1 资产

| V2 包（目录） | 本阶段动作 | V1 来源与方式 |
| --- | --- | --- |
| `pawork-plugin`（extensions/plugin） | 激活：plugin-package + marketplace（feature `market`）；**三处 Ed25519+blake3 验签收敛为本包单一 `signing` 模块**；market HTTP 走 `pawork-net`；安装/撤销路径操作经 `pawork-policy`，删除走精确字面路径 | [archive/M6](README.md) pawork-plugin 节 |
| `pawork-wasm-host`（extensions/wasm-host） | 激活：wasm-plugin-host + hook-runtime（降级为包内 `lifecycle` 模块）；wasmtime 锁本包、默认构建路径不含；验签经 `pawork-plugin::signing` trait 注入 | [archive/M6](README.md) pawork-wasm-host 节 |
| `pawork-hooks`（extensions/hooks） | 激活：user-hooks 注入式执行器；与 wasm lifecycle 信任域分离（宿主进程内 vs 沙箱内），不同注册位点；hook 决策事件化入 envelope | [archive/M6](README.md) pawork-hooks 节 |
| `pawork-lsp`（extensions/lsp） | 激活：lsp-runtime（async LSP client）；resource 依赖注入指向 `pawork-resources` loader 抽象、sandbox 经 `pawork-exec` 注入；LSP 能力注册为工具 | [archive/M6](README.md) pawork-lsp 节 |
| `pawork-api` | 增强：`plugin` feature（V1 plugin-api 迁移，三 feature 集齐） | 直接迁移 |
| `pawork-cli` | 增强：`pawork plugin install/list/remove`、`pawork hooks list`、LSP server 配置向导（最小） | 新写 |

## 关键任务

1. **signing 单一化**：V1 三处重复验签（wasm-plugin-host/marketplace/plugin-package）收敛，golden 向量一致。
2. **插件闭环**：安装（验签→写盘→注册 ToolRegistry/Hook 链）→ 调用 → 撤销（注销→精确清理无残留）；不受信包拒装（fail-closed）。
3. **hooks 执行位点**：接 S3 已通电的审批/事件位点；pre-tool 返回值短路语义；hook 事件可重放。
4. **LSP 工具化**：与 rust-analyzer 握手、definition/references/diagnostics 三个查询工具；server 进程经 exec 沙箱启动。
5. **重依赖隔离**：wasmtime（27）与 rmcp（S9 已锁）不进默认构建路径（编译时间与 `cargo tree` 断言）。

## 真实测试与评估（冒烟清单）

- [ ] 示例 WASM 插件（提供一个自定义工具）：`pawork plugin install ./demo.pwpkg` → 对话中 Agent 调用该工具成功 → `remove` 后不可调用、文件无残留。
- [ ] 篡改签名的包安装被拒绝（验签 fail-closed）。
- [ ] `market` feature：从本地/测试 HTTP 源拉 manifest 安装（经 pawork-net）。
- [ ] 配置一个 pre-tool hook（如「阻止对 *.lock 文件的写入」）→ Agent 写 lock 文件被短路拦截且事件可见。
- [ ] 在本仓库（Rust）让 Agent 用 LSP 工具回答「`ModelProvider` 的定义与全部引用位置」并与 grep 结果对比正确性；**评估记录**：LSP 工具 vs 纯文本搜索的回答质量与耗时差异。

## 定向自动化测试

- `cargo test -p pawork-plugin`：签名/验签 golden 向量、安装/撤销闭环、market manifest 拉取（本地 mock 源）、路径操作经 policy。
- `cargo test -p pawork-wasm-host`：加载→调用→卸载、lifecycle 时序、wasmtime 不在 default（feature 断言）。
- `cargo test -p pawork-hooks`：短路语义、事件重放、与 wasm lifecycle 信任域隔离断言。
- `cargo test -p pawork-lsp`：握手 + 查询 e2e（rust-analyzer 可得时；否则 mock server）、注入边界断言（无具体实现硬依赖）。

## 退出标准

- [ ] 冒烟全项通过；四包全部有装配链真实调用点（无零消费者）。
- [ ] 验签收敛单一模块、golden 一致；未签名/篡改包 fail-closed。
- [ ] wasmtime/rmcp 默认构建路径不含；`pawork-api` 三 feature 集齐。
- [ ] hook 与 wasm lifecycle 信任域分离断言通过。

## 为后续阶段预留 / 明确不做

- 预留：插件可声明的能力面（工具之外的 hook/资源源）按 plugin-api 契约在位；市场正式源待运营决策。
- 不做：`tool_search`（冻结候审，工具目录规模未达标）、browser/computer 驱动（冻结候审）。

## 并行拆分建议

- 波 A（并行 ×2）：`pawork-plugin`（signing 先行定 trait）+ `pawork-api` plugin feature；`pawork-lsp`。
- 波 B（并行 ×2，依赖 signing trait）：`pawork-wasm-host`；`pawork-hooks`。
- 波 C（串行）：app 注册位点接线（单一 owner）+ cli + 冒烟。

## 参考

- [../docs/design.md](../../docs/design.md) §4（本阶段功能设计与参照项目映射）· [../docs/references.md](../../docs/references.md)（参照项目手册）
- [archive/M6-extensions.md](README.md)（本阶段主文档：四包迁移细则与接线位点）
