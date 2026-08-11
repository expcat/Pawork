# P14-4：网页抓取适配器

> Phase 14 · 模型用量与额度监控 · 状态：🟢已完成 · 交付成熟度：TargetVerified（有界：内置内存审计仅测试断言用，生产审计 sink 延 P18-13） · 依赖：P14-1、P2-1

**最终目的**：为「无公开 usage API、只能从控制台网页查看额度」的供应商提供网页抓取适配器，以登录态会话抓取控制台页面并解析额度数字，补全 API 类适配器覆盖不到的供应商，同时以明确低可信度标注让用户知晓数据可能因页面改版失效。

**涉及范围**：`quota-service`；复用 `provider-runtime`（HTTP）+ HTML 解析；会话凭据经 provider contract 注入（无 `auth-service` 依赖）

## 细分步骤

1. **会话凭据来源** —— 目的：支持以浏览器登录态（cookie / session token，由用户在受信任前提下导入，存 SecretBackend）发起带认证请求，复用 auth-service 存取，明文不落库。
2. **页面抓取** —— 目的：在 `quota-service` 内实现 `WebScrapeAdapter`，复用 `provider-runtime` 拉取控制台页面与必要的前置 API（如渲染所需的初始数据 JSON）。
3. **选择器配置化** —— 目的：以「供应商 + 窗口 → CSS / JSON 路径 + 解析规则」的可配置表表达抽取逻辑，避免硬编码、便于页面改版后只更新配置。
4. **解析容错与校验** —— 目的：抽取数字后做合理性校验（非负、在预期量级），解析失败返回 `ParseFailed` 并附带原始片段供诊断，不 panic。
5. **低可信度与版本脆弱性** —— 目的：`confidence = scraped`、`source` 记录页面 URL 与选择器版本；连续抓取失败时在 P14-9 自动降级为本地推算（P14-7）。
6. **合规与频率控制** —— 目的：对页面抓取施加严格最低间隔与缓存，避免高频请求；抓取行为纳入审计日志（脱敏）。
7. **测试** —— 目的：用静态页面夹具 + wiremock 验证抽取、容错与选择器版本切换。

## 主要产出物

- `WebScrapeAdapter` 实现
- 选择器配置表与版本字段
- 抓取 / 解析 / 容错测试夹具

## 验收标准

- [x] 能从示例控制台页面抽取至少一种窗口额度
- [x] 选择器失效时返回可诊断的 `ParseFailed`，不崩溃
- [x] 抓取受最低间隔与缓存约束，行为入审计日志（当前为进程内有界内存 audit Vec，仅测试断言；生产审计归 P18-13）
- [x] 抓取数据标注 `confidence = scraped`

**实现边界（2026-08-11 review-remediation）**：WebScrape 内置的有界内存 audit Vec 与 `RefreshScheduler::AuditSink` 职责重叠，当前仅用于测试断言，不构成第二份生产审计记录；生产审计统一走 scheduler/控制面 sink，所有权归 P18-13，本任务的审计项相应延后。

**相关文档**：[usage-quota](../docs/features/usage-quota.md) · [providers](../docs/features/providers.md) · [auth](../docs/features/auth.md) · [observability](../docs/features/observability.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：HTML 解析新增 `scraper`（基于 html5ever + selectors，采用率高、活跃维护），仅用最小子集（解析 + 选择器匹配）；JSON 前置数据复用 `serde_json`。需同步回 ROADMAP「依赖选型基线」。
