# CU-12 · 请求侧截图历史与上下文预算

> 状态：已实现；自动回归通过，等待人工验收（P3）。前置：[CU-10](cu-10-action-observe.md)、[CU-11](cu-11-model-vision-gate.md)、[B10](b10-engine.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

限制发给模型的旧截图，保留完整持久化证据和历史重放。

## 当前依据与读取范围

旧截图持续留在工具历史中，`tool_result_trim` 未接生产循环；Chat / Responses 当前会丢弃 Artifact 类型的图像来源，不能只替换 ImageSource。

[engine Spec](../spec/crates/engine.md)、[models Spec](../spec/crates/models.md)、[storage Spec](../spec/crates/storage.md)、[providers Spec](../spec/crates/providers.md)。

## 实施范围

- 先与 [B10](b10-engine.md) 完成工具结果裁剪归属裁决，可同批处理；只接一条必要生产路径。
- 在请求副本按现有预算保留最新或当前任务必需的观测，保留旧动作文本与图像存在的明确说明。
- 数据库消息、事件和可审阅图片不裁剪；持久重放不变成再次截屏或输入。
- 维持已支持的 Base64 图像编码，确需 Artifact 时另证实完整解析链路，不用引用替代实际图像。

## 完成条件与验证

- 代表性多步任务的模型请求图像数量/字节受控，最新观测仍可用。
- 同一任务持久记录与重放证据完整，模型上下文裁剪不改变历史。
- 现有工具图像、Provider 编码与持久化回归通过，不新增完整上下文框架。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

**已实现（2026-10-07）**，基线 5720008b。

### 实现

按 B10 裁决把 `tool_result_trim` 接入一条必要生产路径——`run_session` 请求组装侧（每轮请求前、输入估算之前），只接这一条。

- `crates/engine/src/context/tool_result_trim.rs`：新增请求侧观测图像裁剪。`trim_observation_images(&mut [Message], retained_image_bytes)` 只处理 `Tool` 消息内 `ToolResultContent` 的 `ImageSource::Base64` 图像（递归入口只从顶层 `ToolResult` 进入）：最后一条含图像的 `Tool` 消息（最新观测）全部图像无条件保留、不占预算；更早图像从新到旧在字节预算内保留；被裁剪图像原位替换为 `[image trimmed: earlier tool result image (…, ~N bytes) removed from this request …; the full image is retained in session history]` 文本说明，同一 tool result 的文本部分（观测 metadata）原样保留。不使用 `ArtifactRef` 占位（Chat / Responses 会丢弃 Artifact 类型图像来源）；用户附图、`Url` / `Artifact` 来源与 `Tool` 消息顶层直接挂的图像均不动（生产循环把工具结果统一包装为 `ToolResult`，顶层图像既不裁剪也不参与最新观测判定——复审 P3 修复）。`observation_image_budget_bytes(&ContextBudget)`：旧观测保留预算 = `max_input_tokens` × 4 字符/token（与 `HeuristicEstimator` 默认口径一致）——token 估算器对图像只计 85 placeholder token，反映不了 base64 在请求体中的真实体量，图像单独按字节设限，随模型窗口缩放。
- `crates/engine/src/tool_loop/mod.rs`：`run_session` 每轮在 `estimate_input` 前执行裁剪，仅当 `TurnContext.limits` 已配置（沿用上下文管理 opt-in 口径，`TurnContext::default()` 行为不变）；有裁剪时发 `Diagnostic { code: "context.observation_images_trimmed" }`（trimmed / retained 图像数与字节 + 预算）留痕。裁剪只改请求副本：`MessageCommitted` 事件、数据库消息与可审阅图像不裁剪，重放（事件投影）不受影响、不变成再次截屏或输入。app 的 `turn_context()` 对已知窗口模型本就配置 limits，GUI / CLI / 子代理 run 同走该唯一生产入口，无需 app 侧改动。
- 维持既有 Base64 图像编码，未引入 Artifact 引用替代实际图像。

### 实际命令与结果

- `bash scripts/test.sh engine`：66 lib + `domain_only` / `no_provider_branch` 红线各 2 个全部通过（exit 0），日志 `/tmp/cu12-engine.log`。
- `bash scripts/test.sh providers tools`：全绿（exit 0），日志 `/tmp/cu12-prov-tools.log`；含既有回归 `request::tests::tool_result_images_map_across_chat_responses_and_anthropic`（三协议工具结果图像编码）与 `computer::tests` 全组。
- `bash scripts/test.sh app`：277 lib + 其余目标全绿（exit 0），日志 `/tmp/cu12-app.log`；含 `computer_approval_image_persistence_and_resume_do_not_repeat_input`（持久图像 + 恢复不重复输入）与 CU-11 vision gate 三例。

### 定向回归（新增）

- `tool_loop::tests::observation_images_trimmed_in_request_copy_but_persisted_in_events`（主路径）：三轮截图循环（每图 2_000 base64 字符、预算 3_000），断言第 4 轮请求只带最新观测 C 与预算内旧图 B（4_000 字节 < 未裁剪 6_000）、A 原位变文本说明且旧动作文本保留、Diagnostic 留痕一次（trimmed=1）、三条已提交工具事件仍带原始图像（持久记录完整、重放源不变）、第 2 轮请求当时最新观测 A 原样可用。
- `tool_loop::tests::observation_images_untouched_without_context_limits`（关键关闭路径）：`TurnContext::default()` 不裁剪，三图全部随请求发出、无 Diagnostic。
- `context::tool_result_trim` 内联四例：最新观测保留 + 旧图（含嵌套 tool result）换说明、预算内保留边界、用户附图 / Url 来源 / Tool 消息顶层图像不动（顶层图像位置更靠后也不改变最新观测判定）、预算随 `max_input_tokens` 折算。

### 真实 / 人工结果

未执行（本任务为离线实现轮，同 CU-11 口径）。待人工项：真实 Host + 视觉模型（按验证规格 `--provider opencode-go --model glm-5.3-flash`）+ compose desktop 跑代表性多步 computer-use 任务，确认请求侧图像数量 / 字节受控、最新观测可用、`context.observation_images_trimmed` 诊断出现，且时间线 / 持久记录中历史截图仍可审阅。人工验收完成后再将 ROADMAP 标为 ✅。

### 遗留缺口

无已知实现缺口；人工验收未完成（见上）。已知边界：`limits` 未配置（模型无注册条目或窗口为 0）时不裁剪，与上下文管理 opt-in 口径一致。
