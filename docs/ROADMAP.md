# Pawork 活动路线图

> 更新：2026-10-04。只保留未完成工作；现行能力与契约见 [Spec](spec/README.md)，已完成计划与过程记录从 Git 历史追溯。

## 待裁决：API 与产品取舍

以下事项涉及公开 API、产品范围或跨包依赖方向，需裁决后处理。保留原编号供引用。

| # | 领域 | 事项 |
| --- | --- | --- |
| B1 | policy/exec | `PolicyEngine.mode` 字段、`classify_command` 公开面、PTY `read_output` 系列零消费——删除或保留；exec 五项安全行为缺口是否补测 |
| B2 | storage | 参数化预留与 `retain` / `snapshot_run` / `conflict_check` / `replay_events` 保留面裁决 |
| B3 | control-plane | QuotaService 读面 `read` / `read_cache_only` / `overview_cache_only` / `invalidate` 零生产消费者（`overview` 已核实有消费者）；LeaseProjection 崩溃恢复机器 app 零接线；`CredentialPicker` / `BudgetCap` / `PolicyGate::Retention` 变体 |
| B4 | workspace+app | prompt-template / 未落地 ResourceKind 集群（`ResourceSelection.prompt_template/prompt_arguments`、`ResourceLimits.max_template_file_refs/max_rendered_prompt_bytes`、`ResourceKind::{PromptTemplate,LanguageServer,UserHook}`、`ResourceInstructionKind::PromptTemplate` app 穷举死臂）——删除跨 workspace + app |
| B5 | models | 四个零消费者 Spec 记录 API：`validate_context` / `merge_provider_source` / `capability_snapshot` / `filter_by_purpose` |
| B6 | auth | `ApiKeyCredential::store/store_with_scopes/delete`、`StoredCredential::with_expires_at`、`DeviceAuthorization` 中转结构、`keychain_*` serde alias 版本期保留面 |
| B8 | auth+workspace | `locator::is_mcp_secret_service` 消费收敛收尾：workspace 前缀副本彻底收敛（跨包依赖方向，见 backlog RV-2026-10-B） |
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

条件性工程项 RV-2026-10-A / RV-2026-10-B 的触发条件见 [backlog](spec/backlog.md#8-条件性工程项)。

## 等待人工验收

以下功能已实现，自动门禁通过；用户验收与消费者接入仍独立跟踪。

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
| Plan GUI | 完整键盘矩阵待验 |
| 技能录制 | 录制产物在真实后续 Run 中的资源发现待验 |
| 任务消耗 | [统计窗口](spec/task-usage-ui.md#7-生产界面)用户验收；MoMai 作品 / 分部 / 章节与 YingMai 作品 / 分段 / 操作类型的消费者接入及 UI 验收 |
| 模型用途筛选 | Composer 用途菜单与 Settings 能力徽标的用户验收；契约见 [模型网关](spec/model-gateway.md#决策adr-064-模型能力用途筛选2026-10-01) |
| 网关消费者 | MoMai 设置成功保存凭证及媒体 GUI 验收；消费者进度见 [MoMai ROADMAP](../../MoMai/docs/ROADMAP.md) |

## 缺环境与外部条件

条件具备后验收真实效果；mock 或本机测试不能替代对应环境。

| 事项 | 待办 |
| --- | --- |
| 账号与额度 | 真实 OAuth/刷新、双账号切换、非零权威读数、倒计时与过期 |
| 跨平台 | Linux/Windows、WebKit、容器与真实 Provider 扩展矩阵；Windows 实例锁（share_mode）、listener close/connect 唤醒与连接回收同形态验证 |
| MM-2 视频真实往返 | 支持端点与账号的专项环境 |
| MM-3 GLM 搜索 | GLM 搜索 MCP 当前未配置 |
| Chrome / Edge 上下文 | 浏览器返回错误 12（Apple Events JavaScript 未启用），用户手动开启并满足 Automation 权限后复验正文读取 |
| 技能录制非 Unix 写入 | 安全目录写入原语未在非 Unix 实现，入口明确返回 Unsupported |
| 网关媒体与账单 | 真实图像 / 视频 / 搜索供应商往返、套餐权限及账单来源；模拟上游不作为真实扣费证据 |

库激活前置、按测量触发的技术项和阶段外产品候选见 [backlog](spec/backlog.md)；发布与全量门禁仍待另行授权（BK-RELEASE-01）。
