# P18-17：Production Provider Composition

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P18-3、P18-4、P18-14、P15-2～P15-5

**最终目的**：让正式 `pawork` 宿主真正消费持久 ProviderAccount / Credential metadata，经 `BackendCredentialResolver` 与 `ProviderFactory` 组合真实 Provider，并通过 `CoreRuntime::register_provider` 与共享 model registry 暴露同一份 provider/model 能力事实源；禁止用未消费的 resolver 变量、Mock Provider 或硬编码 catalog 冒充接线。

**涉及范围**：`apps/pawork` composition、`core-runtime`、`provider-control` repository/factory/registry、`provider-runtime`、内置 Provider crates、`model-registry`、`auth-service`、持久 control-plane repository。

## 细分步骤

1. **持久 repository 命令面** —— account / credential 的 create/disable/delete/test 事务写入 SQLite，重启回读与 tenant 隔离可证。目的：启动 hydration 不成为只读死端，管理动作不在重启后丢失。
2. **Secret 解析与 Provider compose** —— 宿主把 `BackendCredentialResolver` 交给 `ProviderFactory`，只按 `secret_ref` 短时解析；明文不进 DB、日志或长期对象。目的：账号 metadata 与执行凭据闭环但不扩散 Secret。
3. **真实 Provider 注册** —— 为配置存在且可解析的内置 Provider 构造真实 `ModelProvider`，经 `CoreRuntime::register_provider` 注册；不可解析配置 fail-closed 并产生脱敏诊断。目的：成功 Run 不再依赖测试注入。
4. **统一模型 catalog** —— `ProviderFactory::descriptors().builtin_models()` 合并到共享 `ModelRegistry`，能力、上下文与定价不再由 route/GUI 硬编码。目的：Provider、路由与客户端消费同一 canonical catalog。

## 主要产出物

- SQLite-backed ProviderAccount / Credential mutation path 与 restart tests
- 正式宿主 resolver → factory → provider registry 装配
- provider descriptors → model-registry 合并与冲突/缺失配置诊断
- Secret redaction 与跨 tenant 负向回归

## 验收标准

- [ ] account / credential 管理写入跨重启持久，坏行、未知枚举与跨 tenant 操作 fail-closed
- [ ] `BackendCredentialResolver` 被真实 `ProviderFactory` 消费，不存在仅构造未使用的占位变量
- [ ] 至少一个真实内置 Provider 由正式 `pawork` composition 注册并完成非 Mock 的最小 Run
- [ ] Provider 配置缺失/Secret 不可解析时失败信息不含明文且不回退测试 Provider
- [ ] `builtin_models()` 进入共享 model registry，Provider/route/client 查询返回同一 catalog

**相关文档**：[P18 Review](../docs/review/p18-review.md) · [provider-control-plane](../docs/features/provider-control-plane.md) · [models](../docs/features/models.md) · [ADR-014](../docs/adr/ADR-014-secret-os-keychain.md) · [ADR-025](../docs/adr/ADR-025-cli-is-sole-host.md)
