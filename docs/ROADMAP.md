# Pawork 活动路线图

> 更新：2026-09-21。复测基线：`main / f8df04b2`，GUI API 1.22。本页只保留未完成工作、本轮发现及其验证依据；本轮复测后已实现下列修复；提交前复核补齐查询锁竞争与附件交互缺口，自动验证与真窗口验收分别记录，未发布。

旧 `docs/review/` 的 38 份文档已清除，原 ROADMAP 中已完成的实现、修复和逐批日志已移出活动清单。历史内容可从 Git `f8df04b2:docs/ROADMAP.md` 与 `f8df04b2:docs/review/` 追溯；删除记录不代表补做了用户验收。当前实现以源码和 [Spec](spec/README.md) 为准。

## 本轮复测

全新隔离实例 `review0921`，独立 bundle 与数据目录，真实窗口 1080×768，检查 100% / 150% 字号。真实请求统一使用 `opencode-go / glm-5.3-flash`，没有改持久模型默认、账号、信任或子代理配置。截图均为本轮采集并重新打开核对，保留在仓库外 `/tmp/pawork-review-0921/screens/`；此临时目录不是长期证据仓库。

| 步骤 | 实际操作与结果 | 本轮截图 / 外部事实 |
| --- | --- | --- |
| 1. 无项目附件 | 首页「+」→ 系统选择器 → 附加 `proof.txt` → 输入问题 → 发送。模型正确返回文件内识别码 `CEDAR-62948`；文本链路通过，呈现问题见 RV-02 | `02-add-menu.png`、`04-attachment-result.png`；会话 `ses-1789982228959-1` 的 `workspace_id=null`，Run `run-gui-1789982229011-1` completed，SQLite 用户消息含附件正文 |
| 2. 本轮搜索 | 开启搜索后显示能力提示并禁发；模型菜单搜索「搜索」无匹配；Esc 保留草稿，关闭搜索恢复发送。负向能力闸通过；真实搜索与来源链接未验 | `05-search-gate.png`、`06-search-model-picker.png`、`07-capability-search-empty.png`；源码菜单确实支持能力筛选，无匹配不能作为“搜索框不支持能力搜索”的缺陷 |
| 3. 图片与草稿 | 无项目附加 `screenshot.png` → 能力提示 / 禁发 → 切换任务 → 返回，图片和正文恢复；移除后恢复发送。选择、隔离和移除通过；预览缺口见 RV-01 | `08-image-attachment.png`、`18-draft-restored.png`；本轮 `opencode-go / glm-5.3-flash` 在 Host / UI 目录未展示图像能力，发送被禁，未发真实识图请求 |
| 4. 子代理完成 | 主任务调用两次 `spawn_agent`，分别计算乘法与输出 12 条中文建议，再 `wait_agent` 汇总；Activity 显示两项完成，点击进入对话栏、切换和刷新可用 | `09-subagents-activity.png`、`20-subagent-details.png`；父 Run `run-gui-1789982455887-2` completed；两个子会话均 completed，结果为 `56088` 与 12 条建议 |
| 5. 子代理阅读与停止 | 对话栏滚动、100%→150%→100%；另创建 300 条建议的长任务，流式期间点击停止。取消通过，当时窗口仍可见部分流式正文，子代理最终 result 为空，父任务正常结束；未据此认定部分正文已提交为助手消息；阅读问题见 RV-04～07 | `12-subagent-result-150.png`、`13-subagent-running.png`、`14-subagent-cancel.png`；子 Run `run-child-fa2-18d74bae6d3dfc28-6` 持久化 `run_cancelled`，父 Run `run-gui-1789982587062-3` completed |
| 6. 设置与失败恢复 | 子代理设置页出现一次 10 秒超时，刷新恢复；并发数输入 0 禁保存、恢复 4，未保存。图片选择器选择不支持的 `unsupported.pdf` 被拒绝、草稿保留；反馈问题见 RV-03、08～10 | `15-subagent-settings.png`、`16-settings-refresh.png`、`17-concurrency-invalid.png`、`19-attachment-error.png`；Host 日志与 AX 树独立留存 |
| 7. 断线阅读 | 停止隔离 Host，主 Timeline 保留，子代理列表与已加载正文消失，面板显示“加载中…”和“未选择子代理”；发现 RV-11 | `21-subagent-disconnected.png`；Host exit 0，窗口与 AX 确认连接已关闭，数据仍在本轮 SQLite 备份中 |

源码外证据：`/tmp/pawork-review-0921/{evidence.json,binaries.json,events.json,session.db,host.log,subagent-panel-ax.txt}`。SQLite 一致性备份含 6 个 Run 的终态：5 completed、1 cancelled。文本附件回合中模型另尝试 `read_file proof.txt`，返回 `workspace not found: ws-unbound` 后使用已注入正文答对；算术子代理的 `run_command` 被不可信工作区 Policy 拒绝后自行算出结果。两次工具失败均有持久事件，不能把 Run 完成等同于所有工具成功。

### 自动验证与边界

- 构建：`cargo build --offline -p pawork -p pawork-desktop --bins --features gpui/runtime_shaders`，通过。
- Desktop：`bash scripts/test.sh desktop`，245 passed、0 failed。
- App / Providers：`bash scripts/test.sh app providers`，487 passed、0 failed、1 ignored。忽略项为需要 `PAWORK_TEST_KIMI_CODE_KEY` 的既有 Kimi live 专项，不记通过。
- 上述命令均使用 `env -u CARGO_MAKEFLAGS DEVELOPER_DIR=/Library/Developer/CommandLineTools RUSTC_WRAPPER=/tmp/pawork-feature-audit/rustc-wrapper.py CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER=/tmp/pawork-feature-audit/test-runner.py` 前缀，复用现有 target；日志为本轮目录中的 `build.log`、`desktop-tests.log`、`backend-tests.log`。
- 定向覆盖附件上传 / 消费 / 持久化 / 客户端隔离、草稿失败保留、图像与搜索能力拒绝、托管搜索请求与 citation 解析、子代理完成 / 取消 / 权限 / 重放。HTTP 边界测试通过不等于真实 Provider 图片或搜索通过。
- 本轮 `opencode-go / glm-5.3-flash` 在 Host / UI 目录未展示图像输入及托管搜索能力，正向真实路径继续待验；此结论不外推到同名模型的其它通道。本轮未覆盖项目图片、完整宽窄三字号矩阵、系统 IME、VoiceOver 实际朗读、其它平台。AX 观察只支持 RV-07，不是无障碍合规结论。
- 文档：已检查 15 份修改文档的 229 个相对链接 / 锚点、全仓 Markdown 中的旧 Review 引用、7 个 Git 历史引用，均通过；`git diff --check` 通过。
- Full workspace gate: **NOT RUN（当前未设置全量门禁）**。

## 本轮新发现的待办

以下 10 项有真窗口和源码 / AX 依据，另有 1 项现场超时（RV-10）。P2 为影响日常使用的缺陷，P3 为信息表达问题。

**修复状态（2026-09-21）**：11 项全部已实现并同批更新 Spec；自动门禁通过——`bash scripts/test.sh desktop` 251 passed / 0 failed（含新增定向：附件结构化拒绝、附件消息分段与折叠、断线保留面板、并发数校验），`bash scripts/test.sh app` 276 passed / 0 failed（含 RV-10 上游阻塞查询分类及慢查询与写命令锁竞争 / 心跳 / 断线取消定向）。全部**等待人工验收**：RV-01 / 03 / 08 的浮层与错误呈现、RV-06 三档字号像素、RV-07 实际朗读、RV-02 真窗口分段与重放一致性、RV-10 按假设（串行主循环被上游阻塞查询饿死）修复后的现场复现核查，均需真窗口复验，未验前不归档。

### 提交前复核（2026-09-21）

- **审查修复**：慢查询改为连接级 `JoinSet` 独立任务，避免串行命令等待 Core 写锁时停止推进持读锁的目录查询；扩展既有连接回归，旧实现先复现超时、修复后通过。附件预览补键盘合成 click 去重与实测 AX 关闭按钮，错误完整换行；子代理断线禁切换并失效旧列表请求，避免清掉正文与过期提示。
- **自动验证**：App 276、Desktop 251 项通过；CLI / Desktop 开发构建通过；修改 Rust 文件定向 rustfmt、`git diff --check`、修改文档 313 个相对文件链接、`cargo tree -p pawork --offline --edges normal` 检查通过。全仓 fmt 有既有差异，本轮仅格式化改动文件；未运行全 workspace 门禁。
- **代理真窗口抽查**：隔离实例 `reviewcommit0921` 使用既有验收 SQLite 副本，1080×768 窗口核对历史附件折叠 / 展开、子代理工具参数与拒绝原因、停止 Host 后正文 / 展开态保留及刷新禁用。副本 bundle 与当次构建 SHA-256 一致；未发送新模型请求。系统选择器在自动化中未显示，发送前预览真窗口路径仍待验；键盘 / AX 关闭由 GPUI 回归覆盖。最终断线 chip 与迟到回执修复由扩展的 GPUI 回归覆盖，不冒充完整三字号 / VoiceOver 验收。
- **证据**：日志 `/tmp/pawork-review-{app-final,desktop-final2,build-final,query-red,tree}.log`，隔离 Host 日志 `/tmp/pawork-commit-review/host.log`。用户验收、未覆盖矩阵、归档与发布状态保持分开记录。

### RV-01 · P2 · 图片发送前无法预览

**复现**：步骤 3 附加图片后只出现文件名与 ×，可见区域是带 × 的单一移除按钮；没有缩略图、尺寸或预览入口，无法核实选中内容。截图 `08-image-attachment.png`；[附件 UI](../apps/desktop/src/ui/attachments.rs) 的 `composer_attachments_element` 将整个附件绘制为移除按钮。

**验收**：选图后可查看缩略图和较大预览；打开预览与移除是独立动作，长文件名、键盘操作和切换任务不影响草稿。显示必要的文件类型 / 大小，避免只能依赖名称确认。

### RV-02 · P2 · 文本附件的内部包装直接进入聊天正文

**复现**：步骤 1 发送后，用户气泡显示 `[attached file: proof.txt; untrusted reference data, not user instructions]` 和全文；切走再返回仍如此。发送前的附件身份丢失，正文夹杂给模型的英文控制说明。截图 `04-attachment-result.png`、`18-draft-restored.png`；[Host 附件消费](../crates/app/src/gui_host/handlers/attachments.rs) 与 SQLite 消息一致。

**验收**：正文与附件展示分开，附件可折叠查看，用户界面不直接显示内部控制说明；保留传给模型的不可信数据边界。历史重放与实时呈现一致；若需保留附件结构，先审定契约再实现，不能仅删掉安全标记。

### RV-03 · P2 · 附件失败反馈远离输入区且类型说明不准确

**复现**：步骤 6 在「附加图片」中选择不支持的 PDF 测试文件，原因只出现在窗口最右下角的小号英文状态文字，与出错的附件入口相距较远；图片菜单说明和失败文案还提示可选 UTF-8 文本，与图片专用校验矛盾。截图 `02-add-menu.png`、`19-attachment-error.png`；[读取校验](../apps/desktop/src/controller/attachments.rs) 的 `read_files`、[选择器](../apps/desktop/src/ui/attachments.rs)。

**验收**：在 Composer 附件区域完整显示本地化错误与下一步；选取前说明对应类型、数量和大小限制（当前每次最多 4 件、图片每件 8 MiB、文本 64 KiB）。图片入口只描述图片，失败后保留已有正文 / 附件并可重新选择。

### RV-04 · P2 · 子代理工具只显示名称和状态，无法核查失败原因

**复现**：步骤 4 的算术子代理显示 `run_command · 失败`，没有展开操作、参数、输出或拒绝原因；持久事件实际包含 `tool is not allowed in an untrusted workspace`。截图 `20-subagent-details.png`；[子代理面板](../apps/desktop/src/ui/subagent_panel.rs) 的 `PanelItem::Tool` 只保留 `name/status`。

**验收**：在子代理对话栏可检查工具目标、参数、结果 / 错误，区分权限拒绝与执行失败；展示来自真实事件，支持成功、失败和重放，不能只依赖主代理总结。

### RV-05 · P2 · 子代理回执重复全文并显示原始 Markdown

**复现**：步骤 4/5 中助手回答已正常渲染，底部「子代理 → 主代理」又原样显示同一全文，`**粗体**`、编号和换行以原始文本铺开，长回执明显拉长面板。截图 `12-subagent-result-150.png`、`20-subagent-details.png`；[结果盒](../apps/desktop/src/ui/subagent_panel.rs) 直接 `.child(result.clone())`。

**验收**：回执按与正文一致的 Markdown 规则渲染；重复长内容默认提供清晰摘要或折叠入口，需要时可读到完整回执，滚动到底与复制不丢内容。

### RV-06 · P2 · 150% 字号下子代理切换标签纵向裁切

**复现**：步骤 5 在 1080×768 窗口把字号调到 150%，顶部代理 chip 的中文纵向裁切；回到 100% 恢复。截图 `11-subagent-150.png`、`12-subagent-result-150.png`；[子代理面板](../apps/desktop/src/ui/subagent_panel.rs) 使用固定 `SUBAGENT_CHIP_HEIGHT=24` 搭配随字号变化的正文，是优先排查点；完整布局根因仍须修复时验证。

**验收**：100% / 125% / 150% 下文字、状态点与焦点框完整，标签栏高度随实际内容变化；多代理切换、滚动与停止按钮仍可达，用真窗口像素复核。

### RV-07 · P2 · 子代理对话正文未进入无障碍树

**复现**：步骤 4 展示完整对话时，AX 中 `inspector-subagent` 只有标题 / 模型 / 状态摘要、切换与刷新按钮；没有用户任务、助手正文、工具详情或结果内容节点。截图 `20-subagent-details.png` 与本轮 `subagent-panel-ax.txt`；[subagent_ax](../apps/desktop/src/ui/subagent_panel.rs) 同样没有发布正文。

**验收**：可见对话内容和工具结果按实际阅读顺序进入 AX，切换代理后的状态与内容同步；几何不伪造。补做键盘与实际朗读核查，不以截图或节点数量代替可读性验证。

### RV-08 · P2 · 非法并发数只有灰色保存按钮

**复现**：步骤 6 并发上限输入 0，保存变灰，无错误或允许范围说明；恢复 4 后可保存。截图 `17-concurrency-invalid.png`；[设置 UI](../apps/desktop/src/ui/settings/subagents.rs) 仅据解析结果禁用按钮，[配置约束](../crates/workspace/src/config/subagents.rs) 实际为 1–16。

**验收**：输入旁标明 1–16，非法 / 空值给出可见且可被 AX 读取的原因；恢复有效值后清除错误，保留原已保存值，不静默截断或自动保存。

### RV-09 · P3 · 子代理全局开关被说明为工作区设置

**复现**：步骤 6 页面写「为整个工作区开启或关闭子代理」，实际配置为 Global-only，用户可能误以为只影响当前项目。截图 `17-concurrency-invalid.png`；[中文 / 英文文案](../apps/desktop/src/ui/i18n.rs) 与[配置范围](../crates/workspace/src/config/subagents.rs) 不一致，tooltip 已写“全局”。

**验收**：可见说明与 tooltip 统一解释对所有项目 / 新发起运行生效，进行中运行不受影响；不改变现有配置语义。

### RV-10 · P2 · 子代理设置查询出现 10 秒超时（待定位）

**现场记录**：步骤 6 首次进入时出现 `Could not load subagent settings · operation receive frame timed out after 10s`，页面仍显示配置；点击刷新后恢复。截图 `15-subagent-settings.png`、`16-settings-refresh.png`。仅观察到一次，未证明稳定复现或数据损坏。

**下一步 / 验收**：沿[设置查询](../apps/desktop/src/controller/settings.rs) 核对 request_id、排队与超时归属，分别复测正常目录和供应商目录探测缓慢的情况；保留可恢复反馈并明确展示配置是否过期。Host 同期有 xAI 目录探测失败 / 超时日志，二者因果关系尚未验证，不能据此直接改超时或归咎 Provider。

### RV-11 · P2 · 断线清空子代理对话并误示加载状态

**复现**：步骤 7 保持算术子代理面板打开，正常停止隔离 Host。主 Timeline 仍可读，子代理 chip、正文和回执全部消失，顶部显示“加载中…”，中央却提示“未选择子代理”。截图 `21-subagent-disconnected.png`；[断线事件处理](../apps/desktop/src/ui/mod.rs) 将 `subagent_activity` 与 `subagent_conversation` 重置为默认值，[面板空态](../apps/desktop/src/ui/subagent_panel.rs) 又将未绑定列表映射为加载中。持久数据未删除，这是已加载内容的可读性与连接状态呈现问题。

**验收**：断线保留当前会话已加载的代理列表、选中项与正文，显示离线 / 内容可能过期并禁用网络动作；不得假示持续加载。重连后重新查询并恢复原选择与阅读位置，切换任务仍保持会话隔离。

## 尚未完成的功能

以下承接原路线图的真实缺口，未因本轮审查自动进入实现；已完成的 Composer 菜单、上传协议、子代理生命周期不重复列为开发任务。

| 功能 | 剩余工作 |
| --- | --- |
| 文件夹附件 | 定义目录内容范围与数量 / 大小限制，明确附加不等于授予整个项目写权限 |
| 持续目标 GUI | 接通设置、状态、停止生命周期，再提供可操作入口 |
| Plan GUI | 接通 Core / CLI 已有的创建、查看、批准、拒绝，不用普通草稿假冒模式 |
| 浏览器上下文附件 | 选择外部 Edge / Chrome 页面，取得快照并展示来源；已有内置浏览器打开不等于附加页面 |
| 技能录制、绘图、插件列表 | 分别定义产物和接入范围；MCP 资源入口不等于插件市场 |

未立项的视觉候选：代码块语法高亮（依赖与测高方案）、工具真实 `+A/−D`（结果 metadata 契约）、失败后重发同一 prompt（消息与 Run 语义）、正文字符级查找高亮、供应商 logo 的权威素材。详细产品候选仍见 [Backlog](spec/backlog.md)。

## 尚未闭合的验收

- **本轮新功能**：RV-01～11 已实现且自动门禁通过，等待上述逐项真窗口复验及用户验收；当前图片识别 / 网络搜索正向真实 Provider 链路被本轮 `opencode-go / glm-5.3-flash` 的目录能力限制，待具备约定测试条件后验证图像内容、搜索来源与引用可用性，不通过换未授权模型绕过。
- **GUI 用户验收**：GUI2-01～07、GUI3-01～08、GUI4 壳层 / 终端及右侧浏览器 / 文件面板的用户验收尚未闭合；历史已实现、自动检查和代理真窗口结果保留在 Git，不重新列开发任务。GUI3-08 动效仍须动态观察。
- **剩余交互矩阵**：系统 IME 在换行后的行为；备用模型目录鼠标状态、管理搜索 / Switch 键盘及完整宽窄三字号；错误与字号反馈共存及跨面板键盘路径。本轮 100% / 150% 局部复测不能覆盖完整矩阵。
- **账号与额度**：真实 OAuth / 刷新、双账号切换、非零额度填充与倒计时 / 过期状态，依赖可用账号及权威读数。不得将旧 401、mock 或本轮普通对话当成通过。
- **测试重构的外部环境**：T0～T8 已有实现与定向验证不再列为开发项；真实 Provider 扩展矩阵、Linux / Windows、WebKit 与容器专项的剩余证据见 [测试与门禁重构计划](testing-refactor-plan.md)。Computer use 原用户指定的自动验收已经完成，不因未发布重新挂人工待验。

## 条件触发的后续项

以下沿用历史登记，本轮未重新认定为当前故障：

- app 改为多宿主 / 多实例共享同库前，复核裸实例计数器的唯一性。
- transport 复活远程实现时，合并或明确 `unpublish` / `revoke` 分工。
- 下次相关 wire 演进时为 `ServerFrame::Snapshot` 补 request_id；现有串行锁防止并发互取，超时后的迟到快照身份仍需契约解决，golden 先行。

验证与交付遵循 [AGENTS.md](../AGENTS.md) 与 [验证规格](spec/verification.md)：已实现、自动检查通过、代理真窗口通过、用户验收、归档和发布分别记录；只运行受影响的检查，不自动启动全 workspace 或发布门禁。
