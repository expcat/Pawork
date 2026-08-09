# 身份认证与 Secret 管理

## 职责

统一管理各 Provider 与扩展（MCP、插件）的认证方式与 Secret 存储，保证明文 Token 不落库、不进日志。

`auth-service` 管理“凭据如何存取、刷新与撤销”；`ProviderAccount`、账号健康、并发 lease、轮换与 affinity 由 [Provider Account Control Plane](provider-control-plane.md) 管理。一个 account 可关联多个 credential，但 account state 不能与 credential secret state 混为一谈。

## 支持的认证方式

API Key；Bearer Token；OAuth 2.0 PKCE；OAuth Device Flow；自动 Refresh Token；AWS Profile；AWS Environment Credentials；GCP Application Default Credentials；Azure Credential Chain；自定义 Header；环境变量引用。

OAuth 实现保留最小手写协议层：使用 `base64` / `rand` / `sha2` / `url` 提供安全原语，不引入整套 `oauth2` SDK。PKCE verifier 由 48 字节均匀随机数做 base64url 无 padding 编码；callback 仅绑定 loopback，在生成授权 URL 前回填实际监听端口，并以 64 KiB 上限完整读取分片请求头。回调页只返回固定 `text/plain` 文案，不回显授权端 query；TokenSet、PKCE 与 Device Flow 临时 secret 的 `Debug` 输出统一脱敏。

组合层在 Provider 构造或每次请求前调用 `resolve_oauth_credential_for_request`：临近过期时自动刷新，先将新 access token 与可选轮换 refresh token 回写 Secret Backend，再短暂解析为 bearer credential。同一 credential 的并发请求使用 singleflight gate，共享一次刷新结果；成功响应缺少 `expires_in` 时保留既有过期时间，避免误判为永不过期。

## Secret 存储

存储优先级：

1. OS Keychain
2. 用户指定 Secret Backend
3. 环境变量
4. 加密配置文件
5. 临时 Session Credential

SQLite 只能保存：Tenant / Account / Credential ID；Provider；显示名称；过期时间；Scope；Secret/Keychain reference；刷新状态；脱敏状态。**不能保存明文 Token。** `provider-control` 只拿 `secret_ref`；明文 `ResolvedCredential` 仅在受信调用边界短暂存在，不进入 lease/event/audit。

## Auth 状态机

```text
NotConfigured
Configured
Refreshing
Valid
Expired
Revoked
Error
```

该状态机描述 credential 本身。`ProviderAccount` 的 Active/CoolingDown/BillingBlocked/Disabled 与 `CredentialLease` 的 Requested/Acquired/Released/Expired/Reclaimed 是独立状态机。

## 对 GUI 暴露

开始登录；返回授权 URL；接收 OAuth callback；取消登录；删除 Credential；测试 Credential；显示脱敏状态；刷新 Credential。

## 验收标准

- Secret 不写入数据库与日志
- OAuth callback 可接收与校验
- 脱敏状态正确显示
- legacy 单 credential 可无损映射为 synthetic default account，且默认行为不变

## 相关文档

- [providers](providers.md) · [provider-control-plane](provider-control-plane.md) · [tenant-audit](tenant-audit.md) · [observability（脱敏）](observability.md)
- [ADR-014 Secret 存 OS Keychain](../adr/ADR-014-secret-os-keychain.md)
- [ADR-033 控制面分离](../adr/ADR-033-control-plane-separation.md) · [ROADMAP P2-6 / P6-4 / Phase 18](../../ROADMAP.md)
