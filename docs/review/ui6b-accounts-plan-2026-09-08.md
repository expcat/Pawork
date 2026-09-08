# UI-6b 多账号实施方案（2026-09-08）

状态：**用户已于 2026-09-08 回复「确认实施」；方案已接受，G1 已实现，定向自动检查与代理真窗口检查通过，等待用户人工视觉验收**。实施前基线为 `main` / `697f5fc3`；接受决策见 [ADR-059](../spec/settings.md#adr-059ui-6b-命名账号与持久选择2026-09-08)，验证状态以 [ROADMAP](../ROADMAP.md) 为准。下文源码缺口表保留实施前核查语境。

## 1. 本次要完成的能力

在 UI-6a 已完成的供应商卡内，按供应商声明的认证方式添加多个 API key 或 OAuth 账号；每条账号具有稳定身份、名称、脱敏凭证、过期状态和选择入口。同一供应商可选择一个账号供后续 Run 使用，重启后保留选择。凭证仍只进入 auth backend。

先落实 [backlog G1](../spec/backlog.md#2-已确认多账户与缓存功能族) 进入 UI-6b 的范围；G2 逐账号权威额度与按额度切换仍是 UI-6b 的后续检查点，不能因手动切换完成而将整项额度能力标为完成。账户导入、account factory、`pawork accounts` 完整 CLI、会话亲和、Run 中途换账号和预算 gate 沿用路线图排除范围。

## 2. 实施前核实的缺口

| 环节 | 当前源码事实 | 本次需要补齐 |
| --- | --- | --- |
| API key | [resolve.rs](../../crates/auth/src/resolve.rs) 的写入、读取均固定 `pawork.<provider>/default` | 按稳定 credential ID 定位，保留旧 default 读写入口 |
| OAuth | [default_credential.rs](../../crates/auth/src/default_credential.rs) 固定 `default.access/refresh/meta`；[oauth.rs](../../crates/auth/src/oauth.rs) 已有通用凭证与共享 refresh 核心 | 将持久化元数据与 reload 参数化为指定账号；复用同一 refresh 核心 |
| 原子存储 | [backend.rs](../../crates/auth/src/backend.rs) 已有 `replace_batch`；[file_backend.rs](../../crates/auth/src/file_backend.rs) 写锁内 load-modify-save | 账号索引与凭证同次提交，读改写不能在锁外拼出陈旧索引 |
| Provider 装配 | [provider_assembly.rs](../../crates/app/src/provider_assembly.rs) 中 xAI 固定 key 优先，ChatGPT/Kimi 走 default OAuth | 优先消费已选择的账号及其 kind；无显式选择时保留原解析规则 |
| 生效时机 | [settings/auth.rs](../../crates/app/src/gui_host/handlers/settings/auth.rs) 写凭证后标记 `provider_stale`；[run_start.rs](../../crates/app/src/gui_host/handlers/run_start.rs) 在 Run 前重装配 | 将账号切换接入相同入口；重装配时复核持久选择，覆盖其他 Host 修改选择的情况 |
| GUI | [command.rs](../../crates/protocol/src/app/command.rs) 认证命令仅定位 provider；[settings.rs](../../crates/protocol/src/app/settings.rs) 凭证状态无 ID/名称/选择；[providers.rs](../../apps/desktop/src/ui/settings/providers.rs) 行操作仍是 provider 级 | additive 命令与账号状态，Desktop 按 ID 绑定动作和 AX |
| 额度 | [api_key.rs](../../crates/providers/src/channels/api_key.rs) 的 Go `/usage` 仅返回认证验证成功与否；[query.rs](../../crates/app/src/gui_host/handlers/query.rs) 将 quota 查询交给 [UsageService](../../crates/app/src/services/usage.rs)，未消费查询中的 account/credential 身份 | 不将本地 token 汇总或验证成功推导成账号余额；另做逐账号额度接线 |

## 3. 存储与选择语义

**每个 provider 一个账号索引，放在现有 auth backend。** 索引包含版本、变更 revision、账号列表和 `selected_credential_id`；列表记录 `credential_id`、`kind`、`display_name` 及创建时间。每个账号的 secret 与 OAuth 元数据按其 ID 定位。索引、选择与相关凭证的一次操作必须在同一后端原子事务中完成；生产 FileBackend 用现有写锁和原子 rename，MemoryBackend 保持同样语义。不新增数据库、配置层或生产依赖，auth 文件外层 `version: 1` 与 service/account 容器保持兼容。

`credential_id` 是 Pawork 生成的非机密 opaque ID，不使用 key 片段、邮箱或 OAuth 上游 account claim。ChatGPT 的上游 `account_id` 继续只用于对应账号的路由头，不能与 Pawork 的本地身份混用。

旧 `default` API key 与 OAuth 三条记录先作为已有账号读取；第一次账号写操作再原子登记索引，保留原 secret 定位，不复制或删除旧凭证。旧同 kind default helper 改为统一账号实现的薄入口，避免 default 与 named 两套刷新逻辑。索引损坏、选中 ID 不存在或凭证读取失败均明确报错，不借 env 或其他账号掩盖失败。

没有显式选择时保留当前解析顺序和 env fallback。新添加第一个可用存储账号时可成为默认选择；已有可用凭证（含 env fallback）时，添加账号不静默替换正在使用的身份，需点击选择。env 只显示来源，不读出片段、不导入账号列表、不提供删除按钮。

选择保存成功才更新界面，标记含义是“后续 Run 使用”。下一次 Run（包括同 provider/model）从该账号重新装配 adapter；当前 Run 已取得的凭证快照继续用于其工具续轮。自动命名和压缩等独立请求入口同样经过账号解析，Engine 不按 provider 名分支。账号选择持久化与 Run 凭证快照的边界须由回归验证，不能只依靠界面标记。

删除未选中账号只删除指定 ID；选中账号仍有其他存储账号时要求先选另一条，避免隐式换身份。删除最后一个账号后无存储选择，env fallback 如存在则明确显示为来源。provider 级 `auth_remove` / `auth logout` 继续表示移除该 provider 全部存储凭证，不能留下列表外账号。

OAuth refresh 按账号的 service/account 共用既有 singleflight；账号替换/删除与 refresh 的提交必须互斥或在锁内校验原记录，防止迟到刷新复活已删除账号、覆盖新登录或把 A 的 refresh 写给 B。新登录缺 refresh 不继承旧账号值，刷新响应缺 refresh 才保留同账号值。复用现有锁路径与刷新核心，不另建账户路由框架。

## 4. 已确认的 GUI API 1.15 扩展

本次已确认从 1.14 additive 升至 1.15，采用以下最小词汇；所有新命令仅开放 GUI 通道，并在 registry 登记 `since: 1.15`。

| 命令 | 载荷 | 写入与回执 |
| --- | --- | --- |
| `auth_account_add_api_key` | `provider_id, display_name, api_key: ApiKeySecret` | 先按 UI-6a 验证，成功后原子新增；返回新增账号的脱敏状态 |
| `auth_account_start` | `provider_id, display_name, flow: "oauth"` | 启动原有 OAuth 流程；完成后新增独立账号，返回既有授权提示形状 |
| `auth_account_select` | `provider_id, credential_id` | 校验 provider 归属与支持的 kind，持久选择；回执包含写后 selected ID |
| `auth_account_remove` | `provider_id, credential_id` | 删除指定账号；回执包含被删 ID 和写后 selected ID（可空） |

复用 `auth_cancel` 的 provider 级 OAuth 取消与现有同 provider 单飞闸；添加、选择、删除不能与同 provider 未完成认证相互覆盖。API key 验证失败或 OAuth 失败/取消不留下半条账号，也不改变旧选择。名称 trim 后非空；输入长度和 frame 上限沿用现有边界，不通过名称推导 secret 定位。

`ProviderCredentialStatus` 在现有 `kind / masked_credential / expired / expires_at` 上增加 `credential_id / display_name / selected`。`selected` 表示当前解析策略为后续 Run 选用此条，不表示本次 Run 已切换。列表按创建顺序稳定呈现；legacy 条目缺少真实创建时间时保持固定顺序，不伪造历史时间。`ProviderAuthState` 从选中账号推导，OAuth 的 `expired` 仍只表示时间事实，不预判 refresh 成败。

旧命令仍具有原来的 default 替换/provider 全删语义；不能把未指定 credential ID 的旧 Replace 暗中指向另一个账号。旧协商 minor 的响应过滤新增字段；新 Desktop 连接旧 Host 时仅呈现既有能力，不发送新命令。继续用 `AuthChanged` 通知 provider 状态变化并重查权威账号列表，避免增加一套账号事件流。

实现顺序：先补新命令/状态与旧 minor 的 golden，再改类型、registry、协商版本和 typegen，最后接 Host 与 Desktop。兼容投影只放协议边界，内部共用同一账号实现。

## 5. Desktop 交互

沿用 UI-6a 的卡片、字号、留白和 Settings 元素实测 AX。Credentials 区增加“添加 API key / 添加 OAuth”入口；账号行显示名称、认证方式、掩码和状态，非选中行提供“使用此账号”，选中行显示“已选择”。删除操作明确指向该行名称并沿用现有二次确认。暂不新增重命名、批量操作或独立账号管理页面。

```mermaid
flowchart LR
  A[添加账号] --> B[验证 key 或完成 OAuth]
  B --> C[auth backend 原子保存]
  C --> D[账号列表]
  D --> E[选择账号]
  E --> F[持久保存选择]
  F --> G[下一次 Run 使用该账号]
```

添加表单和 OAuth 等待沿用现有 inline 流程，输入清理、错误保旧、断线 gate 和键盘操作一起接线。账号动作、测高与 AX 必须按 `credential_id` 绑定，不能按列表下标绑定；滚动或删除前一行后不得操作错账号。100% / 125% / 150% 字号及 1080×720 窄窗按既有布局回归和真窗口核验。

## 6. 额度后续检查点

Go `/usage` 已是明确候选来源，不能笼统声称所有供应商都没有额度接口。但当前函数丢弃了读数，现有 `QuotaSnapshotView` 也没有百分比单位；仅凭验证成功或已有 quota 类型，不能宣称逐账号额度已经接通。

G1 接线后，G2 继续核对 Go 的账号/订阅作用域、rolling/weekly/monthly 窗口、百分比与重置时间、抓取时刻和过期处理。两个 key 可能共享同一订阅额度，不得相加或当作两份独立预算。额度查询必须真实消费 provider + credential 身份；未知、失败或过期数据不参与自动选择。完成来源和快照映射后再落实剩余额度显示及 Run 边界的切换规则，所需 quota wire 扩展同批明确，不能把百分比硬塞成 token/count。

在该检查点完成前，Usage 保留无数字的 unavailable 状态，手动账号选择可独立验收；路线图分别记录 G1/G2 进度，不宣布 UI-6b 整体完成。

## 7. 最小写入集与验收

| 范围 | 计划写入与需加载的 Spec |
| --- | --- |
| Auth | credential 存取、索引事务与 refresh 参数化；[auth Spec](../spec/crates/auth.md) |
| App | auth 生命周期、provider 装配、GUI settings handler/Run 前复核；[app Spec](../spec/crates/app.md) |
| Protocol | 命令、账号状态、registry/version、golden 与 typegen；[protocol Spec](../spec/crates/protocol.md) |
| Desktop / Client | Settings controller、projection、账号行/AX/i18n；先核对 [desktop Spec](../spec/crates/desktop.md) 与 [client Spec](../spec/crates/client.md)，Client 通用发送已足够时不改 Client |
| 文档 | 接受后登记 Settings ADR，同步 architecture/contracts、相关包 Spec、GUI 设计、capabilities 和 ROADMAP；本批准备阶段不将拟议行为写成已实现 |

优先扩展已有内联测试：主路径覆盖同 kind 两账号共存、选择/重启恢复、下一 Run 使用目标凭证；关键失败路径覆盖验证失败保旧、原子事务失败不部分写入及 OAuth 删除/刷新竞态。协议 golden 覆盖新词汇和旧 minor，Desktop 沿用现有 Providers 布局/AX 回归。测试只使用 MemoryBackend、临时 FileBackend 和 mock 服务，不操作用户真实认证。

已运行的定向命令（结果与真窗口边界见 ROADMAP 本批证据）：

```bash
cargo test -p pawork-auth -p pawork-app -p pawork-protocol --offline --lib --tests --features pawork-protocol/typegen -- --test-threads=1
cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders
```

同一时刻仅一个 Cargo 进程。构建和最终真窗口检查由主代理收口；真实 Run 固定当次 `opencode-go / glm-5.3-flash`，不改持久默认。没有第二个真实账号时，路由正确性以 mock 请求的凭证命中证明，真窗口只报告实际完成的交互与 Run，不能将同一 key 的两个别名当作两个独立账号验收。

实施前准备验证：只做文档相对文件链接、`git diff --check` 和差异范围检查。生产实现、自动门禁、用户人工验收、归档分别登记；全量 workspace 门禁与发布仍未授权。

## 8. 确认依据

实际读取的 [AGENTS.md §5](../../AGENTS.md#5-验证决策) 明确要求：**“golden 先于实现改动；schema/wire 演进须用户确认。”** [架构 §3.2](../architecture.md#32-冻结契约激活即采用完整形状golden-先于实现改动) 将 GUI 协议列为冻结契约。本次必须新增账号身份、命令和响应字段，无法只改 Desktop 完成；因此先交付本文，再请求确认 §3–§5 的账号方案与 GUI 1.15 扩展。

路线图已授权 UI-6b 的功能方向；上述再次确认针对本次具体 wire 演进，并非对源码核查、方案文档或已授权可逆工作的重复审批。2026-09-08 用户已回复「确认实施」，授权本方案的账号与 GUI 1.15 扩展。
