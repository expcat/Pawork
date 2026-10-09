# CU-12 · 请求侧截图历史与上下文预算

> 状态：已实现；当前定向回归与模拟验收通过；独立 Sol max review PASS（P3）。前置：[CU-10](cu-10-action-observe.md)、[CU-11](cu-11-model-vision-gate.md)、[B10](b10-engine.md)。更新：2026-10-09。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

限制发给模型的旧截图，保留完整持久化证据和历史重放。

## 当前依据与读取范围

当前 `run_session` 已在请求组装侧调用 `trim_observation_images`，只裁剪请求副本中的旧工具图像；通用分级裁剪函数 `trim_tool_result` 仍没有生产消费者。Chat / Responses 会丢弃 Artifact 类型的图像来源，因此本项保留 Base64，裁掉的图像原位换文本说明。

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

按 B10 裁决，在 `tool_result_trim` 模块内新增专用的 `trim_observation_images`，并接入唯一必要生产路径——`run_session` 请求组装侧（每轮请求前、输入估算之前）。这不表示通用函数 `trim_tool_result` 已接线。

- `crates/engine/src/context/tool_result_trim.rs`：新增请求侧观测图像裁剪。`trim_observation_images(&mut [Message], retained_image_bytes)` 只处理 `Tool` 消息内 `ToolResultContent` 的 `ImageSource::Base64` 图像（递归入口只从顶层 `ToolResult` 进入）：最后一条含图像的 `Tool` 消息（最新观测）全部图像无条件保留、不占预算；更早图像从新到旧在字节预算内保留；被裁剪图像原位替换为 `[image trimmed: earlier tool result image (…, ~N bytes) removed from this request …; the full image is retained in session history]` 文本说明，同一 tool result 的文本部分（观测 metadata）原样保留。不使用 `ArtifactRef` 占位（Chat / Responses 会丢弃 Artifact 类型图像来源）；用户附图、`Url` / `Artifact` 来源与 `Tool` 消息顶层直接挂的图像均不动（生产循环把工具结果统一包装为 `ToolResult`，顶层图像既不裁剪也不参与最新观测判定——复审 P3 修复）。`observation_image_budget_bytes(&ContextBudget)`：旧观测保留预算 = `max_input_tokens` × 4 字符/token（与 `HeuristicEstimator` 默认口径一致）——token 估算器对图像只计 85 placeholder token，反映不了 base64 在请求体中的真实体量，图像单独按字节设限，随模型窗口缩放。
- `crates/engine/src/tool_loop/mod.rs`：`run_session` 每轮在 `estimate_input` 前执行裁剪，仅当 `TurnContext.limits` 已配置（沿用上下文管理 opt-in 口径，`TurnContext::default()` 行为不变）；有裁剪时发 `Diagnostic { code: "context.observation_images_trimmed" }`（trimmed / retained 图像数与字节 + 预算）留痕。裁剪只改请求副本：`MessageCommitted` 事件、数据库消息与可审阅图像不裁剪，重放（事件投影）不受影响、不变成再次截屏或输入。app 的 `turn_context()` 对已知窗口模型本就配置 limits，GUI / CLI / 子代理 run 同走该唯一生产入口，无需 app 侧改动。
- 维持既有 Base64 图像编码，未引入 Artifact 引用替代实际图像。

### 实现轮实际命令与结果（2026-10-07）

- `bash scripts/test.sh engine`：66 lib + `domain_only` / `no_provider_branch` 红线各 2 个全部通过（exit 0），日志 `/tmp/cu12-engine.log`。
- `bash scripts/test.sh providers tools`：全绿（exit 0），日志 `/tmp/cu12-prov-tools.log`；含既有回归 `request::tests::tool_result_images_map_across_chat_responses_and_anthropic`（三协议工具结果图像编码）与 `computer::tests` 全组。
- `bash scripts/test.sh app`：277 lib + 其余目标全绿（exit 0），日志 `/tmp/cu12-app.log`；含 `computer_approval_image_persistence_and_resume_do_not_repeat_input`（持久图像 + 恢复不重复输入）与 CU-11 vision gate 三例。

### 实现轮新增定向回归

- `tool_loop::tests::observation_images_trimmed_in_request_copy_but_persisted_in_events`（主路径）：三轮工具图像循环（A/B/C 各 2_000 base64 载荷字节、旧图预算 3_000），断言第 4 轮请求只带最新观测 C 与预算内旧图 B（共 4_000 字节，低于未裁剪的 6_000）；最新观测不占旧图预算，不能据此声明总图像载荷 ≤3_000。A 原位变文本说明且旧动作文本保留、Diagnostic 留痕一次（trimmed=1）、三条已提交工具事件仍带原始图像（事件源完整）、第 2 轮请求当时最新观测 A 原样可用。用例中的载荷不是实际 JPEG，仅证明数量、字节和留存。
- `tool_loop::tests::observation_images_untouched_without_context_limits`（关键关闭路径）：`TurnContext::default()` 不裁剪，三图全部随请求发出、无 Diagnostic。
- `context::tool_result_trim` 内联四例：最新观测保留 + 旧图（含嵌套 tool result）换说明、预算内保留边界、用户附图 / Url 来源 / Tool 消息顶层图像不动（顶层图像位置更靠后也不改变最新观测判定）、预算随 `max_input_tokens` 折算。

### 真实 / 人工结果

2026-10-07 实现轮未执行真实 Provider / Desktop 验收。2026-10-09 按用户授权以模拟完成请求预算、事件留存与持久恢复的可验证部分，证据见下；没有真实视觉模型、GPUI 看图或真实桌面截图，不登记为像素验收或真实平台通过。

### 遗留缺口

无已知实现缺口；本轮模拟与独立 review 已通过。已知边界：`limits` 未配置（模型无注册条目或窗口为 0）时不裁剪；旧图预算不限制最新观测的体积；真实模型与图像审阅没有本轮证据。

## 本轮模拟验收（2026-10-09）

Grok xhigh 对话 `01a12143-9739-7772-9ce2-179147370a0e` 在 `main@53c2e70d` 交付源码 no-op，无新增测试。旧 Engine 全包日志不能单独证明最终 `exec.rs` 版本，本轮只重跑两个既有 `run_session` 用例，2 passed / 65 filtered / exit 0。

| 实际输入 | 产品调用与可观察结果 | 证据范围 |
| --- | --- | --- |
| 四轮模拟 Provider，工具回 A/B/C 各 2_000 Base64 载荷字节，旧图预算 3_000 | 实际 `run_session` 第 2 轮 A、第 3 轮 A+B、第 4 轮 B+C；A 换说明但 `observation shot_a` 留存，Diagnostic 一次；三条 Tool `MessageCommitted` 仍各含原始载荷 | 当前 Engine 两用例；事件出口完整，不等于同一用例已写 SQLite |
| 同一输入，`TurnContext::default()` | 第 4 轮仍带 A+B+C，无裁剪 Diagnostic | 当前关闭路径 |
| GUI `RunStart`、SQLite、审批后替换的 MockTool 回 `/9j/2Q==`，模型窗口为 0 | 实际 Host 审批与事件持久化，`replay_events` 留存载荷，`resume_messages_keep_pending` 后工具调用数仍为 1 | 复用 CU-16 同版持久/恢复用例；不配置图像预算，不证明裁剪 |
| 两条工具结果图像 `aaa` / `bbb` | Chat 编成下一条 user 的 data URL，Responses 放在 `function_call_output` 的 `input_image`，Anthropic 放在 `tool_result` 的 image source | 复用同版三协议编码用例；不是网络往返或像素验收 |

上述载荷均不证明真实 JPEG 像素。旧图预算 3_000 时 B+C 共 4_000 字节是预期行为，最新观测不占预算。请求裁剪、SQLite 持久恢复与协议编码是分别验证的边界，未将其写成同一条真实桌面闭环。

证据目录 `/tmp/cu12-20261009/`：`01-engine-observation.log` 为 451 bytes / SHA256 `711a3ca389bd616b6065542cc43373178353fc9284640e0b8398c0dcfe2ebdb8`，独立 `.exit` 为 0；`00-source-identity.txt` 与 `00-cu16-fingerprint-check.txt` 保留源码身份与复用核对。当前 `exec.rs` blob 为 `12c2b89429bc0a42bc26a938f551731f5c25cff4`，专用裁剪源码自实现以来未改。复用 `/tmp/cu16-20261009/01-app-product-chain.log` 中对应用例（2311 bytes / SHA256 `844e5a7e3acaf892afd8d15c77793765977dae9e42fd7dbf64ef32b1fdafd34a`，`EXIT:0`）；App / storage / engine 自 `5b97194b` 至本轮 HEAD 无差异。该用例走 MockTool，不使用本轮前已改动的 NativeHost / target / computer_native 实现。三协议只复用 `/tmp/cu12-prov-tools.log` 的对应目标（22602 bytes / SHA256 `f3944f125f9904b3727057909ebc912089584eba9fb15e82b857992fc4a01ebd`）；编码源码自 `5720008b` 至本轮 HEAD 无差异，不把历史全包数计作当前门禁。

```text
Validated: cargo test --offline --lib -p pawork-engine -- observation_images_trimmed_in_request_copy_but_persisted_in_events observation_images_untouched_without_context_limits（2 passed / 65 filtered / exit 0）；指挥追加 git diff --check 与文档相对链接检查
Targeted regressions: 两个当前 run_session 用例；App 持久/恢复与 Provider 三协议编码仅同版复用，未重跑；无新增测试
Full workspace gate: NOT RUN
```

独立 `gpt-6.1-sol / max` 对话 `01a12151-00e2-7f60-8c90-043932eb2180`、turn `01a12151-08d9-7bb0-8e6c-208732d52908` 返回 PASS，无必须修项或非阻断建议。指挥核对受审 diff 16_429 bytes / SHA256 `8afc79b94579e6620716b54a13394933bc0db61ae7a13caabe3eb54b16d7bf15` 未变后，只补审查与完成状态元数据并本地提交。
