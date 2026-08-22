# R5 — Provider 中立化与凭证收口(T6 + T11 + K-10)

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R5 行。根因:canonical domain 想 Provider 无关,但缺少命名空间化的扩展通道,导致存储层写死 provider 键名清单、通道登记散在三处硬编码;凭证解析在 config/auth 双份同形实现,V1 `keychain_*` 词汇残留;Anthropic adapter 顶部 TODO(全仓唯一真 TODO)静默丢弃调用方能力。本阶段建立扩展元数据契约与单一凭证 locator,并收口 K-10。

## 1. 现状证据(执行时重验;路径为 R1 合并后位置)

- **中立层渗漏**:`storage` event_store 的 `OPAQUE_METADATA_ALLOWLIST = ["openai.responses.summary_entries"]`、`CONTINUATION_METADATA_ALLOWLIST = ["anthropic_block_kind"]`(实态 2026-08-22:`crates/storage/src/session/event_store.rs:21-22`,R1 摊平后路径,形状未漂移)——新增 provider 特性要改存储层常量。
- **通道三处登记**:`crates/providers/src/channels/api_key.rs:26-35` 每通道一个 cfg-feature 枚举变体(实态:在 providers 不在 auth)、`crates/app/src/channels.rs:106-137` 硬编码六通道表 + :148-156 `str→ApiKeyChannel` match(原 `host/app/src/channels.rs:108-153`,行号前移)、engine 红线守护测试名单过期(S12-CR06-10;实态 `crates/engine/tests/no_provider_branch.rs:13-34` 手写 20 名)。
- **键名错位(2026-08-22 核查新发现)**:任务书所称既有落盘键 `openai.responses.summary_entries` **全仓无生产者**;唯一生产者写无前缀 `responses.summary_entries`(`crates/providers/src/responses_reasoning.rs:13`),因不在 allowlist 落盘时被保形脱敏;storage 测试用 allowlist 拼写自造数据掩盖错位。`anthropic_block_kind` 全仓零生产零消费,属预留脚手架。波 A 读兼容须覆盖两种旧拼写。
- **凭证双实现**:`foundation/config/src/env.rs:1`「S0–S5 过渡机制」与 `providers/auth/src/resolve.rs:77` 同形独立实现(`api_key_env_name` 双份);`credential.rs:58-72` `keychain_service/keychain_account` 是 V1 兼容名(实际后端 auth 文件);F05 的 `pawork.mcp.*` 前缀白名单 + 独立 `mcp-auth.json` 是补丁式域隔离。
- **K-10**:`providers/adapters/src/anthropic/mod.rs:7` TODO——prompt cache/thinking/hosted tools/signature/citations 不写 wire,静默丢弃。
- **ReasoningProtector 烂尾对**:`memory_protector.rs:3`「S6 临时宿主实现…S7 接入 Protected Blob Store 后替换」从未发生;`providers/core/src/reasoning.rs:3` 同注;PWB1 protected(1,456 行)因此零生产消费者(R0 D18 保留待本阶段接线)。
- **CapabilityNegotiator**(465 行,R0 D17 保留):P15-8 设计件,包外零调用——本阶段的能力收口正是其消费场景。

## 2. 目标设计

1. **provider_hints 命名空间契约(T6)**:定义 opaque 扩展元数据规则——键名 `provider_hints.<provider>.<key>` 前缀化、大小上限、Secret 扫描拒绝;存储层按规则透传,删除两个键名清单常量;既有落盘键(`openai.responses.summary_entries`、`anthropic_block_kind`)提供读兼容映射(旧键读得出,新写走命名空间)。事件信封形状不变(hints 本就在 opaque metadata 内)。
2. **通道 preset 数据化**:六通道登记收敛为 providers 内单一静态注册表(id、协议、默认 endpoint、凭证形态、feature 门);`ApiKeyChannel` 枚举与 app 硬编码表由注册表派生或删除;新增通道 = 注册表加一行。engine 红线守护测试(「不按 provider 名分支」)从注册表自动生成名单(S12-CR06-10 根治)。
3. **credential locator 合一(T11)**:单一模块定义 env 名推导、auth 文件 service 命名、域隔离规则(`pawork.mcp.*`);config 侧过渡实现删除,auth 为唯一事实源;`keychain_*` 存储词汇迁移为 auth 文件语义(一次性格式迁移:读旧写新 + 迁移测试;JSON v1 兼容读保留一个版本期)。
4. **K-10 Anthropic 能力收口**:以 CapabilityNegotiator 为载体,prompt cache/thinking/hosted tools/signature/server_tool/citations 逐项裁决——实现、显式 `Unsupported`(拒绝而非静默丢弃)、或登记候选;能力表与 registry(R3)同步;TODO 注释清除。
5. **ReasoningProtector 持久化**:宿主注入 `ProtectedBlobStore` 实现替换 `InMemoryReasoningProtector`(兑现 S6/S7 注释承诺);PWB1 获得首个生产消费者;storage `protected` feature 进入 `pawork` 闭包(密钥管理沿 PWB1 既有设计,Secret 红线回归必跑)。若实测成本过高,备选:显式裁决降级并把 PWB1 转冻结候审(须用户确认)。

## 3. 波次拆分

> 波 B 实态回写(2026-08-22 三路核查后):§1 证据路径为 R1 前旧路径,实态 foundation/config → crates/workspace/src/config/env.rs、providers/auth → crates/auth;config env 唯一外部消费者是 app/provider_assembly.rs:23(auth 侧 api_key_env_name 需 pub 化为硬前置)。keychain_* 是代码/serde 词汇而非 auth.json 落盘键(落盘键 service 名 pawork.<provider> 等值不变),迁移 = StoredCredential serde 字段改名 + alias 读旧写新 + 迁移测试。mcp-auth.json 装配在 app/extensions.rs:333-338(ADR-039 决议装配留 app),本波只将其文件名与 pawork.mcp.* 前缀常量化进 auth locator 供消费;tools/mcp/oauth.rs 无前缀白名单逻辑(域隔离靠独立后端文件),白名单唯一位于 tools/mcp/security.rs。

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| A | provider_hints 契约(domain/storage:规则 + 透传 + 读兼容 + golden)∥ 通道注册表(providers/app/engine 守护测试) | storage、domain、providers/responses_reasoning.rs(生产者改新写,轨 a 单点)∥ providers/channels、app(channels/provider_assembly)、engine tests+dev-dep | 并行 ×2 |
| B | credential locator 合一 + keychain 词汇迁移(auth 格式迁移测试先行)+ mcp-auth 域隔离规则收编 | auth(新 locator 模块 + 词汇改名)、workspace(config env 删除)、tools(mcp security/oauth 测试随迁)、app(provider_assembly/auth.rs 消费点 + extensions.rs mcp-auth 常量消费) | 串行(Secret 面单一 owner;安全回归全跑) |
| C | K-10 能力收口 + CapabilityNegotiator 接线 + ReasoningProtector 持久化 | providers(anthropic/negotiate/reasoning)、app(装配注入)、storage(protected feature 闭包) | 串行 |

## 4. 验证

- 信封/DDL golden 零 diff;provider_hints 新旧键读写回归;Secret 扫描拒绝测试。
- auth 迁移:旧 `keychain_*` 文件读取→新格式写入→再读一致;`invalid_grant`/空 refresh 语义不回退(S6 收口行为);全局脱敏回归(trace 0 泄漏)。
- 通道注册表:`pawork models` 六通道聚合与 V2 快照一致;engine 守护测试自动名单生效。
- K-10:每项能力一条 wiremock 契约(写 wire / 显式拒绝);Anthropic 通道真实冒烟(GLM Anthropic 端点,矩阵内)。
- protected:PWB1 golden + 加密读写回归 + `cargo tree` 确认 chacha20poly1305 仅随 feature 进闭包。

## 5. 退出标准

- [x] 存储层 provider 键名清单删除;hints 契约 + golden 生效(波 A,2026-08-22)
- [x] 通道注册表单点登记;engine 红线名单自动生成(波 A,2026-08-22)
- [x] 凭证解析单一事实源;keychain 词汇消失(存储与代码);mcp 域隔离收编(波 B,2026-08-22;StoredCredential serde alias 读旧名兼容期登记 ROADMAP §4)
- [ ] K-10 逐项有决议落地;唯一真 TODO 清除;ReasoningProtector 持久化(或显式改判)
- [ ] 安全红线回归全绿;冒烟通过;v3_plan §3 更新
