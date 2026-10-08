# B8 · MCP Secret 前缀单一事实源裁决

> 状态：✅ 完成（保留导入副本，未物理去重，独立 review PASS）。更新：2026-10-08。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决 workspace 与 auth 间的前缀副本及依赖方向。

## 读取与事实

基线已核实为 `main` / `a906b6a3165d3181fb90ae5c8e94eaeea75eeec0`，工作区干净。B1–B6 不重做。读了本文、[auth Spec](../spec/crates/auth.md)、[workspace Spec](../spec/crates/workspace.md)、[backlog §8](../spec/backlog.md#8-条件性工程项) 与 [架构](../architecture.md) 的包布局和 Secret 边界。CodeGraph 定位 `MCP_SERVICE_PREFIX`、`is_mcp_secret_service`、`mcp_secret_service` 与 MCP `SecretRef::resolve` 后，补读 `extensions.rs` 的删除路径、`import/parse.rs`、`import/apply.rs`、auth 与 workspace / tools 的 Cargo.toml，以及 workspace smoke 对导入 service 字节的断言。未改 MCP / App 消费者，未通读 flows。未读真实凭证，未跑真实 Provider。

运行时前缀在 `crates/auth/src/locator.rs`：`MCP_SERVICE_PREFIX` 为 `pawork.mcp.`（含末尾点），`is_mcp_secret_service` 只做 `starts_with`。同文件测试接受 `pawork.mcp.filesystem`，拒绝 `pawork.mcp`、`pawork.openai` 与 `pawork.chatgpt.oauth`。MCP `SecretRef::resolve` 与 App `mcp_server_secrets_for_removal` 都调用这个函数；App 还用同一常量拼出待删除的 service，并用 `MCP_AUTH_FILE_NAME` 打开独立的 `mcp-auth.json`。这两处不是第二份前缀定义。

workspace 副本是 `crates/workspace/src/import/mcp.rs` 的私有常量，只被 `mcp_secret_service` 使用。`import/parse.rs` 用它把 `${VAR}` / `$VAR` 写成导入 `SecretRef` 和 `CredentialReference`；`import/apply.rs` 的 `select` 用同一函数筛选引用。`tests/smoke.rs` 钉住产物字节 `pawork.mcp.cursor-web` 与 `pawork.mcp.github`。workspace 不读取 `mcp-auth.json`，也不解析明文。

## 实施范围

- 比较现有依赖方向允许的最小方案，不能仅为一个常量先建公共框架。
- 只有裁决后才改变依赖或删除副本；同时收敛对应消费者。

## 完成条件

- 前缀的单一事实源和依赖方案有明确裁决，凭证定位结果保持一致。
- 若实际改变依赖边，同步架构和包级 Spec，并验证受影响 Secret 定位行为。

## 裁决

保留 `MCP_SECRET_SERVICE_PREFIX`。auth locator 仍是运行时命名权威。workspace 这份字符串只构造导入计划，不是第二套门禁。两处字面量都还在，本次没有物理去重。

| 方案 | 结果 | 理由 |
| --- | --- | --- |
| workspace 依赖 auth | 拒绝 | `pawork-auth` 的生产依赖含 `reqwest`、`url`、带 `net` 的 `tokio`、`sha2`、`getrandom`。workspace 的直接消费者是 tools、mcp、app。mcp 与 app 已经依赖 auth；`pawork-tools` 的 Cargo.toml 不依赖 auth。架构 §2.1 写明抽出 mcp 之后 tools 不依赖 rmcp / auth / reqwest。workspace 自身也不做网络和 Secret 存取。为共享 `pawork.mcp.` 扩大 tools 的生产闭包不成立。 |
| 新建常量包 | 拒绝 | 只有一个冻结字符串，用法是导入构造和运行时判定两件不同的事。架构 §2.1 要求新包有独立职责和可强制的边界，不按一个常量拆包。 |
| 前缀下沉 domain | 拒绝 | domain 承载 canonical 契约。`pawork.mcp.` 是 Secret 后端的 service 键，架构把 auth locator 标为 Secret 审计边界。下沉会把存储键放进 domain 公开面，并把权威从 auth 挪走。没有行为缺陷需要这次迁移。 |
| 前缀下沉 policy | 拒绝 | auth 今天不依赖 policy。该前缀不是 `PolicyDecision`，也不是路径内核。 |
| 仅用 dev-dependency 做相等断言 | 拒绝 | 去不掉生产副本，还会让 workspace 测试构建拉起 auth 的 reqwest。本次不新增镜像测试。 |
| 保留私有副本 | 采用 | 见上。运行时拒绝仍只走 `is_mcp_secret_service`。 |

同步范围只包括字面量 `pawork.mcp.`（含末尾点）和由此拼出的 `pawork.mcp.<server>`。不包括 workspace 自己的导入用 `SecretRef` 类型、`mcp-auth.json`，也不包括 Provider / OAuth 前缀。没有末尾点的 `pawork.mcp` 已被 locator 测试拒绝，副本不能改成这个形式。

RV-2026-10-B 的原触发条件是「用户对 B 级清单裁决后处理」。本任务就是该裁决，条件已经触发并关闭，不改写成继续等待下一次授权。RV-2026-10-A 仍按 backlog 留在 engine 测试骨架下次改动时处理，本任务不碰它。

未改依赖边，因此不改架构和 Cargo 生成物。导入产物字节、`mcp-auth.json`、Provider / OAuth 域隔离、冻结 wire / schema 和安全拒绝语义都不变。

## 完成记录

完成条件里的权威、依赖方案和「定位结果保持一致」已经写明：常量值与导入构造都没改。依赖边没变，所以不同步架构，也不为未改动的定位行为重跑回归。注释与文档改动不编译、不新增测试。

2026-10-08 独立 `gpt-6.1-sol` / `max` review 明确 PASS，实现与审查对话均正常 completed、error=null。审查核对六文件 diff、真实消费者、生产依赖与四份实现记录，当前 diff 与 `/tmp/pawork-b8-mcp-prefix/final.diff` 逐字节一致；全程只读，未重复编译、测试或门禁。实现对话：`01a11920-18a3-7020-8355-c7cc1708c763`；独立 review：`01a1192b-dfeb-7ad1-b795-e085aeb2e9b2`。本任务自身完成条件已满足，ROADMAP 同步标为 ✅；未来修改前缀时仍须同批同步两处定义。

写入集：`crates/workspace/src/import/mcp.rs`（只改注释）、`docs/spec/crates/auth.md`、`docs/spec/crates/workspace.md`、`docs/spec/backlog.md`、`docs/ROADMAP.md`、本文。

## 验证

文档与注释改动，不编译，不新增测试。既有 `tests/smoke.rs` 仍钉住 `pawork.mcp.cursor-web` 与 `pawork.mcp.github`；常量值未改，本次没有重跑。

两处定义仍是 `pub const MCP_SERVICE_PREFIX` 与私有 `MCP_SECRET_SERVICE_PREFIX`，值都是 `pawork.mcp.`（11 字节，含末尾点），逐字节相同。架构与 Cargo.toml 不在 diff 中。

```text
Validated: python 比较两处前缀字面量（相等）；rustfmt --edition 2021 --config skip_children=true --check crates/workspace/src/import/mcp.rs（exit 0）；git diff --check（exit 0）；bash scripts/mock/gate.sh --level 0（exit 0，5 个改动 Markdown 的相对链接存在）
Targeted regressions: none。定位器、依赖和导入字节未改，不编译，不重跑 workspace / auth / MCP 回归。
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

日志目录：`/tmp/pawork-b8-mcp-prefix/`。未读取或改动真实凭证。实现者未提交；独立审查通过后由指挥精确暂存本任务六文件并本地提交，不推送。
