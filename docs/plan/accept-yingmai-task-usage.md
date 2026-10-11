# AC-09 · YingMai 任务消耗消费者接入与验收

> 状态：✅ 完成（消费者联合模拟验收，E2 OS 输入 / 窗口模拟）；独立 review PASS；Native 窗口、系统输入与真实供应商费用待验。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

闭合 YingMai 作品、分段和操作类型三个维度的消费者接入。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[任务消耗 UI](../spec/task-usage-ui.md)、[gateway Spec](../spec/crates/gateway.md)、[YingMai ROADMAP](../../../YingMai/docs/ROADMAP.md)。

## 验收范围

- 按现行共享契约核对作品 / 分段 / 操作类型关联和查询范围。
- 消费者实现与人工验收在 YingMai 的真实入口完成，本计划不把 Pawork 单测记为其已接入。

## 完成条件

- 维度与真实账本一致，缺失、失败和未知状态诚实显示。
- 消费者请求和真实界面有可追溯证据。

## 完成记录

2026-10-12，Grok 4.7 xhigh 在 Pawork `d0206680`、YingMai `f5a30c29` 上完成本项联合模拟验收。产品源码无改；临时 Avalonia 11.3.22 Headless 后端承载正式 `MainWindow` 的「任务消耗」页面，控件输入、绑定与 Button 点击 handler 经 `MainViewModel` / `UsageViewModel`、正式 `PaworkClient.QueryUsageAsync` 请求真实本地 Gateway `POST /v1/usage/query` 与隔离 SQLite。项目 / 会话为 fixture 注入，OS 窗口与输入为 E2 模拟，不证明 Native 窗口、物理键鼠、AX 或已安装候选包重启。

账本由四条标明来源的隔离 Chat Completion fixture 经正式 Gateway 建立，没有直接插入 SQLite，也没有走映脉正式生成 UI。作品 A 的三条记录分别为文字成功（Token 11 / 4）、分镜成功（Token 未回传）和文字失败（`upstream_error`，Token 未回传）；作品 B 只有自己的文字成功（3 / 1）。全部费用为空。作品 →「第三段」→「文字」下钻得到 3 / 3 / 2 条，操作分组显示中文「文字」「分镜」；切到另一作品只得到 1 条，返回原作品恢复 3 条。未知 Token 2 次、未知费用 3 次分别显示，计划视频提交 0 秒明确标为非实测 / 扣费。关闭 Gateway 后刷新保留原汇总和明细，并显示查询失败。

证据位于 `/tmp/pawork-ac09-yingmai-task-usage/evidence`：`accept.json`、`ui-report.json`、`ledger.json`、`headless.json`、原日志 / `.exit` 与源码副本。五张 `ui-work.png`、`ui-segment.png`、`ui-operation.png`、`ui-other.png`、`ui-retained.png` 来自同版 Headless + Skia 的实际 `RenderTargetBitmap`，为 1440×940 非空 PNG；它们支持当次模拟界面核看，不升格为 E3 Native 像素或 E4 用户签字。

初次 Native 启动在 Avalonia 渲染时钟返回 `-6661`，accept exit 134，未发出四条请求，账本为空；显示恢复后的重跑仍 exit 134。失败分别保存在 `failed-134/`、`native-rerun-134/`，初次 `orchestrate.exit=2` 与重跑失败不改成成功。首次 Headless exit 2 保存在 `headless-exit-2/`：临时断言错把已开始的流期望为 HTTP 502，并用全行「未知」误数 Token。按现行契约修正为 HTTP 200 后核最终 `failed / upstream_error`，Token 未知按字段判断；最终 Headless 与账本核对分别 exit 0。

- Validated：正式 Gateway 临时 example `cargo build -p pawork-app --example ac09_gateway_host --offline`、定向 rustfmt / check、Avalonia harness Release 构建及两仓 `git diff --check` 均 exit 0；`python3 /tmp/pawork-ac09-yingmai-task-usage/rerun_headless.py` 的最终验收与账本核对分别 exit 0。初次 Native 编排及其历史退出码单独保留。
- Targeted regressions：`dotnet test tests/YingMai.Tests/YingMai.Tests.csproj --filter FullyQualifiedName~UsageQueryKeepsOpaqueFiltersCursorUnknownValuesAndSeparateCurrencyKinds -c Release`，1 passed / 0 skipped，exit 0；验证原样 filter / cursor、未知值和币种种类。媒体矩阵、分页操作、取消、补交及系统手势未在本轮重跑；历史文字结论不当作仍可复核的旧生成物。
- Full workspace gate: NOT RUN（当前未设置 Pawork 全量门禁；本轮未跑 YingMai 全量门禁）。

最终收口见 `final-e2-headless-manifest.json`：冻结的是 `sources/Program.headless.cs` 与对应 harness，旧 Native `manifest.json` 保留为失败证据。指挥核对五张 PNG、当前六份产品源码与冻结源码 SHA，并在实例删除前直接读取真实 SQLite，保存 `coordinator-sqlite-check.json`；四条记录与 `ledger.json` 一致。`metadata-before.json` 与 `metadata-final-headless-after.json` 一致，用户 settings / config / auth 的摘要、inode 与 mtime 未变。本项 mock、Gateway、harness 与 caffeinate 已退出，私有凭证、隔离实例、SSD harness、临时 example / Cargo 片段与 Gateway 二进制已清理；无 Secret 的证据保留。Sol max 独立审查 `01a12d35-00e0-7930-9011-8f2a221a84cb` / turn `01a12d35-0626-7690-b1de-b0bbb4578ab1` PASS，阻断与非阻断问题均无。真实供应商、费用、Native 窗口与物理输入仍待对应环境，不声明全部映脉流程验收通过或已归档。

