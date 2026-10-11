# AC-08 · MoMai 任务消耗消费者接入与验收

> 状态：✅ 完成（消费者联合模拟验收）；独立 review PASS；真实供应商费用、系统输入与最终用户视觉 / 归档待验。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

闭合 MoMai 作品、分部和章节三个维度的消费者接入。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[任务消耗 UI](../spec/task-usage-ui.md)、[gateway Spec](../spec/crates/gateway.md)、[MoMai ROADMAP](../../../MoMai/docs/ROADMAP.md)。

## 验收范围

- 按现行共享契约核对作品 / 分部 / 章节关联和查询范围。
- 只处理消费者接入与界面验收；涉及 MoMai 写入时以对应仓库的约定和任务范围执行。

## 完成条件

- 三个维度查询与真实使用账本匹配，无串作品、漏归属或误导零值。
- 消费者的实际请求、结果和用户界面均有证据。

## 完成记录

2026-10-12，Grok 4.7 xhigh 在 MoMai `ef23904c`、Pawork `62e0b0db` 上完成本项。产品源码无改，临时 Avalonia 窗口装载正式 `TaskUsageView` 与样式；正式页入口 `StartAsync` 和控件绑定的 Command 经 `TaskUsageViewModel` / `TaskUsageService`、保存的 profile 和真实 macOS Keychain 私有别名，请求独立 Gateway HTTP 与临时 SQLite。输入是模拟绑定 / Command，不是物理键鼠、AX 或已安装应用重启。

三个调用均来自正式 `WritingService.GenerateAsync` → Gateway，没有直接种入账本：同作品章节调用、同作品无用量调用及另一作品调用。`task_usage_calls` 共 3 行，均为 `momai / text / succeeded`；作品查询只含本作品 2 行，分部「第一部」与章节「第三章」各 1 行，返回整部作品恢复 2 行，另一作品仅返回自己的 1 行。章节输入 / 输出为 3 / 1；无用量行的 Token、分部与章节为空，界面显示「Token 未回传」，费用均显示未知。旧 `usage_records` 的 2 行 `cost_micros=0 / XXX / unknown` 不作任务费用事实。关闭 Gateway 后刷新保留原汇总、明细与范围并显示查询失败。

证据保存在 `/tmp/pawork-ac08-momai-task-usage/evidence`：`accept.json`、`ledger.json`、`manifest.json`、`sources/`、逐命令原日志 / `.exit` 和 `ui-work.png`、`ui-division.png`、`ui-chapter.png`、`ui-retained.png`。指挥核对当前产品 / 临时源码摘要、SQLite 三行与界面数据、截图和退出码。`failed-run1/` 保留模型 ID 选择错误，`failed-run2/` 保留卷章期望序号错误，`dotnet-build-pre.*` 保留临时项目路径构建失败；账本核对器的只读 URI 问题修正后最终编排退出 0，旧结果保存在 `success-before-ledger-fix/`。这些修正均在临时验收程序内。

- Validated：`python3 /tmp/pawork-ac08-momai-task-usage/orchestrate.py` 最终 exit 0；正式 Gateway 临时 example 的 `cargo build -p pawork-app --example ac08_gateway_host --offline`、定向 rustfmt / check、Avalonia harness Release 构建、Keychain probe、联合验收及账本核对均 exit 0。Rust 既有警告与临时截图 CS0618 警告保留原日志。
- Targeted regressions：`dotnet test tests/MoMai.Infrastructure.Tests/MoMai.Infrastructure.Tests.csproj --filter FullyQualifiedName~TaskUsageWorkflowTests -c Release`，4 passed / 0 skipped。分页、取消与币种区分复用 MoMai 同版既有记录，本轮不宣称重跑。
- Full workspace gate: NOT RUN（当前未设置 Pawork 全量门禁；本轮未跑 MoMai 全量构建 / 测试）。

临时 example / Cargo 片段、SSD harness 与 Gateway 二进制已清理，原源码和构建摘要留证；私有 Keychain 别名及 probe 别名已删除，生产项仍在，用户 config / auth / `momai.db` 前后元数据与摘要一致。上游是本机 mock，`opencode-go/model-a` 仅为隔离目录路由，没有访问已到期套餐。真实供应商用量 / 费用、系统输入及最终用户视觉 / 归档保留待验，不声明已归档或真实环境全面通过。

独立 Sol max reviewer `01a12cc5-9833-7e82-a427-649709d8679f` 于 2026-10-12 对两仓冻结差异、正式链、原日志、SQLite、截图与清理核验 PASS；阻断与非阻断问题均无。产品源码无改，完成状态仅表示本轮授权的模拟验收。

