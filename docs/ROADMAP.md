# Pawork 活动路线图

> 更新：2026-10-04。2026-09-23 全项目 Review 的 T-01～T-12 修复与 API 1.23/1.24 产品能力（文件夹附件、Plan GUI、持续目标、Chrome/Edge 上下文、技能录制、绘图、插件列表、MM-2 视频、MM-3 Qwen 搜索）已实现，通过定向自动门禁与本机真窗口验收并提交；活动计划文档（docs/Plan/）随本轮收口移除，过程与证据从 Git 历史追溯（`git show 46a4ff81:docs/Plan/README.md`）。本页只保留未完成工作，不能由代码存在推定「完美完成」；当前实现以源码和 [Spec](spec/README.md) 为准。2026-10-04 用户授权的全量 Review（R-01～R-10）已完成并收口，B 级待裁决清单与移交项闭环见本文「已收口：2026-10 全量 Review」节。

## 已收口：2026-10 全量 Review（R-01～R-10）

2026-10-04 用户授权的全量 Review（过度设计、低效测试门禁、过时内容、繁琐优化）已按 R-01→R-10 顺序完成，每个任务独立提交。累计（R-01～R-09）：131 个文件 +449 / −1507 行；R-10 收口提交包含本节改写、ADR 索引、下述移交项修复与 backlog 登记。冻结契约、架构红线与安全语义未动；全部 B 级事项见下方汇总。后续用户授权复验并修复发现的问题，本轮已修 B7 / B9 的行为缺陷，其余 API / 产品取舍仍保留待裁决。

### 任务与提交

| 任务 | 范围 | 提交 |
| --- | --- | --- |
| R-01 契约层 | domain、protocol、testkit、transport | 59384b11 |
| R-02 安全与配置 | policy、exec、workspace | 9b8acc3f |
| R-03 持久化与账本 | storage、control-plane | 4dba7669 |
| R-04 模型与网关 | models、providers、auth、gateway | 2d88ef43 |
| R-05 Agent 执行层 | engine、workflow、orchestration、git | 6416c3de |
| R-06 工具与平台包 | tools、mcp、browser、computer-use、terminal | 9e2d3f26 |
| R-07 Core 装配 | app | 5b912fb9 |
| R-08 入口与连接 | cli、acp、client、gui-server、apps/pawork | 147ae5b2 |
| R-09 Desktop | apps/desktop | 7cd1fd36 |
| R-10 文档与门禁收口 | docs/、scripts/、依赖审计 | 215fc8c5 |

### R-10 收口结论

- **依赖审计**：`cargo tree --workspace` 无环、29 成员；`-p pawork` 闭包 25 个 workspace 成员（不含 desktop / browser / terminal / testkit）。`git diff bec896e5..HEAD -- '**/Cargo.toml'` 仅 exec 把 serde_json 从生产依赖移到 dev-dependencies——本轮零第三方生产依赖新增。
- **门禁效率**：scripts/test.sh feature 组合与各包 `[[test]]` required-features 逐一核对一致（providers 9 通道 feature、storage compaction/checkpoint/protected、protocol typegen、transport memory、orchestration git、app ui-fixture、client probe-self-test）；live-smoke 为 env 门控真实 API、spawn-e2e 走 --host，均按设计不进默认门禁；无重复冒烟。脚本本次未改动。
- **ADR 索引**：新增 [docs/adr/README.md](adr/README.md)，裸编号引用（ADR-001～039、P12～P18 系列）统一按索引检索归档正文；AGENTS.md 已加指引。

### 2026-10-04 复验与修复

复验范围为 `bec896e5..215fc8c5` 的 R-01～R-10 全部修改，以及本次发现的问题修复。已实现：OAuth Device 轮询在 pending / slow_down 后等待，请求和等待共用较短有效期限；refresh token 读取保留后端损坏 / IO 错误；budget record_id 增加进程命名空间，避免重启后的同归属 / 同用量记录被幂等账本吞掉。原有 Clone、pending、失败 / 取消重试语义保持。另修复 ADR 历史路径通配符命令，收敛 R 改动行的格式差异，并同步 auth / orchestration Spec。

自动验证命令与结果：

| 验证 | 实际命令 / 范围 | 结果 |
| --- | --- | --- |
| 28 个非 Desktop 包 | `bash scripts/test.sh --log /tmp/pawork-r01-r10-packages-final.log domain protocol testkit transport policy exec workspace storage control-plane models providers auth gateway engine workflow orchestration git tools mcp browser computer-use terminal app cli acp client gui-server pawork` | 2117 个不同目标 / 测试通过；包含安全拒绝、协议 / typegen golden、持久化 / 重放、Provider mock、App / GUI fixture |
| Desktop | `bash scripts/test.sh --log /tmp/pawork-r01-r10-desktop-tests.log desktop` | 258 项通过 |
| 最终 OAuth 慢 HTTP 回归 | `cargo test -p pawork-auth --offline --lib oauth::tests::device_poll_waits_and_obeys_deadline` | 1 项通过；mock HTTP 延迟 5 秒，轮询期限 200 毫秒，2 秒外层护栏 |
| 真实 Host 子进程 | `bash scripts/test.sh --log /tmp/pawork-r01-r10-host-tests.log --host` | 3 项通过：协议往返、权限拒绝、无凭证失败持久化 |
| 正式 Host / Desktop 构建 | `bash scripts/pawork-desktop.sh build` | 两个正式二进制构建成功，按本次 artifact 消息定位产物 |
| 生产依赖 | `cargo metadata --offline --format-version 1`、`cargo tree --workspace --offline --edges normal`、`cargo tree -p pawork --offline --edges normal` | 29 成员，无环；Host 闭包 25；Domain 与 Desktop 边界通过，第三方生产依赖未新增 |

联合回归汇总中的 2119 次通过包含 budget 跨进程回归的两次子进程执行，主测试数按目标 / 名称去重为 2117。4 项显式 ignored 分别为 OAuth refresh 子进程入口（父回归会实际调用）、两个 golden 写入生成器及需真实 Kimi API key 的 live 目录验证；未把真实 Provider 待验记为通过。最终 OAuth 定向复验在联合回归后加强 mock 延迟，生产源码未再改变。

首次联合编译因本次格式整理误删三个 Settings 导入而失败，恢复后全批通过；R-07 原提交包含这些导入，不属于 R 回退。静态检查已确认 R 改动行不再有 rustfmt 差异，保留 R 之前的全仓格式遗留；不声明 `cargo fmt --all --check` 全仓通过。R 涉及的 36 篇 Markdown 共 551 个相对链接、52 个锚点有效；`bash scripts/mock/gate.sh --level 0`、两处行为修复文件的 `rustfmt --check --edition 2021`、三个构建 / 测试入口的 `bash -n` 及 `git diff --check bec896e5` 通过。单个审查者只读检查 R 全范围与最终行为修复，未发现可行动问题。完整测试日志在上述本机 `/tmp` 路径，不能替代仓库可复现命令。

代理真窗口验证已通过（2026-10-04，macOS 26.6.2，本次正式构建）。使用独立 bundle 与隔离目录 `/tmp/pawork-r01-r10-ui.LDQOJk`：`ask-for-writes` 档如实拒绝需审批的 TerminalCreate；隔离 Host 临时改用 `ask-for-dangerous` 后，PTY 在正确项目目录读取 `proof.txt` 并写出三份验收标记。新 Desktop 进程通过 canonical 快照恢复原终端 ID、项目归属和运行态，原 Host / shell PID 不变且能继续输入；暂停 Desktop 50 秒覆盖 30 秒心跳期限及 15 秒 watchdog 采样，窗口显示断线、保留已有输出，点击重连后同一 PTY 继续读写；关闭终端后标签消失且 shell PID 退出。窗口像素 / AX 与真实文件、进程事实相互核对，记录为上述目录的 `window-verification.json` / `process-*.json` / `host.log`，截图只保留在会话中。Desktop 外观配置未改变，验收 Host / Desktop / PTY 均已退出。

Host 仅在当次启动使用 `--provider opencode-go --model glm-5.3-flash`，未改持久默认；隔离凭证目录为空，目录回退与 PTY 不构成真实模型请求验收。用户人工验收、真实 OAuth / Provider 与跨平台验证未完成；未发布、未归档。Full workspace gate: NOT RUN（当前未设置全量门禁）。

### B 级：保留事项与缺陷修复

B7 / B9 已按本次修复授权处理；其余事项涉及公开 API、产品取舍或跨包依赖方向，继续保留待裁决。编号便于引用。

| # | 领域 | 事项 |
| --- | --- | --- |
| B1 | policy/exec | `PolicyEngine.mode` 字段、`classify_command` 公开面、PTY `read_output` 系列零消费——删除或保留；exec 五项安全行为缺口是否补测 |
| B2 | storage | 参数化预留与 `retain` / `snapshot_run` / `conflict_check` / `replay_events` 保留面裁决 |
| B3 | control-plane | QuotaService 读面 `read` / `read_cache_only` / `overview_cache_only` / `invalidate` 零生产消费者（`overview` 已核实有消费者）；LeaseProjection 崩溃恢复机器 app 零接线；`CredentialPicker` / `BudgetCap` / `PolicyGate::Retention` 变体 |
| B4 | workspace+app | prompt-template / 未落地 ResourceKind 集群（`ResourceSelection.prompt_template/prompt_arguments`、`ResourceLimits.max_template_file_refs/max_rendered_prompt_bytes`、`ResourceKind::{PromptTemplate,LanguageServer,UserHook}`、`ResourceInstructionKind::PromptTemplate` app 穷举死臂）——删除跨 workspace + app |
| B5 | models | 四个零消费者 Spec 记录 API：`validate_context` / `merge_provider_source` / `capability_snapshot` / `filter_by_purpose` |
| B6 | auth | `ApiKeyCredential::store/store_with_scopes/delete`、`StoredCredential::with_expires_at`、`DeviceAuthorization` 中转结构、`keychain_*` serde alias 版本期保留面 |
| B7 | auth | 本轮已修：refresh 读取保留 Storage / IO 错误；Device pending / slow_down 等待、HTTP 请求共享有界期限。auth 82 项与定向回归通过，真实 OAuth 仍需账号验收 |
| B8 | auth+workspace | `locator::is_mcp_secret_service` 消费收敛收尾：workspace 前缀副本彻底收敛（跨包依赖方向，见 backlog RV-2026-10-B） |
| B9 | orchestration | 本轮已修：budget record_id 增 PID + 纳秒 + 进程级计数器；两个真实子进程同归属 / 同用量记录都完整入账，原有失败 / 取消重试与 Clone 幂等回归通过 |
| B10 | engine | `run_session_turn` 与 `tool_result_trim` 模块整装零消费者；`ContextBudget::reserved_tokens` 等小 API 集群；`TokenEstimator::estimator_kind`（移除跨 storage） |
| B11 | orchestration | 注入 / 恢复面 `with_task_graph` / `with_worktree_allocator` / `with_patch_merger` / `retry_task` / `recover_report` app 零调用 |
| B12 | git | Stage / HunkStage 生产接线或归档 |
| B13 | mcp | oauth 模块整体零生产接线（`begin_pkce_login` / `complete_pkce_login` / `McpBearerProvider` / `OAuthHttpConnector`，涉 MCP 凭证链路安全语义） |
| B14 | app | `AppCore::from_resolved` / `from_config` 测试专用同步装配双轨（内部 block_on）收敛 |
| B15 | app | gui_host `query.rs` diff_get `complete` 表达式死乘法（零测试覆盖）；`set_model_enabled` / `set_provider_models_enabled` 尾部 cleared_roles 写盘重复收敛；config_unavailable 两处 GUI 文案统一 |
| B16 | app | same-provider 子 core `provider_auth_revision` 不回填导致首轮冗余重装配；lib.rs 对 EventHub / IdempotencyStore 家族 crate 内 re-export 收窄 |
| B17 | cli | gateway 子命令 clap 帮助与 chat 附件上限错误文案为英文（与全库中文 UI 不一致）；chat `local_image_parts` 打开文件后二次 is_file 复查（收益低） |
| B18 | client SDK | headless 稳定面 MockTransport 四个零消费公开方法（`push_responses` / `fail_next_read` / `sent_count` / `assert_sent_json`）与未列入稳定面的 `sdk_version_string`——删除属对外 semver 收缩 |
| B19 | desktop | 终端 create 响应仍把 `id` 当 `terminal_session_id` 回退；`default_socket_path` / `default_token_path` / `token_path_for_instance` 无生产调用 |
| B20 | desktop | `--probe` / `--probe-smoke` 仍选 `glm-coding` / `glm-4.7` 与 `deepseek-v4-flash`，与验证规格功能测试模型不一致（改模型会改变冒烟行为） |
| B21 | desktop | `format_size`（f32）与 `format_byte_size`（f64）边界舍入可能差 0.1；单工具组标题不用已删除的 `tool.group_one`；`task_usage` 的 `tr` 与 `ui/i18n.rs` 的 `t()` 并行、词条不在主目录 |

### C 级：移交项闭环核对

| 移交项 | 来源 | 处理结果 |
| --- | --- | --- |
| ADR / P 裸编号引用统一口径 | R-01/R-03/R-08 | 已落地：docs/adr/README.md 索引 + AGENTS.md 指引 |
| providers 三处 `RecordingProviderSink` 复制 | R-01 | R-04 已收敛到 testkit（测试现 `use pawork_testkit::RecordingProviderSink`） |
| providers 三处 dead_code 警告 | R-05 | 已消：`with_opencode_session` ×2、`reject_kimi_external_image_urls` 补通道 feature cfg 门（对齐 channels/mod.rs 的 api_key 门），默认 feature 编译零警告 |
| engine 测试骨架 LoopContext 转发重复 | R-05 | 登记 backlog RV-2026-10-A（收益中等，不单独立项） |
| client.md API 版本漂移 | R-01→R-08 | R-08 已回写 API 1.26 |
| design.md computer-use 调研锚缺连字符 | R-06 | 已修 |
| workspace mcp.rs 第三份 MCP secret 前缀副本 | R-06/R-07 | 已加注释声明 auth locator 单一事实源与同步义务；彻底收敛转 B8 / backlog RV-2026-10-B |
| design.md「六运行模式」 | R-08 | 已修为七（chat / run / headless / acp / gui / service / gateway） |
| gui-design.md Settings「八页」 | R-09 | 已修为九页（含子代理页） |
| loader.rs non_shorthand_field_patterns lint | R-02 | 已修（shorthand 模式） |
| app 子 core 挂空 MemoryBackend 疑虑 | R-04 | R-10 核实：same-provider 子 core 经 `from_parts_with_protocol` 取父 credential 快照、跨 provider 复用父真实 backend，无空 backend 读凭证问题；残留的 revision 不回填已列 B16 |

2026-09-27 [包边界调整](architecture.md)已实现：拆出 models、gui-server、acp、mcp、gateway，workspace 为 29 成员，不合并现有包。受影响包的现有回归、协议 golden、依赖边界审计与真实 `pawork` 子进程 3 项测试通过；第三方生产依赖集合、线上协议与持久格式不变。本次未运行 Desktop 构建、真窗口或真实 Provider 验收，不改变下方已有人工验收结论；全 workspace 门禁未运行，改动已提交，未发布、未归档。

## 当前任务：任务记录与消耗统计

2026-10-04 已实现两级 task / subtask、正交 group 和 operation 的统一调用日志、全量汇总与游标 API、客户端隔离 / 本地操作报告及侧栏 GPUI 统计窗口；协议 API 1.26 与 SQLite v4 纯新增表，不改变原费用去重或供应商执行路径。定向自动门禁已通过；代理真窗口验证章节 / 分部 / 分段、操作 / 模型 / 客户端 / 时间筛选、补交详情、101 条明细分页、复制 / 键盘、窄窗与断线提示，窗口读数与真实 HTTP / SQLite 一致。模拟上游产生 115 条记录、9 次生成、2 张图片和 10 秒视频计划提交，不作为真实供应商账单证据。用户人工验收未完成，全 workspace 门禁未运行；已提交并推送，未发布、未归档。命令与证据见 [验证记录](spec/model-gateway.md#2026-10-04-任务消耗验证记录)。契约见 [模型网关](spec/model-gateway.md#任务日志与统计2026-10-04)，生产界面与原型差异见 [任务消耗](spec/task-usage-ui.md#7-本轮生产实现)。Pawork 完成验证后，已在 MoMai（作品 / 分部 / 章节）和 YingMai（作品 / 分段 / 操作类型）分别启动消费者接入任务；消费者 UI 实现与验收仍在各自任务进行。

## 等待人工验收（已实现，自动门禁通过）

ADR-064 [模型能力用途筛选](spec/model-gateway.md#决策adr-064-模型能力用途筛选2026-10-01) 已实现：domain `ModelPurpose` canonical 用途词汇（text / image_input / image_output / video_input / web_search）映射既有能力位；models 修复 `filter` 的 image_output 缺口并新增 `default_text` 收窄表；providers 让已实测图像生成模型（wan2.7-image 系）进目录并解析 Chat wire 的 image content part；Host `model_list` 增 `text` / `image_output`（GUI API 1.25），会话模型 / 角色默认 / 子代理规则对非 text 模型 fail-closed；Gateway v1.1 目录携带 `capabilities`、支持 `?purpose=` 过滤、多模态 content part、`web_search` 请求位与 `message.images` 图像输出透传；Desktop Composer 用途 chips + 四能力徽标、Settings 目录徽标；CLI `models --purpose`。定向回归（models / providers / protocol / app / cli / desktop）通过。Composer 用途 chips 与 Settings 徽标已通过代理真窗口复验。仍待用户人工验收与 MoMai 侧 `?purpose=image_output` / wan 真实生图往返；真实图像生成模型端到端冒烟未完成（需专项账号与模型授权）。

2026-10-01～02 Review 已修复：显式输入模态声明不阻断生图 / text 能力回填；纯生图提示按 Chat content parts 发送；CLI / 恢复会话与命名入口复核 text 能力，GUI 模型选择错误映射为 `model_not_text`；内嵌图片受 2 MiB 请求体上限约束，8 KiB 只限制远程 URL；用途按钮的键盘 / AX 接线、用途空态与角色空分组、用途切换后的高亮滚动已补齐。真窗口发现并修复账号副标题消失、语言恢复后搜索 placeholder 不一致、纯生图模型错误显示推理设置、重复模型 ID、子代理视频徽标遗漏、辅助窗口按钮层级与已取消 Run 的工具仍显示运行中。App / Providers / CLI 503 项、GUI 错误路径 1 项、Desktop 258 项、真实 Host 子进程 3 项通过；domain / models / engine / protocol 与 golden 定向回归通过。

代理已走查首页与侧栏、用途菜单、全部九页 Settings、附件与项目引用、Timeline / 审批 / 失败 / 取消 / 查找、Files / Markdown 保存、Changes / diff、PTY、MCP Resources、系统 WebView、子代理空态和五类辅助窗口，并复验宽窄窗、三档字号与中英文；截图保留在会话验收目录，不入仓库。此结论为代理真窗口验收，用户人工签字未完成；录制后续 Run、非空子代理交互与外部浏览器正文沿用下方待验项。Gateway 实际目录的五用途过滤和请求大小边界已核对；固定真实验证模型 `opencode-go / glm-5.3-flash` 返回 `model_not_found`，真实识图 / 生图 / 视频 / 搜索推理未获本轮成功证据。没有切换验证模型或改持久默认；未发布、未归档。

GW-1 [通用本机模型网关](spec/model-gateway.md) 已实现：OpenAI 兼容目录与普通/SSE 补全、按客户端 token 签发/撤销、路由/租约/用量归因、独立 CLI 生命周期。网关 HTTP 与模拟上游定向回归、CLI 库测试和 `pawork` 构建通过；MoMai 实际客户端与网关跨进程目录/撤销、普通/SSE 补全的模拟上游联调通过，真实 CLI serve/status/shutdown 已验证。MoMai 设置窗口已通过 computer use 验证模拟目录连接、模型显示及失效令牌/断开失败处理；真实智谱 GLM-5.3 与 GLM-5.3-Flash 已通过 MoMai GUI 完成大纲、正文、划词和关系双向推演验证；GUI 设置成功保存凭证流程仍待验，详细边界见 [MoMai ROADMAP](../../MoMai/docs/ROADMAP.md)。本轮媒体扩展的实现与验证记录随提交收口；未发布、未归档，全 workspace 门禁未运行。

2026-10-03 未发布改动审查已实现根因修复：纯生图直接解析有界、可取消的完整 JSON，与 SSE 共用内容映射；原生视频使用共享类型和必需后端端口，任务 ID 绑定提交账号，凭证与身份同事务冻结，提交 / 查询共用渠道配置及租约收尾；Go 会话头与 canonical 请求身份统一。domain / providers / gateway / app 所选目标共 522 个不同测试通过，最终 App 网关专项 4 项和真实 Host 子进程 3 项通过；旧生图 mock 改为实际 JSON 契约，mock L0/L2 通过（server smoke 70 项）。当前 `pawork` + 本地 mock 的代理消费者模拟验收 27 项通过：普通 / SSE、生图 PNG 下载解码、视频提交后跨 Host 重启查询及 MP4 下载解码、失败脱敏、参数 / 鉴权拒绝、撤销和退出；原 Global 配置 / 权限已恢复，验收进程已退出。命令、首次失败及证据边界见 [验证记录](spec/model-gateway.md#2026-10-03-验证记录)。这是本机模拟验收，真实媒体供应商、MoMai 媒体 GUI 和用户人工签字仍待验；未发布、未归档。

| 范围 | 待验收要点 |
| --- | --- |
| RV-01 | 图片缩略图、预览/移除分离；键盘操作和切任务保留草稿 |
| RV-02 | 文本附件包装与正文分离，可折叠、保留不可信边界，重放一致 |
| RV-03 | Composer 错误本地化、类型/大小限制准确 |
| RV-04 | 子代理工具可展开目标、参数与结果，权限拒绝与执行失败可区分 |
| RV-05 | 子代理回执按 Markdown 渲染，长回执折叠/摘要，无重复全文 |
| RV-06 | 100% / 125% / 150% 字号下子代理标签像素无裁切 |
| RV-07 | 子代理正文实际可由键盘访问和朗读，不以 AX 节点数量代替 |
| RV-11 | 断线保留已加载子代理对话，重连恢复，无误导加载态 |
| GUI2-01～07、GUI3-01～08、GUI4 | 壳层/终端/浏览器/文件面板用户验收闭合；GUI3-08 动效动态观察 |
| IME、模型目录与跨面板交互 | 换行后系统 IME；目录鼠标、管理搜索/Switch 键盘；宽窄窗三字号；错误与字号反馈共存及焦点路径 |
| Plan GUI | Tab / Shift+Tab 主路径真窗口已通过；完整键盘矩阵待验 |
| 技能录制 | 录制产物在真实后续 Run 中的资源发现待验 |

已闭合不再列出：RV-08～10 设置现场复核（2026-09-25）；MM-1 真实识图（2026-09-25，`opencode-go / glm-5.3-flash` 正确描述绘图并持久 RunCompleted）；持续目标真模型两轮、耗尽暂停、追加恢复、跨进程取消与人工达成；绘图、文件夹附件、插件空列表与视频 URL 面板的真窗口路径。

## 缺环境与外部条件

不能用 mock 或本机测试替代；条件具备时按对应任务验收。

| 事项 | 待办 |
| --- | --- |
| 账号与额度 | 真实 OAuth/刷新、双账号切换、非零权威读数、倒计时与过期 |
| 跨平台 | Linux/Windows、WebKit、容器与真实 Provider 扩展矩阵；Windows 实例锁（share_mode）、listener close/connect 唤醒与连接回收同形态验证 |
| MM-2 视频真实往返 | 支持端点与账号的专项环境（wire/拒绝/重放自动回归已过） |
| MM-3 GLM 搜索 | GLM 搜索 MCP 当前未配置 |
| Chrome / Edge 上下文 | 浏览器返回错误 12（Apple Events JavaScript 未启用），用户手动开启后复验；系统 Automation 权限保持关闭，正文未读取 |
| 技能录制非 Unix 写入 | 安全目录写入原语未在非 Unix 实现，入口明确返回 Unsupported |

库激活前置与按测量触发的条件性技术项、阶段外产品候选统一登记在 [backlog](spec/backlog.md)；发布与全量门禁未授权（BK-RELEASE-01）。
