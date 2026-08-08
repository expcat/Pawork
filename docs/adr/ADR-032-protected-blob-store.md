# ADR-032：敏感制品使用 Protected Blob Store（非 OS Keychain）

- **状态**：Accepted
- **日期**：2026-08-08
- **取代范围**：部分收窄 [ADR-014](ADR-014-secret-os-keychain.md) 的适用边界（ADR-014 仅限小型凭证）

## 背景

[P15-7 Reasoning State](../../plan/P15-7-reasoning-state.md) 同时要求：(a) crash 后 reasoning continuation 可恢复；(b) 加密 reasoning / signature 不入普通数据库与日志；(c) 可「不落库」。三者互相冲突：既要求崩溃恢复就要求持久化，而「不落库」与持久化矛盾；又禁止塞进普通 Event payload。

早期方案曾考虑把 reasoning 凭证存入 OS Keychain（[ADR-014](ADR-014-secret-os-keychain.md)）。但 reasoning 凭证：

- 体积大（加密 reasoning blob 可达数 KB～数十 KB，远超 Keychain 单条设计）；
- 频次高（每轮 reasoning 产生多条）；
- 需要 retention policy、引用计数 / GC、compaction 兼容；
- 与 Session/Provider 作用域强绑定。

OS Keychain 的设计目标是「少量、长期、用户级凭证」（API Key / OAuth Token），不适合承载 reasoning blob。

## 决策

引入专用 **Protected Blob Store** 承载 reasoning 等敏感制品，与三类存储职责分离：

| 存储 | 承载 | 加密落盘 | 作用域 |
| --- | --- | --- | --- |
| 普通事件 / Projection（ADR-016） | 业务事件、快照 | 否 | Session / Stream |
| 普通 Blob Store（ADR-004） | 大型非敏感内容（tool output / 图片 / diff） | 否 | content-addressed |
| OS Keychain（ADR-014） | 小型用户凭证（API Key / OAuth Token） | 平台托管 | 用户级 |
| **Protected Blob Store（本 ADR）** | **敏感制品（reasoning 凭证等）** | **是（encrypted-at-rest）** | **Provider + Session** |

Protected Blob Store 要求：

- **authenticated encryption at rest**：每个写入使用随机 nonce，密文携带算法 / key version / 作用域元数据和认证标签；
- 数据密钥按 Provider + Session 作用域派生或生成，wrapping key 由组合层注入的 `ProtectedKeyResolver` 解析；`protected-blob-store` 不直接依赖 OS Keychain / `auth-service`，避免依赖倒置；
- OS Keychain 最多保存小型 wrapping key 或其引用，不保存 reasoning blob；密钥材料不进入 Event、日志、诊断包或 GUI；
- 支持 key version 与在线轮换；旧密文在 retention 期内仍可解密，重加密过程不得改变逻辑引用或引用计数；
- 不写普通 Event payload（Event Store 只存 `protected_blob_ref`）；
- 不写日志、不进诊断包；
- 不展示给 GUI 明文；
- 按 Provider + Session 作用域；
- retention policy + reference counting / GC；
- 完整性校验；
- compaction 不破坏当前 reasoning chain 所依赖的 blob 引用计数；
- crash 后经 `protected_blob_ref` 仍可恢复 continuation。

`ReasoningItem` 只保留安全引用：`{ id, summary?, protected_blob_ref, opaque_metadata, continuation_metadata }`，原文（OpenAI `encrypted_content` / Anthropic `signature` / xAI 回灌标识）作为不透明 blob 加密落盘，不解码、不解析。

实现上 `protected-blob-store` 可复用 `artifact-store` 的文件布局、原子写入与 GC 底层，但不能复用普通 Blob 的明文内容地址作为跨作用域标识。物理寻址应使用密文摘要或带作用域密钥的 keyed digest，避免通过相同明文 hash 泄露跨 Session / Provider 的内容相等性；普通 Blob 与 Protected Blob 的命名空间、安全 API 和引用计数必须隔离。

读取时先校验调用者作用域，再验证认证标签与 key version。缺失密钥、作用域不匹配、密文损坏或认证失败必须返回显式的 `ProtectedBlobUnavailable` / `ProtectedBlobCorrupted`，阻断需要该凭证的 continuation 并产生脱敏事件；**禁止**回退到明文落盘、普通 Artifact、Event payload 或日志。

## 后果

- reasoning blob 不入 OS Keychain；ADR-014 适用范围收窄为「小型用户凭证」。
- Event Store 只存安全引用，事件流保持轻量且可重放（ADR-016）。
- 安全验收新增「敏感制品隔离测试」：reasoning 原文不落普通 Event payload / 日志 / Keychain。
- 多 Provider 凭证共存时按作用域隔离，GC 不可误删当前推理链所需 blob。
- 组合层必须提供可测试的 key resolver；存储层保持平台无关，不把 OS Keychain 依赖向领域层或存储底层扩散。

## 相关

- [ADR-004 大型内容 Blob Store](ADR-004-blob-store.md) · [ADR-014 Secret 存 OS Keychain](ADR-014-secret-os-keychain.md) · [ADR-016 事件持久化重放](ADR-016-core-event-persist-replay.md)
- [安全验收](../quality/security-acceptance.md) · [sessions](../features/sessions.md) · [providers](../features/providers.md)
- [P15-7 Reasoning State](../../plan/P15-7-reasoning-state.md)
