# AC-07 · MoMai 网关凭证与消费者验收

> 状态：✅ 完成（模拟验收）；独立 review PASS；真实供应商、安装应用重启与系统像素另列待验。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

闭合 MoMai 设置保存凭证、配置持久化及现行 Gateway 消费者模拟验收。

用户已明确按 MoMai 现行范围完成凭证与网关验收，并纠正本计划的媒体误写。Pawork Gateway 用途为正文（Text）/ 大纲（Outline）/ 分析（Analysis）/ 润色（Polish）；嵌入使用独立通用端点，Save handler 明确拒绝将 Embedding 配置为 Pawork 用途，不新增媒体入口或嵌入网关能力。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[gateway Spec](../spec/crates/gateway.md)、[模型网关](../spec/model-gateway.md)、[MoMai ROADMAP](../../../MoMai/docs/ROADMAP.md)。

## 验收范围

- 从 MoMai 正式设置输入/绑定/Save 到 Secret 存储与 SQLite 配置，再经后续消费者 Gateway 请求检查凭证和身份归属；外部 Provider 可以模拟，实际 HTTP 与账本不能由 formatter 或 DTO 自测替代。
- 正文 / 大纲 / 分析 / 润色的界面与网关回执联合取证；消费者范围继续以 MoMai 的活动文档和源码为准。模拟使用临时数据库与独立 secure-store 项，不触碰用户默认 `pawork` 凭证。OS 授权若不可用，如实保留待验，不把内存 fake 当成持久化或真实进程重启证据。

## 完成条件

- 消费者成功保存、重新读取并使用正确凭证，Secret 不进入公开输出；模拟与系统存储边界明确。
- 现行用途的消费者界面与网关终态一致，实际请求与 SQLite 身份可追溯；Pawork 自测不能替代消费者模拟验收。

## 完成记录

2026-10-12，执行对话 `01a12c6a-ac74-7862-8dc6-0968e6920409`（Grok 4.7 xhigh）。行为验收基线为 Pawork `ad208b64`、MoMai `74ce478c`。本轮保留现行能力，只在 MoMai 初始设置提示与 Embedding 拒绝提示补齐「润色」，并同步用途文档；错误码、存储与 HTTP 行为未改。

### 联合证据

- 临时 Avalonia 窗口挂载正式 `PaworkConnectionView`、ViewModel、资源与 `AddMoMaiDesktop` 组成入口；模拟修改 Tab / TextBox / ComboBox 的绑定值，检查 `CanExecute` 后调用正式 Button 绑定的 Command。四用途选择、连接、保存状态与令牌输入清空均有回执；这不是物理键鼠或像素验收。
- 正式 Save handler 把四份配置写入隔离 SQLite，`SecretRef=pawork`、loopback `/v1`、流式配置与上下文长度 128000 均重读匹配。测试私有 adapter 仅将该 SecretRef 映射到唯一临时 service alias，委托真实 `MacOsKeychainSecretStore`；同进程与第二个 harness 进程均重读匹配，不使用内存密钥库。系统存储本次实际成功；没有重新启动已安装的 MoMai.app。
- 第二个进程经正式 `WritingService.GenerateAsync` 执行正文 `GenerateManuscript`、大纲 `GenerateOutline`、分析 `AnalyzeChapter`、润色 `RewriteSelection`，四次均成功。上游为确定性 mock；独立进程中的正式 Pawork Gateway、HTTP 和 SQLite 为真实链路，模型为 mock 的 `opencode-go/model-a`，不是一次真实 Provider 验收。
- 四个调用 ID 与账本逐项匹配，client 为 `momai`、tenant 为 `thirdparty/momai`、task 为新作品 `c82ec3bb-2212-4bc5-a088-f3e893b1a36f`。网关 operation 四次均为 `text`，用途区别来自 MoMai 配置，不能把它写成四种网关媒体 operation。提示标记不在账本中。
- Embedding 经正式 Save handler 返回 `modelsettings.pawork_gateway.selection_purpose_invalid`，原凭证仍匹配。验收时拒绝文案漏写润色，随后只改两句字符串并跑既有回归；联合日志保留补文案前的真实输出，不重写历史证据。

### 验证与证据位置

`/tmp/pawork-ac07-momai-gateway/evidence/` 保存 `probe.json`、`save.json`、`consume.json`、`ledger.json`、运行进程退出码、编译日志、`manifest.json` 及实际运行源码 `sources/`。源码运行前后 SHA256 相同，Gateway 二进制摘要与编译/运行对应关系已记录。临时 Rust example 已移出产品写入集，源码副本仍可审查。

Validated：

- `cargo build -p pawork-app --example ac07_gateway_host --offline`：exit 0，仅临时宿主编译，保留既有 warning。
- `dotnet build /Volumes/SSD/ac07-momai-harness/Ac07.csproj -c Release --nologo`：exit 0，0 warning / 0 error；原 `/tmp` 构建因 `/private/tmp` 路径相对化失败，exit 1 与原日志保留，迁移 harness 时的 owner/group 警告也保留。
- `python3 /tmp/pawork-ac07-momai-gateway/orchestrate.py`：exit 0；真实钥匙串 probe、设置保存、第二进程消费均 exit 0。
- 补文案后 `dotnet build src/MoMai.Desktop/MoMai.Desktop.csproj --nologo`：exit 0，0 warning / 0 error；`codegraph sync .` 与 `codegraph status --json .`：无待处理变更、worktree mismatch 或重建建议。
- 指挥只读核账本 `python3 /tmp/pawork-ac07-momai-gateway/evidence/root-ledger-verify.py`：exit 0，四个调用 ID、success 终态、作品、client 与 tenant 均匹配；结果与实际退出码单独保留。

Targeted regressions：补文案后 `dotnet test tests/MoMai.Application.Tests/MoMai.Application.Tests.csproj --filter FullyQualifiedName~SavePaworkModelsHandlerTests --nologo`：exit 0，9 passed / 0 failed / 0 skipped，未新增测试。原工具的实际命令、输出与 exit 另存 `command-results-from-thread.json`、`copy-fix-results-from-thread.json`，未补造历史 `.exit`；一次无效 `codegraph node --source` 查询 exit 1，已换有效方式读取。

Full workspace gate: NOT RUN（未运行 Pawork 全 workspace 或 MoMai 全 solution 门禁）。

### 保留边界

用户现有配置、auth 与默认 MoMai 数据库的 size / mtime / inode 前后相同；默认钥匙串项只检查存在性，未读取其值，不能将存在性检查写成内容摘要。只清理本项临时别名与 token 文件，独立进程已退出。四类创作界面的逐页视觉验收、已安装应用重启、Windows、真实供应商与账单不在本次证据范围；任务消耗消费者三维界面继续由 [AC-08](accept-momai-task-usage.md) 独立验收。

独立 review：`01a12c94-789b-79e3-bca1-b1e8287da1cb`（GPT-6.1 Sol max），turn `01a12c94-7d25-7ad0-a46e-a091a5887de7`，completed / error=null，PASS，阻断与非阻断问题均无。受审两仓 diff 与 33 项证据摘要核对一致，审查者只读核了当前隔离 SQLite，未重编译或重跑测试。指挥提交前再次核对快照及证据一致；通过后仅同步本文件与 ROADMAP 的审查状态，MoMai 源码和受审文档未变。

