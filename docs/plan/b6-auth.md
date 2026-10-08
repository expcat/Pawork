# B6 · Auth 存储 API 与别名保留期裁决

> 状态：✅ 完成（独立 review PASS）。更新：2026-10-08。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决凭证 API、中转类型及旧 serde 别名的保留范围。

## 当前线索与读取范围

登记项为 `ApiKeyCredential::store/store_with_scopes/delete`、`StoredCredential::with_expires_at`、`DeviceAuthorization` 中转结构和 `keychain_*` serde alias 的版本期保留面。

[auth Spec](../spec/crates/auth.md)、[凭证与脱敏流程](../spec/flows.md)。

## 实施范围

- 核对账号导入、OAuth、Secret backend 及旧配置的真实消费和兼容要求。
- 裁决后只收敛对应接口或别名，不改变 Secret 不入数据库和日志的红线。

## 完成条件

- 每项保留期、删除条件或实际消费者有记录，旧配置处理明确。
- 凭证访问、刷新、过期和脱敏的受影响定向回归通过，Spec 同步。

## 完成记录

本轮起点为 `main / 4e64f8e9e166433653346878457b2a8538c1ae23`，工作区干净；B1–B5 不重做。先读本任务、auth Spec、凭证流程与架构冻结/Secret 边界，用 CodeGraph 定位后核对生产、公开入口、账号导入、OAuth、SecretBackend 与既有测试。未读真实凭证文件或 Keychain，未跑真实 Provider。

| 登记项 | 消费与格式依据 | 裁决 |
| --- | --- | --- |
| `ApiKeyCredential::store` / `store_with_scopes` / `delete` | 全仓只有 `credential.rs` 测试调用。生产写入是 `add_api_key_account` 与 `store_default_api_key`（经 `store_legacy_api_key`），删除是 `remove_provider_account` 与 `delete_default_api_key`。App 装配、额度与 Gateway 只使用 `from_stored` + `resolve`。`store` 把 secret 写到生成的 account，但不更新账号索引。API key 的 scopes 在账号路径恒为空；OAuth scopes 走 `store_oauth_token` / meta。 | 删除三个包装。空密钥拒绝且零写入、元数据与 Debug/JSON 无明文、删除后 `resolve` 为 `NotFound`、同 provider 不同 account 不覆盖，改接到上述生产入口。serde round-trip 改为直接构造 `StoredCredential`。 |
| `StoredCredential::with_expires_at` | 全仓无调用，含测试。OAuth 与 refresh 用结构体字面量或 `expires_at = Some(...)`。到期判断仍在 `needs_refresh` 与 refresh 回归。 | 删除构造器。`expires_at` 字段保留。 |
| `DeviceAuthorization` | `oauth` 公开模块中的公开结构，但 crate 根不重导出，仓库内无其它引用。`request_device_authorization` 构造后把字段原样搬进返回值 `DeviceUserPrompt`。App 与 device 轮询消费的是 `DeviceUserPrompt`。不是冻结 wire；设备授权 HTTP JSON 仍由同一函数解析。 | 删除中转结构，直接构造 `DeviceUserPrompt`。`DeviceUserPrompt` 的 Debug 脱敏回归保留。 |
| `keychain_service` / `keychain_account` serde alias | 2026-08-22 `33dfc36a` 把字段改名为 `secret_*`，alias 只用于反序列化，新写出的 JSON 只用新键。auth.json `FORMAT_VERSION = 1` 是 `service → account → secret`，账号索引 version 1 不含这两个键，生产代码也不从磁盘反序列化 `StoredCredential`。本类型没有独立 format version，仓库内没有批量改写历史 JSON 的迁移。注释中的「一个版本期」没有可审阅的到期边界。 | 保留 alias 与 `legacy_keychain_field_names_migrate_on_rewrite`。删除须同时满足：确认没有仍需读入的外部 JSON 使用旧键；若该 JSON 进入版本化磁盘或线上契约，先升 format version，并由 golden 拒绝旧键。在此之前不另造迁移框架，不改 Secret 语义。 |

`ApiKeyCredential::stored` / `into_stored` 不在登记面，本轮不改。B8 的 MCP 前缀副本不在本任务。

精确写入集：`crates/auth/src/credential.rs`、`crates/auth/src/oauth.rs`、`docs/spec/crates/auth.md`、`docs/spec/security.md`、本任务与 `docs/ROADMAP.md`。security 只改仍写着「一个版本期」的那一句，避免和本裁决冲突。

未新增测试。安全锚点留在既有函数上：空 API key 在两个生产写口都被拒绝且后端仍空；命名账号删除后 secret 槽与 `resolve` 均为 `NotFound`；元数据 Debug/JSON 不含明文；两条命名 API key 的 account 不同且 secret 不互相覆盖；`StoredCredential` scopes round-trip 不含明文；旧 `keychain_*` JSON 仍可读，重写只出 `secret_*`。刷新 singleflight、跨进程锁、损坏 fail-closed 与 OAuth 轮换测试未改，并随 auth 套件通过。

```text
Validated: bash scripts/test.sh --log /tmp/pawork-b6-auth/test-auth.log auth
  → cargo test --offline --tests -p pawork-auth（无额外 feature，exit 0）
  cargo check --offline -p pawork（exit 0）
  rustfmt --edition 2021 --config skip_children=true --check crates/auth/src/credential.rs crates/auth/src/oauth.rs（exit 0）
  git diff --check（exit 0）
Targeted regressions: pawork-auth 82 passed / 0 failed / 1 ignored（既有跨进程 helper，非本轮新增）。覆盖上述空密钥、删除、脱敏、分账号、JSON round-trip、keychain alias，以及未改的刷新、锁、损坏 fail-closed 与 device flow。
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

日志目录：`/tmp/pawork-b6-auth/`。未读取或改动真实凭证。指挥另核对 108 个相对文档文件链接通过，并确认受审六文件 diff 与日志 `final.diff` 字节一致。

2026-10-08 独立审查完成：实现对话 `01a1190a-faf7-7e63-90c4-4b69825dd3f4` 使用 Grok xhigh，审查对话 `01a11919-881a-7ce1-932d-f6d682f42b7e` 使用 Sol max；两者均 `completed`、`error=null`，审查明确 PASS。审查只读核对源码、消费链、diff、原始 Auth 日志与实现对话检查输出，没有重新编译、测试或修改文件。确认删除项无仓库内生产消费、安全锚点保留、Device Flow 行为与旧 alias 兼容不变，B6 自身完成条件满足。指挥据此同步 ROADMAP 并精确暂存本任务文件进行本地提交；不推送、不发布，平台/人工验收与全 workspace 门禁不在本轮范围。
