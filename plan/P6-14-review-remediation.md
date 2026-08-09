# P6-14：Phase 6 评审修复（REVIEW remediation）

> Phase 6 · 主要 Provider · 状态：🟢已完成 · 交付成熟度：Implemented · 依赖：P6-1 ~ P6-9

**最终目的**：消除 [REVIEW.md](../REVIEW.md) §6（Phase 6）评审发现的安全与正确性缺陷、OAuth 未接线与基线/文档漂移——让 Google API key 不进 URL、Anthropic thinking budget 与 max_tokens 不冲突、结构化输出不静默丢弃、OAuth auto-refresh 与 refresh token 轮换回写进入请求路径，并按评审结论处置 `oauth2` 基线虚置。

**涉及范围**：`provider-google`、`provider-anthropic`、`provider-openai`/`provider-openai-compatible`、`auth-service`、根 `Cargo.toml`、ROADMAP「依赖选型基线」、`docs/features/providers.md`

## 细分步骤（分组）

### A. 安全与正确性（V1 / V2 / V3）

1. **V1 Google key 出 URL**：`provider-google` 把 API key 从 `?key=` query 改为 `x-goog-api-key` 请求头，URL 移除 key。目的：secret 不进代理/服务端日志与重定向面，与 Anthropic/OpenAI「头携带 secret」一致。
2. **V2 Anthropic thinking budget 钳制**：构造请求体时将 `thinking.budget_tokens` 钳制为 `< max_tokens`（留余量），并对「未显式设 max_output_tokens 但开 thinking」补默认提升或告警；补触网 mock 断言 `budget_tokens < max_tokens`。目的：默认 max（4096）+ High（8192）不再被 API 400 拒绝。
3. **V3 Anthropic 结构化输出**：`ResponseFormat::Json | JsonSchema` 至少注入一条 system/tool 约束把 schema 喂给模型，或在 `ModelCapabilities` 标注不支持后由上层回退，不再静默丢弃。目的：与 OpenAI/Google 行为对称，P6-8 验收对 Anthropic 真正达成。

### B. OAuth 接线（V4）

4. **V4 auto-refresh 编排**：在 provider 构造/请求前置处接入「检查 `needs_refresh` → 刷新 → 回写 access/refresh → 更新 `expires_at`」，补 `update_oauth_token` 写回函数与「刷新后轮换 token 被持久化」契约测试。目的：P6-4「auto refresh」从原语升级为端到端，轮换型 Provider 不在下次刷新失败。

### C. 健壮性（V5 / V6 / V7 / V8）

5. **V5 cache_control 收敛**：仅在可缓存前缀的稳定边界（system、首个稳定 user turn、工具定义末尾）标记 `cache_control`，受 Anthropic 断点上限约束。目的：多轮长对话不累积超限标记触发 400。
6. **V6 回调服务器/redirect_uri**：`CallbackServer` 循环读到请求头结束或限长后解析；`start()` 用实际绑定端口回填/校验 `redirect_uri`。目的：分片/大 cookie 不解析失败，配置错误不生成错误授权 URL。
7. **V7 PKCE 均匀性**：verifier 生成改拒绝采样或 `base64url(rand 48B)`，补均匀性属性测试。目的：消除 `*b % 66` 取模偏差。
8. **V8 Gemini 工具 id 稳定性**：在 `provider_metadata`/ToolCall 元数据保留 Gemini 原始顺序，回写 functionResponse 时按 name 对齐，不依赖合成 id 跨轮稳定。目的：多工具并发/重放场景 id 稳定。

### D. 基线/包清理

9. **oauth2 决策**：按评审结论维持手写 OAuth，移除根 `Cargo.toml` 的 `oauth2 = "5"`（零引用），在基线与 plan 补「手写自实现理由」说明。目的：基线不再虚置。
10. **回填**：根 `Cargo.toml` 回填 `base64`/`rand`/`sha2`/`url`（OAuth 手写引入，均未登记），同步 ROADMAP 基线表。目的：基线一致。

### E. 文档漂移

11. **providers.md 语义**：补 `include_usage`（随 P2-12 V3）、stop reason 语义、Anthropic 结构化输出（V3）说明；provider_options「覆盖 canonical」语义在 provider-api 文档统一声明。目的：文档与实现一致，避免上游误用。
12. **内置目录新鲜度**：标注三家 `builtin_models()` 数据日期，建立目录更新跟踪项（不在此任务实现远端 `/models`）。目的：目录与线上脱节可见。

### F. 安全复审收口

13. **OAuth secret 与 callback**：为 `TokenSet`、PKCE session、Device Flow 临时凭据提供脱敏 `Debug`；callback 使用固定 `text/plain` 文案且不回显 query。目的：明文 secret 不进日志，loopback 回调不形成反射注入面。
14. **刷新一致性**：refresh 响应缺少 `expires_in` 时保留既有到期策略；同一 credential 的并发请求使用 singleflight gate，共享一次刷新结果。目的：避免误判为永不过期及轮换 refresh token 的并发消费竞态。
15. **Provider options 约束**：Anthropic 的 `max_tokens` / `thinking` / `temperature` / `stop_sequences` 纳入保留字段，不能覆盖 canonical 映射与 thinking clamp。目的：透传选项不能绕过安全/正确性约束。

## 主要产出物

- Google key 改头；Anthropic thinking budget 钳制 + 结构化输出注入；OAuth auto-refresh 编排 + 轮换回写 + 契约测试
- cache_control 收敛；回调服务器/redirect_uri；PKCE 均匀性；Gemini 工具 id 稳定性
- OAuth 临时 secret Debug 脱敏、callback 固定文本响应、refresh singleflight/TTL 保留；Anthropic canonical 字段防覆盖
- oauth2 移除 + 手写说明；base64/rand/sha2/url 回填；providers.md 语义补全 + 目录数据日期

## 验收标准（保留 REVIEW 追踪编号）

- [x] **V1**：Google 请求 key 在 `x-goog-api-key` 头、URL 不含 key（契约断言）
- [x] **V2**：`thinking.budget_tokens < max_tokens` 恒成立（含默认值，触网 mock 断言）
- [x] **V3**：Anthropic 结构化输出注入 schema 指令或显式不支持（不再静默丢弃，测试）
- [x] **V4**：请求前置触发 auto-refresh；轮换的 refresh token 被回写持久化（契约测试）
- [x] **V5**：多轮长对话 cache_control 标记不超 Anthropic 断点上限（用例）
- [x] **V6**：回调分片/大 cookie 可完整解析；redirect_uri 与监听端口一致（测试）
- [x] **V7**：PKCE verifier 生成无取模偏差（base64url 48B 随机输入属性测试）
- [x] **V8**：Gemini functionResponse 按 name/顺序对齐，不依赖合成 id 跨轮稳定（多工具测试）
- [x] **基线**：`oauth2` 移除并补自实现说明；`base64`/`rand`/`sha2`/`url` 回填，ROADMAP 基线表同步
- [x] **文档**：providers.md 含 include_usage/stop reason/结构化输出/provider_options 覆盖语义；内置目录标注数据日期
- [x] **安全复审**：OAuth 临时 secret 不出现在 Debug；callback 不反射 query；缺 TTL 不清空过期时间；并发刷新只交换一次；Anthropic canonical 字段不可被 options 覆盖
- [x] **快速验证**：只运行发生变化的 Provider/OAuth 路径与安全契约子集；仅在 schema 变化时检查生成物，完整三家 modern contract 由 P15-9 集中执行

**相关文档**：[REVIEW.md](../REVIEW.md) §6 · [ADR-002 Agent Engine 与 Provider 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Provider 契约测试](../docs/adr/ADR-015-provider-contract-tests.md) · [providers](../docs/features/providers.md)

> 基线决策（2026-08 review）：手写 OAuth 在「PKCE + token 交换 + Device Flow」子集质量合格（S256 经 RFC 7636 测试向量验证），维持手写、移除 `oauth2`；前提是补齐 V4 三缺口（auto-refresh 接线、轮换回写、回调健壮性）。
