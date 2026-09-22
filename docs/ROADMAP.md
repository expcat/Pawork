# Pawork 活动路线图

> 更新：2026-09-22。基线：`main / 37fae8f3` + 工作区未提交的 ADR-063 / VISION-2 改动。本页只保留未完成工作；2026-09-21 复测轮的步骤表、自动验证与提交前复核叙述为已完成历史，连同 RV-01～11 的详细复现与验收条文从 Git `37fae8f3:docs/ROADMAP.md` 追溯。当前实现以源码和 [Spec](spec/README.md) 为准。

## 活动任务：多模态与搜索对接（MM-1～MM-3）

来源：2026-09-22 官方文档调研（VISION-2 / SEARCH-1，逐模型证据见 [providers.md](spec/crates/providers.md)）确认一批模型具备图像 / 视频 / 搜索能力，能力声明与默认表已落地，但通道 wire 与产品面尚未对接。按 [backlog](spec/backlog.md) 转正闸门登记：B5（图片输入与多模态）由此进入活动线；B1（webfetch + websearch 内置工具）是 Agent 侧工具，与 MM-3 的 Provider 托管搜索 wire 属不同机制，仍为候选。真实模型验证口径不变：`opencode-go / glm-5.3-flash`，不写持久默认。

### MM-1 · 图像输入端到端补全与正向验收

**需求**：把已声明的 `image_input` 能力变成可用的用户路径。

- Kimi 通道 fail-closed：已落地。`kimi-code` 与 `kimi-platform` 在发 HTTP 前拒绝外部图片 URL，`ms://` 与 base64 继续放行。
- CLI 图像入口：已落地。`chat --prompt` / `run` 接受可重复的 `--image <path>`（png / jpeg / gif / webp，每个不超过 8 MiB）；`--json` 与交互 REPL 拒绝。
- 逐通道图像限制：已落地。发 HTTP 前按官方口径校验——z.ai glm-5.3-flash 仅 png/jpeg、5MB；百炼 qwen3.8/3.7/3.6 为 png/jpeg/webp/bmp/gif、20MB；DeepSeek flash 为 png/jpeg/gif/webp、32MiB；MiniMax M3 仅 jpeg/png/webp（官方无单图字节上限，不另造）；xAI `grok-*` 为 png/jpeg/gif/webp、20MB。xAI Chat Completions 图像 wire 已接 `image_url`（`tests/xai.rs`）。
- 正向真实验收：按约定模型发真实识图请求，确认返回内容确实基于图片；同批闭合 RV-01（发送前预览）/ RV-03（失败反馈）的人工验收。

**非目标**：不做图片生成；不引入 Files API、`detail`、`min_pixels` 等厂商可选增强；不改 canonical `ImageContent` 形状。

**写入集**：`crates/providers`（Kimi URL 拒绝、xAI Chat wire、逐通道限制）、`crates/cli`（附加入口）、`apps/desktop`（验收暴露的缺口）、对应 Spec 同批回写。

**验收**：wire 契约测试（Kimi 外部 URL 拒绝、xAI Chat 写图）；真实模型识图往返；真窗口正向路径（选择 → 预览 → 发送 → 渲染）。

### MM-2 · 视频输入 canonical 与接线

**需求**：为官方确认支持视频的模型（glm-5.3-flash、kimi 系、minimax-m3、百炼 qwen 系）提供视频输入。

- canonical 新增视频 ContentPart 与 `video_input` 能力位：属冻结契约演进，golden 先行，实施前向用户确认契约草案（URL 引用优先；base64 仅在各通道官方支持时映射）。
- 通道 wire：Kimi `video_url`、百炼 `video_url`、MiniMax 视频、智谱视频输入格式（2026-09-22 调研结论，实施时以官方文档复核为准）。
- Desktop 附件入口扩展视频类型与大小限制；capability gate 复用现有 fail-closed 语义，未声明通道一律拒绝。

**非目标**：不做本地抽帧 / 转码预处理；不做音频；不为不支持通道做双轨模拟。

**写入集**：`crates/domain`（契约 + golden）、`crates/protocol`、`crates/providers`、`crates/app`、`apps/desktop`、Spec 同批回写。

**验收**：golden 与 wire 契约测试；真实模型视频理解往返（具备条件时）；真窗口附件路径。

### MM-3 · 搜索接线扩展（Responses 复用 / MCP）与来源展示

**需求**：把「本轮搜索」从目前仅 xAI / ChatGPT Responses 通道，扩展到官方文档支持搜索的其余通道。2026-09-22 调研要点：Kimi 新集成与百炼 Qwen 3.8 的现行搜索入口都是 Responses `web_search`（服务端执行、`url_citation` 引用）；GLM Coding Plan 的官方搜索入口是远程 MCP `webSearchPrime`；DeepSeek 官方明确内置工具一律 Ignored。

- Kimi / Qwen 3.8：优先评估复用现有 Responses `web_search` 组装器与 `url_citation` 归一；若 kimi-code / qwen-token-plan 实际端点不提供 Responses，该通道如实标待验，不回接旧 Chat 形状（Kimi `$web_search` 内置工具 2026-10-20 停用）。
- GLM：通用 API 的 Chat `web_search.enable` 工具对象仍在；Coding Plan 走远程 MCP `webSearchPrime`，先验证既有 MCP 客户端配置即可用，不另造 Chat 特例。
- DeepSeek 保持拒绝；opencode-go 上游透传未证实，未证实前不声明；xAI Chat 的 `search_parameters` 为旧 Live Search，维持不声明。
- 搜索结果与来源归一到现有 `CitationAdded` / `ServerTool` 事件；Desktop Timeline 渲染来源链接（projection 已投影 citation 事件，UI 未渲染）。
- 正向真实验收：真实返回答案与来源链接可用；`search_provider` / `search_model`（ADR-055 保存项）的搜索路由不在本任务。

**非目标**：不做 Agent 侧 webfetch / websearch 内置工具（B1 候选）；不改 Global `web_search` 配置语义；不接 Kimi `$web_search` 旧形状。

**写入集**：`crates/providers`（Kimi / Qwen Responses 或 MCP 声明与 gate 放行）、`crates/app`、`apps/desktop`（来源渲染）、Spec 同批回写。

**验收**：wire 契约测试（写法、citation 归一、未声明拒绝）；真实模型搜索往返与来源渲染真窗口核查。

## 等待人工验收（已实现，自动门禁通过）

RV-01～11（2026-09-21 复测发现）全部已实现并同批更新 Spec；`bash scripts/test.sh desktop` 251 passed / 0 failed、`bash scripts/test.sh app` 276 passed / 0 failed。逐项真窗口复验与用户验收未完成，未验前不归档；详细复现与验收条文见 Git `37fae8f3:docs/ROADMAP.md`。

| ID | 问题 | 待验收要点 |
| --- | --- | --- |
| RV-01 | 图片发送前无法预览 | 缩略图 / 预览与移除分离，键盘与切换任务不掉草稿（随 MM-1 真窗口验收同批闭合） |
| RV-02 | 文本附件内部包装进入聊天正文 | 正文与附件展示分离、可折叠，保留不可信边界，重放一致 |
| RV-03 | 附件失败反馈远离输入区且类型说明不准 | Composer 区域完整本地化错误与限制说明（随 MM-1 同批闭合） |
| RV-04 | 子代理工具只显名称和状态 | 可展开目标 / 参数 / 结果，区分权限拒绝与执行失败 |
| RV-05 | 子代理回执重复全文并显示原始 Markdown | 与正文一致渲染，长回执折叠或摘要 |
| RV-06 | 150% 字号子代理标签纵向裁切 | 100% / 125% / 150% 真窗口像素复核 |
| RV-07 | 子代理正文未进无障碍树 | 键盘与实际朗读核查，不以节点数量代替 |
| RV-08 | 非法并发数只有灰色保存按钮 | 可见且 AX 可读的 1–16 原因 |
| RV-09 | 子代理全局开关说明为工作区设置 | 文案与 Global 语义一致 |
| RV-10 | 子代理设置查询 10s 超时 | 按上游阻塞假设修复后的现场复现核查 |
| RV-11 | 断线清空子代理对话并误示加载 | 断线保留已加载内容、重连恢复 |

## 尚未完成的功能

以下承接既有路线图的真实缺口，未因审查自动进入实现；已完成的 Composer 菜单、上传协议、子代理生命周期不重复列为开发任务。

| 功能 | 剩余工作 |
| --- | --- |
| 文件夹附件 | 定义目录内容范围与数量 / 大小限制，明确附加不等于授予整个项目写权限 |
| 持续目标 GUI | 接通设置、状态、停止生命周期，再提供可操作入口 |
| Plan GUI | 接通 Core / CLI 已有的创建、查看、批准、拒绝，不用普通草稿假冒模式 |
| 浏览器上下文附件 | 选择外部 Edge / Chrome 页面，取得快照并展示来源；已有内置浏览器打开不等于附加页面 |
| 技能录制、绘图、插件列表 | 分别定义产物和接入范围；MCP 资源入口不等于插件市场 |

未立项的视觉候选：代码块语法高亮（依赖与测高方案）、工具真实 `+A/−D`（结果 metadata 契约）、失败后重发同一 prompt（消息与 Run 语义）、正文字符级查找高亮、供应商 logo 的权威素材。详细产品候选仍见 [Backlog](spec/backlog.md)。

## 尚未闭合的验收

- **GUI 用户验收**：GUI2-01～07、GUI3-01～08、GUI4 壳层 / 终端及右侧浏览器 / 文件面板的用户验收尚未闭合；历史已实现、自动检查和代理真窗口结果保留在 Git，不重新列开发任务。GUI3-08 动效仍须动态观察。
- **剩余交互矩阵**：系统 IME 在换行后的行为；备用模型目录鼠标状态、管理搜索 / Switch 键盘及完整宽窄三字号；错误与字号反馈共存及跨面板键盘路径。
- **账号与额度**：真实 OAuth / 刷新、双账号切换、非零额度填充与倒计时 / 过期状态，依赖可用账号及权威读数。不得将旧 401、mock 或普通对话当成通过。
- **测试重构的外部环境**：T0～T8 已收口（分批记录见 Git 历史 `37fae8f3:docs/testing-refactor-plan.md`）；真实 Provider 扩展矩阵、Linux / Windows、WebKit 与容器专项仍未验。Computer use 原用户指定的自动验收已经完成，不因未发布重新挂人工待验。

## 条件触发的后续项

以下沿用历史登记，本轮未重新认定为当前故障：

- app 改为多宿主 / 多实例共享同库前，复核裸实例计数器的唯一性。
- transport 复活远程实现时，合并或明确 `unpublish` / `revoke` 分工。
- 下次相关 wire 演进时为 `ServerFrame::Snapshot` 补 request_id；现有串行锁防止并发互取，超时后的迟到快照身份仍需契约解决，golden 先行。

验证与交付遵循 [AGENTS.md](../AGENTS.md) 与 [验证规格](spec/verification.md)：已实现、自动检查通过、代理真窗口通过、用户验收、归档和发布分别记录；只运行受影响的检查，不自动启动全 workspace 或发布门禁。
