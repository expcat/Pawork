# P19-11：Resources / MCP / Plugins / Diagnostics

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-2、P19-3、Phase 8、Phase 9、Phase 10、P17-1～P17-4

**最终目的**：让用户看清实际生效的 AGENTS/Skills/Prompt/Profile、MCP/Plugin/Hook/LSP/Marketplace 状态和错误，并通过受控命令管理扩展，而不是让 GUI 直接扫描配置或执行插件。

**涉及范围**：Resource Inspector、Extensions/Health pages、capability/approval UI、Diagnostics viewer/export

## 细分步骤

1. **Resource Diagnostics** —— 展示 source/precedence/hash/status/conflict/reload，支持按 workspace/path/profile 过滤。目的：确定性上下文可解释。
2. **Skills/Prompts/Profiles** —— list/preview/activate/deactivate/test 走 Core API，显示依赖和冲突。目的：不由 GUI 重算合并。
3. **MCP** —— server config、transport、capabilities、health/restart/cancel/log/approval 与 output limit。目的：故障隔离可操作。
4. **Plugin/Marketplace/Hook/LSP** —— install/update/remove/trust/signature/capability/version/handler/diagnostic 状态。目的：扩展供应链和权限可见。
5. **Diagnostics** —— metrics/log/health 只展示脱敏 projection；bundle 导出走 Core Artifact/受控 save dialog。目的：不直接读日志目录。
6. **热重载反馈** —— debounce、reload revision、失败保留旧版本与恢复动作可见。目的：配置变更不造成静默漂移。
7. **安全/故障测试** —— 恶意 manifest、失效签名、崩溃 server、Secret in log、超大 schema/output。目的：管理面不扩权。

## 主要产出物

- Resource Inspector 与 Skills/Prompts/Profile controls
- MCP/Plugin/Hook/LSP/Marketplace management and health UI
- 脱敏 Diagnostics/Bundle flow 与故障 fixtures

## 验收标准

- [ ] 生效来源/优先级/conflict/reload revision 可追溯，GUI 不复制合并规则
- [ ] 扩展安装/启停/重启/权限变更都经 Policy/AppCommand
- [ ] 签名、capability、trust 与执行 owner/isolation level 可见
- [ ] Diagnostics/Bundle 不含 Secret，导出不直接授予 renderer 文件系统 scope
- [ ] 故障扩展不阻塞其他页面或 Agent stream
- [ ] Marketplace 真实 consumer（P17-3 延期落点）：source/install/update/uninstall、签名/trust/team policy 状态与审批经 AppCommand 呈现，卸载先停 package-owned Monitor，不绕过 Policy
- [ ] Profile v2 真实 consumer（P17-5 延期落点）：prompt/model/effort/tools(denied)/skills/MCP/permissions/hooks/memory/isolation 可见可校验，refs 解析失败与 isolation fail-closed 状态可解释
- [ ] Compat 真实 consumer（P17-13 延期落点）：五类来源只读探测、dry-run 预览与 Imported/Disabled/Unsupported/Conflict 诊断可见，显式应用与 export_plan 不执行外部 hook/MCP

**相关文档**：[skills](../docs/features/skills.md) · [workspace-index](../docs/features/workspace-index.md) · [mcp](../docs/features/mcp.md) · [plugins](../docs/features/plugins.md) · [observability](../docs/features/observability.md) · [Desktop GUI](../docs/features/desktop-gui.md)
