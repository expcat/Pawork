# ENV-01 · 真实账号与额度环境验收

> 状态：当前账号与正式额度查询核实完成，独立 review PASS；三个额度窗口真实拒绝，自然临期、双账号、非零读数和倒计时仍待环境。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#缺环境与外部条件)

## 目标

补齐 OAuth、刷新、双账号与权威额度的真实环境证据。

## 当前依据与读取范围

本轮按当前账号元数据、正式额度来源和实际网络结果重新核实；旧网络与套餐结论只作检索线索。日常真实模型已按用户授权调整，额度读口仍按现行源码核对。

[auth Spec](../spec/crates/auth.md)、[Settings Spec](../spec/settings.md)、[验证规格](../spec/verification.md)。

## 验收范围

- 准备真实账号、额度和自然临期窗口，验收 OAuth/刷新、双账号切换、非零权威读数、倒计时和过期。
- 网络、账号或额度缺失按真实阻塞记录，mock 不能替代权威来源。

## 完成条件

- 每项有账号/端点环境、日期和实际结果，Secret 脱敏。
- 失败和过期状态正确，未执行项仍待验；不把缺额度显示为零余额通过。

## 完成记录

2026-10-12 使用当前 `main`、已有 `target/debug/pawork` 和独立 `env01quota` Host，通过正式 GUI 协议发送指定账号的 `account_quota` 查询。Secret 由正式认证链读取，不进入 argv、证据或数据库。仅临时选择 `opencode-go / deepseek-flash` 以启动查询宿主，未执行 Go 模型推理；用户日常功能模型仍按[验证规格 §2.1](../spec/verification.md#21-功能测试用模型)。

| 项目 | 实际条件与结果 | 结论 |
| --- | --- | --- |
| 账号 | 当前存有各一个 Go、xAI、Kimi Code 账号及 GLM / Qwen 默认 Secret 槽；无已配置的同 Provider 双账号 | 元数据核实完成；不证明所有凭证有效，双账号真实切换待验 |
| OAuth | xAI 元数据到期时间早于本次核实时间，未处于自然临期窗口 | 未启动 xAI 刷新或重登录；自然临期刷新待验 |
| 权威额度 | 正式 Go `/usage` 查询的 rolling5h / weekly / monthly 均为 `failed`、`forbidden`，used / limit / remaining / reset 均为空，`from_cache=false` | 真实拒绝已记录；没有百分比读数，不当作零余额或非零额度通过 |
| 网络补充 | 未认证 `/usage` 经当前代理及直连均得到 HTTP 403 | 只证明这些请求的拒绝结果，不能据此判定凭证有效、套餐权限或 403 的唯一原因 |
| 配置与回收 | 查询前后 auth / config / desktop 摘要一致；本项 Host exit 0、临时实例 / token / socket 已回收，既有 Pawork PID 仍存活 | 本项未改持久认证或设置；B20 较早的 auth 摘要变化归属仍未核明 |
| 倒计时 / 过期 | 本次失败窗口未返回 reset | 权威倒计时与额度过期待验；不以模拟替代 |

本项未修改生产源码或新增测试。日常模型终态只复用 B20 的 GLM 证据，不能替代本项权威额度或 OAuth 刷新。未开展新的 GUI 模拟，不声明 E4 人工签字。

证据：`/tmp/pawork-env01-accounts-quota/evidence`。`safe-meta` 首次因 Python 缺 `tomllib` exit 1，修正运行解释器后的 `safe-meta-py313` exit 0；原失败保留。`auth-list-go`、`quota-go-usage`、`usage-direct-noauth` 与 `cleanup-env01quota` 各 exit 0；查询脚本 exit 0 表示取证完成，三个产品额度窗口仍失败。B20 参考为 `/tmp/pawork-b20-probe-acceptance/evidence/coordinator-final-smoke-check.json`，不改写为本次认证复验。

Validated: 现有二进制的正式账号列表、独立 Host / GUI 协议额度查询、脱敏元数据与前后摘要、真实 HTTP 拒绝和实例回收；文档链接与 diff 检查。

Targeted regressions: none（无生产代码变化；未为文档运行 Cargo 或重造模拟框架）。

Full workspace gate: NOT RUN（当前未设置全量门禁）。

