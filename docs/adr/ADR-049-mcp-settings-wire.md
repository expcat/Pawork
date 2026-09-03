# ADR-049：Settings 工具与 MCP 页 wire（McpTest / McpServerRemove，API 1.7）

- **状态**：Accepted（用户 2026-09-03 确认，D1–D5 按拟议执行）
- **日期**：2026-09-03

## 背景

SET-6 逐页立项的第三页是「工具与 MCP」。任务书（plan/settings.md SET-6 表）锁定的最小真实能力是「Host 权威 MCP list/test/config mutation」，明确不做假工具市场。2026-09-03 经主代理源码实读与两路 glm_explorer 独立只读核查三方确认的基线事实：

- **list**：wire mcp_list 查询自 V1_0 即 GUI 可用，handler 已实装（crates/app/src/gui_host/handlers/query.rs），Desktop Inspector Resources 页已消费（apps/desktop/src/controller.rs / ui/resources.rs）；响应形状钉死 servers 数组（name/transport/state/tools/last_error），数据链零协议改动可复用（query/parse/epoch/stale/断线 fail-closed），无需扩展响应。
- **test**：Host 已实装 AppCore::mcp_test(Option<&str>)（crates/app/src/extensions.rs，现场 ping + list_tools + 回写 slot；stdio server 在 untrusted workspace 拒绝（既有 PermissionDenied 语义）。CLI pawork mcp test 已消费；wire 上无任何 GUI/headless 词汇。
- **config mutation**：Host 零写路径。writer 仅有 write_default_model_pair / write_proxy_url（crates/workspace/src/config/writer.rs）；McpConfig 从已合并配置的 extra.mcp 段读取，Global 层 mcp.servers.<name> 是唯一可持久化写入点；trusted/auto_start 特权旗标仅 builtin/global 层有效，其余层剥离并告警（crates/workspace/src/config/loader.rs 红线）。本片只写 Global 层。
- **Secret**：SecretRef 强制 pawork.mcp.* 命名空间（crates/tools/src/mcp/security.rs），独立 backend mcp-auth.json 与 Provider auth 域隔离（红线 F05）；SecretBackend::delete 存在且幂等；GUI 侧无任何 pawork.mcp.* 写入口（写封装属后续 add 切片，本片不涉及）。

## 拟议决策

### D1 — 新命令 McpTest { name }

- AppCommand::McpTest { name: String }，registry 标 since = V1_7、仅 GUI available；headless/ACP 不开放（沿用 ADR-046 D5 通道保守策略）。required_capability: None（同 mcp_list），不新增 GuiCapability 变体。
- name 必填；未知 server 即 Error（fail-closed，不动现有 slot）。
- Host 语义 = 复用 AppCore::mcp_test(Some(name))：现场 ping + list_tools 并回写该 slot 状态；stdio server 在 untrusted workspace 拒绝（既有 PermissionDenied 语义不变）。本片不动 pawork-policy 与 MCP 装配语义。
- 响应 Data 回执测试后的完整 servers 数组（与 mcp_list 响应同形状）；回执无 Secret，进 command ledger 响应缓存可接受（与 set_default_model 回执同级）。
- 非幂等（触网、可能改变连接状态）：registry idempotent: false。

### D2 — 新命令 McpServerRemove { name }

- AppCommand::McpServerRemove { name: String }，since = V1_7、仅 GUI available，其余同 D1。
- 移除范围锁定 Global 层 mcp.servers.<name>：crates/workspace writer 新增 write_mcp_server_remove——加载 Global 层原始配置、移除该键、未知字段保留、原子写回（复用 write_proxy_url 同族 RMW + 进程锁 + 原子写模式）。
- 定序：校验 server 存在 → Global 原子写 → 清理 SecretRef（pawork.mcp.<name> service 下全部 account 经 SecretBackend::delete 幂等清理（命名空间前缀 fail-closed）→ 内存同步（shutdown 该 slot client → 删 slot → 重建 registry 去除该 server 工具。
- 同会话生效：移除后 mcp_list 不再含该 server，其工具不再注册；重启后与盘一致（盘为权威）。
- name 不存在即 Error，写盘/清密/内存三处皆不动（fail-closed 保旧）；任一步中途失败即 Error 且失败阶段如实回执（不静默）。
- 安全语义：移除是收窄操作，不构成授权扩展；进行中 run 已快照的工具不回溯撤销（快照语义，页面文案诚实标注）。

### D3 — 本片不含 server add；add 登记为后续候选

- GUI 新增 MCP server 需 SecretRef 明文非重放传输（参照 ADR-046 ApiKeySecret 先例 + app 层新建 pawork.mcp.* 受限写封装 + 多字段表单（transport/env/permissions/auto_start/trusted 安全约束，属独立安全切片；本片不做，登记为后续候选。

### D4 — 版本策略

- 沿用 ADR-046 D5 用户已拍板口径：初始未发布版本不采取兼容策略。API minor 升 V1_7（SUPPORTED_API_VERSIONS 追加 1.7 仅作记账；registry since 只作来源元数据，不产生行为分支；不新增 GuiCapability 变体。

### D5 — golden 先行与定向回归

- golden 先于 handler 检入：client 侧 mcp_test / mcp_server_remove 命令帧各一；server 侧回执帧各一（servers 数组形状复用 mcp_list 金样）。typegen 重新生成三产物并过 --check。
- 定向回归上限：主路径两条（mcp_test → slot 状态回写且重查一致；remove → 盘/密/内存三处一致，重查不含）; 关键失败路径一条（未知 name 三处皆不动 fail-closed 保旧）。

## 否决支

- **首片含 add**：需 SecretRef 明文非重放传输 + pawork.mcp.* 受限写封装，属独立安全切片，登记候选。
- **扩展 mcp_list 响应补 endpoint**：Resources 页冻结形状，非本片必要，不做。
- **GUI 切换 trusted/auto_start**：特权旗标仅 Global 层可设，属独立安全决策，登记候选。
- **enable/disable 概念**：McpServerConfig 无此语义，不造。
