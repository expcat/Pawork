# Pawork 活动路线图

> 更新：2026-10-03。2026-09-23 全项目 Review 的 T-01～T-12 修复与 API 1.23/1.24 产品能力（文件夹附件、Plan GUI、持续目标、Chrome/Edge 上下文、技能录制、绘图、插件列表、MM-2 视频、MM-3 Qwen 搜索）已实现，通过定向自动门禁与本机真窗口验收并提交；活动计划文档（docs/Plan/）随本轮收口移除，过程与证据从 Git 历史追溯（`git show 46a4ff81:docs/Plan/README.md`）。本页只保留未完成工作，不能由代码存在推定「完美完成」；当前实现以源码和 [Spec](spec/README.md) 为准。

2026-09-27 [包边界调整](architecture.md)已实现：拆出 models、gui-server、acp、mcp、gateway，workspace 为 29 成员，不合并现有包。受影响包的现有回归、协议 golden、依赖边界审计与真实 `pawork` 子进程 3 项测试通过；第三方生产依赖集合、线上协议与持久格式不变。本次未运行 Desktop 构建、真窗口或真实 Provider 验收，不改变下方已有人工验收结论；全 workspace 门禁未运行，改动未提交、未发布、未归档。

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
