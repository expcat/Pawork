# 产品实现与待验收事项

> 更新：2026-09-25。用户已将本页原“未排期”能力全部纳入当前任务，并确认 [API 1.24 契约](product-contracts-2026-09-24.md)。本页区分实现、自动验证、代理窗口验证和用户验收；具体最新结果见 [ROADMAP](../ROADMAP.md)。所有新增变更尚未提交、发布。

## 1. 多模态与搜索

| 项目 | 已实现 | 剩余验证 / 外部条件 |
| --- | --- | --- |
| MM-1 图片 | 原图片输入、模型能力 gate、预览和重放路径保留，绘图复用该路径 | 2026-09-25 真实绘图→预览→发送→识图通过；`opencode-go / glm-5.3-flash` 正确描述黑色 X 与白底，持久 RunCompleted，3,831 / 178 tokens，10 秒。临时模型已恢复禁用 |
| MM-2 视频 | canonical Video、API 1.24 `video_urls`、CLI 重复 `--video-url`、GUI URL 面板、4 条合计上限、拒绝凭据/非 HTTP(S)、模型与实际端点 gate、持久重放 | wire/拒绝/重放自动检查通过；真实支持账号及指定测试模型能力不等同，真实视频往返待相应环境 |
| MM-3 搜索 | 既有 xAI/ChatGPT/OpenCode Responses 保留；Qwen Token Plan 专用端点与明确模型清单接复用 Responses，canonical citation URL 可点击/复制 | Qwen 新接线路由与来源回归通过，末次端点拒绝加固复验通过；指定测试模型不声明搜索时验证拒绝，不偷偷换模型；GLM 搜索 MCP 当前未配置 |

视频首版只传远程引用，不下载/抽帧/转码；Chat Completions 的 ModelStudio 支持模型按[官方视频 wire](https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions)转换，Responses/Anthropic/Kimi 未证实格式明确拒绝。URL 本身不能推导视频长度或精确 token 用量，未把引用长度估算写成视频计费保证。

Qwen 的[Token Plan 专用端点](https://help.aliyun.com/zh/model-studio/token-plan-team-quickstart)与[联网搜索模型清单](https://help.aliyun.com/zh/model-studio/web-search)共同约束能力；Coding Plan 和自定义端点不继承该声明。搜索与视频同时请求时，当前 Responses 路径明确拒绝视频。Kimi 的[工具/Formula 链路](https://platform.kimi.ai/docs/guide/kimi-k3-quickstart)不是 Responses web_search，当前没有将它冒充为已支持。GLM MCP 沿用现行配置与独立凭证域，不将 Provider 凭证静默复制给 MCP。

真实功能测试固定使用 [验证规格](../spec/verification.md) 的 `opencode-go / glm-5.3-flash`，只覆盖当次参数。历史 CAP-PROBE 结果不算本轮复测，mock HTTP 成功不算真实搜索/识图/视频成功。

## 2. 已实现功能的剩余验收

RV-01～11 的既有自动回归随 Desktop 定向检查覆盖；下表保留需要真窗口、键盘/朗读或用户确认的证据。自动回归不能代替真实验收。

| ID | 待验收结果 |
| --- | --- |
| RV-01 | 图片缩略图、预览/移除分离；键盘操作和切任务保留草稿，与 MM-1 同批 |
| RV-02 | 文本附件包装与正文分离，可折叠、保留不可信边界，重放一致 |
| RV-03 | Composer 错误本地化、类型/大小限制准确，与 MM-1 同批 |
| RV-04 | 子代理工具可展开目标、参数与结果，权限拒绝与执行失败可区分 |
| RV-05 | 子代理回执按 Markdown 渲染，长回执折叠/摘要，无重复全文 |
| RV-06 | 100% / 125% / 150% 字号下子代理标签像素无裁切 |
| RV-07 | 子代理正文实际可由键盘访问和朗读，不以 AX 节点数量代替 |
| RV-08 | 本机真窗口通过：输入 17，中文 1–16 原因可见且 AX 可读，保存禁用；恢复输入 4，持久配置未改 |
| RV-09 | 本机真窗口通过：开关说明明确所有项目、仅新 Run 生效；Host 查询 enabled=true、max_concurrent=4 与窗口一致 |
| RV-10 | 本机真窗口页面加载通过；独立 Host 查询约 3 毫秒返回，旧 10 秒超时本次未复现 |
| RV-11 | 断线保留已加载子代理对话，重连恢复，无误导加载态 |

| 验收范围 | 剩余证据 |
| --- | --- |
| GUI2-01～07、GUI3-01～08、GUI4 壳层/终端/浏览器/文件面板 | 用户验收闭合；GUI3-08 动效动态观察 |
| IME、模型目录和跨面板交互 | 换行后系统 IME；目录鼠标、管理搜索/Switch 键盘；宽窄窗三字号；错误与字号反馈共存及焦点路径 |
| 账号与额度 | 真实 OAuth/刷新、双账号切换、非零权威读数、倒计时与过期；不能用 mock、旧 401 或普通对话代替 |
| 特定外部环境 | 尚未完成的 Linux/Windows、WebKit、容器及真实 Provider 扩展矩阵；按实际任务选环境，非默认全量门禁 |

Computer use 原用户指定的自动验收已经完成，不因未发布重新挂人工待验。原批次完成记录保留在 Git，不在活动计划复制测试数量。

## 3. 本轮新增产品能力

| 能力 | 实现边界 | 自动验证 / 真实验收 |
| --- | --- | --- |
| 文件夹附件 | 有界文本快照：最多 128 文件、512 项、8 层、64 KiB；跳过隐藏/符号链接/二进制；附加不扩大写权限 | Desktop 回归通过；真实文件夹选择与 JSON 快照预览通过，确认隐藏项省略 |
| 持续目标 | Host 串行 Run、显式 token/轮数、暂停、追加预算恢复、转向、人工达成/放弃；复用 Plan/Policy；重启只读恢复并要求显式继续 | 生命周期/重放/耗尽已过定向检查，追加预算语义已通过复验；真模型两轮及预算/轮数暂停、追加恢复、独立 CLI 取消、人工达成操作与 Host 状态通过 |
| Plan GUI | 创建/修订/提交/批准/拒绝、expected_version 校验、正在执行时拒绝修改、复用批准 gate | app 与 Desktop 自动回归通过；真窗口保存/提交/批准与 Host 状态通过；最终构建 Tab 从标题→步骤→原因→刷新、Shift+Tab 返回通过，完整键盘矩阵仍待验 |
| 外部浏览器上下文 | macOS Chrome / Edge 当前活动页显式快照，来源 URL/title 与不可信内容包装；系统 Automation 授权及浏览器脚本设置适用 | 边界自动检查通过；Chrome 真机返回错误 12，确认浏览器脚本开关关闭且拒绝自动化切换；用户手动点击仍无勾选，按浏览器环境阻塞登记；未获取正文，未改变系统 Automation 权限，测试页已关闭；其它 OS 明确不支持 |
| 技能录制 | 最近持久工具操作选择→可编辑 SKILL.md→工作区独占新建；过滤 Secret，manifest 最后写入；Unix 目录描述符阻止路径替换 | app/安全拒绝自动回归通过；真窗口选择/编辑/保存与工作区两个技能目录核对通过，真实后续 Run 资源发现待验；非 Unix 安全写入尚未实现 |
| 绘图 | 640×360 手绘、撤销/清空、PNG 附件；有界笔画与点数；复用上传/能力/重放 | 独立 PNG 解码验证通过，Desktop 自动回归通过；真实拖绘、撤销、附加与预览通过 |
| 插件列表 | 查询实际安装记录；当前无运行时，返回真实空列表，技能/MCP 保持独立入口 | 已接线；真实空态窗口及技能录制入口通过，不代表市场/插件运行时已实现 |

目标预算依据 Provider 实际上报用量，首个观察到的耗尽即取消，不能保证延迟用量到达前完全不超额。目标 Run 暂不开放子代理，避免未纳入预算的子任务；自动命名不额外发起目标预算外请求。剩余 token / 轮数与追加额度一起保留。

代码语法高亮、真实工具增删行 metadata、失败 prompt 重发、字符级查找高亮、供应商 logo，以及其它未立项内容继续在 [Backlog](../spec/backlog.md) 管理，不因本轮 Review 自动扩大范围。

2026-09-24 文件夹读取补充：macOS / Linux x86_64、aarch64 使用持有的目录描述符与 `openat(NOFOLLOW)` 逐组件读取，拒绝选择后目录/符号链接替换导致的越界；其它平台入口禁用并显示平台要求。未新增依赖。

2026-09-24 真实窗口补充：视频 URL 保存与移除通过；指定模型不声明视频能力时，Composer 显示中文原因并禁用发送。识图首次真实尝试在 HTTP 请求前失败，根因为启动模型缺少当前供应商的能力证据；已复用有界模型发现修复，保留供应商身份与能力拒绝边界，app 280 项回归通过，2026-09-25 真实识图复验通过。

2026-09-25 构建与恢复：正式 Host/Desktop 构建通过（`cargo build --offline -p pawork -p pawork-desktop --bins --features gpui/runtime_shaders`）；新 Host/窗口从原持久数据恢复任务、计划、目标与已录制操作。插件→录制技能的窗口标题修复已真窗口复验。

本机证据目录：`/tmp/pw-roadmap-sil6orss/`（当次日志，不是仓库可复现门禁）。识图外部事实见 `vision-sep25-evidence.json`；目标/计划重启状态见 `goal_get-sep25-restored.json`、`plan_get-sep25-restored.json`；模型禁用恢复回执见 `temporary-model-restored.json`。浏览器用户手动开启仍失败，继续保持 fail-closed。

最终自动复验（2026-09-25）：Desktop 255 项、browser 5 项通过；基于当前 `target/debug/pawork` 的 `spawn-e2e` 三项通过。焦点回归首次因 GPUI 模拟窗口不具备原生 AX 句柄而失败；明确模拟边界后复验通过，原生 AX 不计入该自动结论。

最终窗口复验（2026-09-25）：`cargo build --offline -p pawork-desktop --bin pawork-desktop --features gpui/runtime_shaders` 通过。更新验收 bundle 后，Plan 的 Tab / Shift+Tab 顺序和可见光标通过；目标从五个字段到刷新、开始，再回标题，跳过五个禁用操作。Chrome 仅在 example.com 上复验，仍返回脚本禁用，中文指引可见且 AX 可读，测试页随后关闭。没有新增真实模型请求或权限变更。
