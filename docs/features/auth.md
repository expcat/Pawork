# 身份认证与 Secret 管理

## 职责

统一管理各 Provider 与扩展（MCP、插件）的认证方式与 Secret 存储，保证明文 Token 不落库、不进日志。

## 支持的认证方式

API Key；Bearer Token；OAuth 2.0 PKCE；OAuth Device Flow；自动 Refresh Token；AWS Profile；AWS Environment Credentials；GCP Application Default Credentials；Azure Credential Chain；自定义 Header；环境变量引用。

## Secret 存储

存储优先级：

1. OS Keychain
2. 用户指定 Secret Backend
3. 环境变量
4. 加密配置文件
5. 临时 Session Credential

SQLite 只能保存：Credential ID；Provider；显示名称；过期时间；Scope；Keychain reference；脱敏状态。**不能保存明文 Token。**

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

## 对 GUI 暴露

开始登录；返回授权 URL；接收 OAuth callback；取消登录；删除 Credential；测试 Credential；显示脱敏状态；刷新 Credential。

## 验收标准

- Secret 不写入数据库与日志
- OAuth callback 可接收与校验
- 脱敏状态正确显示

## 相关文档

- [providers](providers.md) · [observability（脱敏）](observability.md)
- [ADR-014 Secret 存 OS Keychain](../adr/ADR-014-secret-os-keychain.md)
- [ROADMAP P2-6 / P6-4](../../ROADMAP.md)
