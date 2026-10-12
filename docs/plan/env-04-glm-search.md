# ENV-04 · MM-3 GLM 搜索环境验收

> 状态：✅ 完成（真实 MCP 往返／模拟审批），独立 review PASS；Host / GUI 与持久接入未验。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#缺环境与外部条件)

## 目标

配置真实 GLM 搜索 MCP 后验收搜索链路。

## 当前依据与读取范围

本轮重核现有 GLM 合法凭证和官方 Coding Plan 搜索 MCP；持久配置为空不直接判为无法准备，结果按实际往返记录。

[mcp Spec](../spec/crates/mcp.md)、[模型网关](../spec/model-gateway.md)、[验证规格](../spec/verification.md)。

## 验收范围

- 核当前凭证与端点条件，以临时配置验证现有正式 MCP 接入；不写持久设置或认证。
- 区分 Provider 托管搜索和 Agent 内置搜索候选，不扩大为全新搜索工具。

## 完成条件

- 真实 MCP 请求、搜索结果及失败路径有证据。
- 缺配置继续明确不可用，不把 mock 或普通对话当搜索成功。

## 完成记录

2026-10-12，使用现有 GLM 凭证、官方远程 HTTP MCP 和临时隔离配置，完成正式 Rust 客户端、工具注册与调度链的真实搜索往返。持久 MCP 现有项只有本地 stdio echo，GLM 搜索未写入持久配置；本轮没有启动该 echo 服务。官方端点为 `https://api.z.ai/api/mcp/web_search_prime/mcp`，认证值仅从正式 `FileBackend` 在进程内解析，临时写入 `MemoryBackend` 的 `pawork.mcp.glm-search / bearer`，由 `SecretRef` 供 Authorization 头读取；不把 Provider service 直接当 MCP locator。

服务端实际发现工具名为 `web_search_prime`，以运行时发现结果为准，不能用文档标题 `webSearchPrime` 替代。initialize 只声明 tools；schema 要求 `search_query`，另有 `content_size`、`location`、`search_domain_filter`、`search_recency_filter` 字符串。白名单仅放行此工具。它的 read_only 为 false，注册名为 `glm-search_web_search_prime`，descriptor 是 ExternalPlugin / requires_approval=true；server 与 host 的注册信任均为 false。

临时 driver 使用正式 `McpConfig::from_value → build_client → register_server_tools → ToolScheduler::execute_named → McpToolAdapter → ManagedMcpClient::call_tool`。调度使用 `NeverAsk`、受信临时 workspace 与 `SimulatedApproval`，属于进程内审批模拟，不能作为 GUI 或人类实际批准。入参只有 `search_query = Z.AI Web Search MCP Server webSearchPrime official documentation`（64 字符）；2026-10-12 19:04:54–19:04:56 +08，工具调用一次，进程记录 2618 ms。

返回 `success=true` / `is_error=false`，正文记录 2100 字符，codec 未截断。JSON 内容包含八条 title / link / content / refer，指向 Google Docs、Wikipedia、Jenkins、Hugo、Grafana、PostgreSQL、.NET 与 Airflow 的泛文档页面，没有命中 docs.z.ai。本次证明实际 MCP 接入和返回链可用，不证明搜索相关性或官方文档检索质量。连接记录为 Connected / http、restart_attempts=0，shutdown 成功。没有执行 Provider 普通对话、Hosted Search、Host / GUI / 网关搜索；持久接入仍需另行配置。

证据目录 `/tmp/pawork-env04-glm-search/evidence`：`env04-glm-search-run.*` 是首次临时 example 编译失败（两处借用需 clone，exit 101）；修正源码后 `env04-glm-search-run2.*` 是正式往返成功（exit 0），不把编译失败当供应商拒绝。失败与成功源码分别为 `env04_glm_search.compile1.rs`、`env04_glm_search.rs`，原结果 `report.json` 保留。`coordinator-decoded-results.json` 仅从原 excerpt 做 JSON 解码，修正辅助 links 提取中的尾部转义，不修改原结果。初次编译没有发出搜索请求。

调用前 `before-hashes.json` 与指挥收尾实时摘要 `coordinator-current-hashes.json` 一致：凭证位于 ~/.pawork，config.toml 与 desktop.json 位于系统 Application Support 的 dev.pawork.pawork 目录；三文件摘要未变，mcp-auth.json 仍不存在。指挥初次核对错用 ~/.pawork 下的配置路径，已根据执行原回执改正；未以那次断言失败登记通过。临时 example、pawork-policy dev-dependency 与其 Cargo.lock 行已回收；仓库只剩本计划与 ROADMAP 差异，无生产代码、默认设置或认证改动。

失败边界复用 B13 的现有 61 条 MCP 回归日志 `/tmp/pawork-b13-mcp-oauth/mcp-rerun.log`（61 passed、0 failed），其中 Secret 域、权限、取消与超时均有原通过锚点。指挥核对自 `2ffc7690507e9ef2271ba5734d43ca28357e0778` 到当前 HEAD 的 `crates/mcp` 无差异，详见 `coordinator-b13-reuse.json`；本轮未重跑或新增这些测试。实际供应商拒绝、自然断连、系统安装与 GUI 审批未触发，不以这份模拟／复用证明替代。

执行对话 `01a12fb6-4dac-7d70-b3a8-a23dd833f3fa`（Grok 4.7 xhigh）；唯一独立审查对话 `01a12fd1-e5be-79d0-8783-0b20addc1e9a`（GPT-6.1 Sol max）在 turn `01a12fd1-eb39-7c10-91f2-63a5af6825e9` 给出 PASS，阻断与非阻断问题均无。受审快照 `FINAL-review.diff` 共 7816 字节，SHA-256 `dd3c10781b72189c825befb2f2086679d7864a2066abb837e2e880eeeb6140bd`；指挥核对当前差异与快照一致后，仅同步本段审查记录与完成状态。原审查回执保存为 `REVIEW-PASS.json`。

```text
Validated: cargo run -p pawork-mcp --example env04_glm_search --offline（初次 exit 101 保留，修正后 exit 0，1 次实际搜索）；持久摘要对比与原结果解码；doc-links / git diff --check
Targeted regressions: none 新跑；复用同版 B13 MCP 61 条回归（无生产代码改动）
Full workspace gate: NOT RUN（当前未设置全量门禁）
```
