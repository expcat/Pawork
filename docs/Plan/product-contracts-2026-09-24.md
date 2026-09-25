# 本轮新增产品契约草案

> 2026-09-24；用户已将全部未排期能力纳入本轮。本文列明需要新增的冻结契约，用户在草案提出后回复“继续”，按本草案推进，golden 先行。既有修复、文件夹/浏览器文本附件与来源链接不依赖新增契约。

## 1. 兼容规则

在 tasks_cancel 的 API 1.23 之后新增 GUI API 1.24。旧客户端继续协商旧版本，不接受新命令；新客户端连接旧 Host 时隐藏/禁用新入口并说明版本要求。已有事件、命令和响应字段不改名、不删除；新增可选字段缺省保持旧语义。

## 2. 视频引用

Canonical `ContentPart` 增加 `Video(VideoContent)`：

```json
{"type":"video","data":{"url":"https://example.com/clip.mp4","media_type":"video/mp4"}}
```

首版只接 HTTP(S) URL，拒绝内嵌凭据、file/data URL 和本地路径；Host 不下载、转码或抽帧。`ModelCapabilities.video_input` 缺省 false，模型列表增加同名缺省 false 的字段。能力需同时满足模型与实际 endpoint 支持，不能由图片能力推导。

`RunStart` 增加可选 `video_urls: [{url,media_type}]`（缺省空、省略序列化），每轮最多 4 条附件/视频合计。Desktop 输入 URL、展示来源、允许移除，CLI 增加重复 `--video-url`。未支持通道在发请求前明确拒绝；支持通道按其官方 `video_url` wire 转换，不伪装为图片或文本。持久化保留 canonical 引用，重放展示相同来源。

## 3. Plan GUI

新增查询 `plan_get {session_id}`，Data 为现行 PlanSnapshot 或 null。新增命令：

- `plan_save {session_id,title,steps,expected_version?}`：没有 Plan 时创建；已有 Plan 时必须匹配版本后修订。
- `plan_submit {session_id,expected_version}`。
- `plan_approve {session_id,expected_version}`。
- `plan_reject {session_id,expected_version,reason}`。

Data 返回最新 PlanSnapshot；版本过期、会话不存在、会话执行中或非法状态转移明确返回错误，不覆盖新版本。标题上限 256 字节，1–64 步、每步最多 2048 字节。沿用现行 PlanEvent、历史与批准 gate；批准不扩大工具权限。Desktop 面板提供查看、编辑、提交、批准、拒绝，显示真实版本和状态。

## 4. 持续目标

新增查询 `goal_get {session_id}`，Data 为当前目标快照或 null。新增命令：

- `goal_start {session_id,title,criteria,budget_tokens,max_runs}`：标题/标准非空，1–16 条标准，显式给出正整数预算与轮数（最多 100 轮）。同会话仅一个活动目标。
- `goal_pause {session_id,goal_id}`：取消当前 Run，等待其终态后暂停，不继续开新轮。
- `goal_resume {session_id,goal_id,budget_tokens,max_runs}`：显式追加预算，恢复前重新计算已用量。
- `goal_steer {session_id,goal_id,input}`：保存方向修正，下轮使用；不增加预算。
- `goal_finish {session_id,goal_id,outcome,reason?}`：outcome 为 achieved / abandoned。达成由用户确认；Agent 的普通回答不等于人审标准达成。

沿用 GoalEvent 的 created/paused/resumed/steered/achieved/abandoned/progress_updated/criterion_satisfied；Created 附加可选 budget_tokens/max_runs，Resumed 附加可选 max_runs。缺少预算的旧事件只读恢复，不自动执行。每轮沿用正式 RunStart 执行链、审批/Plan gate、用量账本、取消和终态持久化；一个目标连续串行运行，预算耗尽、轮数耗尽、运行失败或 Host 退出即暂停。关闭窗口不停止 Host 中的目标；Host 重启只恢复快照，须用户显式恢复才继续。目标快照显示状态、已用/剩余预算、轮数、当前 Run 与暂停原因。

## 5. 技能录制与插件列表

技能录制先从当前会话选取操作，生成可编辑的 SKILL.md 预览；用户保存后由 Host 使用 workspace_id + relative_path 写入当前工作区 `.pawork/skills/<name>/`，配套现行 manifest.toml。名称只接受小写字母、数字、短横线，拒绝已有目录、路径逃逸和 Secret 内容。新增 `skill_record_preview {session_id,event_ids}` 查询与 `skill_record_save {workspace_id,name,description,content}` 命令；不录制键盘密码，不自动执行录制结果。

启用已有 `plugin_list` 查询：返回实际安装记录及能力和状态，不把 MCP 服务器冒充插件。当前无生产插件运行时，空列表明确显示“尚未安装插件”，并展示现有技能/MCP 的独立入口；本轮不新增插件运行时或市场。

绘图按 Composer 手绘画布实现，产物为图片附件；沿用现行上传、模型能力检查、预览和持久化路径，不改 wire。若用户明确要求 AI 生图，另按真实 Provider 的图像生成契约实施。
