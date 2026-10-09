# CU-11 · 图像能力的前置工具门控

> 状态：已实现；自动回归与本轮模拟验收通过；独立 Sol max review PASS（P3）。前置：[CU-02](cu-02-target-contracts.md)。更新：2026-10-09。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

在工具暴露和执行前拒绝不支持图像的模型，避免截屏后才失败。

## 当前依据与读取范围

当前 app/models 已在工具定义与派发允许表组装前检查 image input 能力，文本模型不暴露 computer / computer_native。工具消息图像仍经既有协商；本轮核正式 AppCore / Engine 的过滤、明确拒绝与能力升级证据。

[models Spec](../spec/crates/models.md)、[app Spec](../spec/crates/app.md)、[tools Spec](../spec/crates/tools.md)、[providers Spec](../spec/crates/providers.md)。

## 实施范围

- 复用现有模型能力和工具 registry/scheduler，在暴露或派发视觉操作前检查 image input 能力。
- 兼顾切模型和恢复已有图像上下文，错误应指出实际能力缺失，而非报告后端断开。
- 能力裁决留在现有 app/models 责任范围；Engine 不按 Provider 名称分支，不增加具体 Provider 依赖。
- 与 [B5](b5-models.md) 协调公开 API 裁决，不新造一套能力目录。

## 完成条件与验证

- 文本模型在截图、输入或连接后端前被正确门控，后端访问次数为零。
- 支持图像的模型仍走现有三协议图像编码，嵌套工具图像的既有回归保留。
- 使用真实功能模型时按验证规格临时覆盖，缺能力或连接失败如实记录。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

**已实现（2026-10-07）**

- 门控落点：`crates/app/src/services/run.rs` 的 `chat_turn_with_run_id`——工具定义组装前按当前 Provider 的三源能力证据（与 VISION-1/SEARCH-1 前置闸门同一份；未知模型按空证据 fail-closed）判定 `image_input`，未声明时 `ComputerUse` 标签工具（computer / computer_native）不进本轮工具定义与派发允许表。模型臆造的 computer 调用由既有 loop_ctx 允许表在调度器之前拒绝（"this model cannot use this tool"），不调用工具执行器。通过能力闸的 Run 仍登记并关闭内存 target scope；此操作不绑定窗口或截图，不是桌面连接。GUI / CLI / 子代理 child run 同走该入口，能力裁决只在 app/models，不按 Provider 名称分支。
- 错误语义：`crates/models/src/negotiate.rs` 的 `image_input` 拒绝理由携带模型 id（model `X` does not declare image input capability），切模型 / 恢复已有图像上下文时指明实际缺失，不误报后端断开。
- 视觉模型不受影响：声明 `image_input` 的模型照常暴露 computer 并沿用既有三协议图像编码；嵌套工具图像回归保留。
- 与 B5 协调：仅复用既有公开面 `ModelRegistry::capability_evidence` + `CapabilityEvidence::merged()`，未新增或收窄任何 B5 待裁决 API（`validate_context` / `merge_provider_source` / `capability_snapshot` / `filter_by_purpose` 均未触碰）。
- 测试配套：`crates/testkit` 的 `MockProviderCallRecord` 增加 `tool_names`（断言请求实际暴露的工具名）；`chat_controls` 的 computer 持久化回归改为声明 `image_input` 的 mock 模型（新契约下视觉操作须视觉模型）。

**实际命令与结果**

- `cargo check --offline --tests -p pawork-app …`（带九通道 feature）：通过。
- `bash scripts/test.sh --log /tmp/cu11-test.log models providers tools app testkit`：606 passed / 0 failed / 1 ignored（既有 opt-in 网络测试），13 个测试目标全绿。
- `bash scripts/test.sh --log /tmp/cu11-test-downstream.log gui-server pawork`：通过（下游 Run 消费方回归）。

**定向回归**

- `services::run::tests::vision_gate_withholds_computer_and_never_dispatches_for_text_models`：文本模型请求同时不含 computer / computer_native 定义；臆造 computer 调用在 scheduler 前收到明确拒绝，MockTool 计数为零；这不等于真实 NativeHost / AX / RFB 探测零访问。
- `services::run::tests::vision_gate_keeps_computer_tool_for_vision_models`：视觉模型照常暴露 computer。
- `services::run::tests::capability_gate_rejects_undeclared_web_search_and_image`（扩充）：图片拒绝错误含模型 id 与能力名，Provider 零调用。
- 既有回归保留通过：`negotiate::tests::capability_gate_rejects_nested_tool_result_image_without_declaration`（models）、`request::tests::tool_result_images_map_across_chat_responses_and_anthropic`（providers）、`gui_host::tests::chat_controls::computer_approval_image_persistence_and_resume_do_not_repeat_input`（app）、tools `computer::tests` 全组。

**复审修复（2026-10-07，sol 审查 2 项）**

- P2：目录发现成功且最终证据升级 `image_input` 时，`chat_turn_with_run_id` 按最终证据重建本轮工具定义与派发允许表（发现只发生在空证据之后，无降级路径）——启动时缺当前 Provider 证据的视觉模型当轮即可用 computer；`startup_model_discovers_current_provider_image_capability` 扩充断言请求保留 computer。
- P3：`docs/spec/crates/testkit.md` 的 `MockProviderCallRecord` 字段清单补齐 `tool_names` 及记录请求工具名的语义。
- 修复后复跑 `bash scripts/test.sh --log /tmp/cu11-test-fix.log models providers tools app testkit`：606 passed / 0 failed / 1 ignored，13 个测试目标全绿（含扩充后的启动发现回归）。

**真实 / 人工结果**

历史离线实现轮未执行真实 Provider / Desktop 验收。本轮按用户授权模拟 Provider 边界，直接驱动正式 AppCore → Engine → 派发允许表，结果见下。真实账号、系统图像权限与平台连接单列待验，不以模拟声明真实桌面通过。

**遗留缺口**

无已知实现缺口；本轮模拟与独立 review PASS。真实 Provider、真窗口截图与系统权限未执行。


## 本轮模拟验收（2026-10-09）

Grok xhigh 对话 01a1212b-d96b-7d93-9011-cd598d3c086f 在 main@04c1ed22 交付源码 no-op，无新增测试。由于历史全包日志早于 CU-10 的 run.rs 修改，本轮重跑四条现有 AppCore 回归；首次多过滤器放在 Cargo 的 -- 前造成参数错误（exit 1），失败日志 01 保留，修正参数后的日志 02 为 exit 0。

实际输入与结果：文本模型 mock-text 不声明 image_input，两轮 Provider 请求均不含 computer / computer_native，臆造 computer screenshot 在 scheduler 前由允许表拒绝，事件保留 this model cannot use this tool，替换的 MockTool 计数为零。视觉模型 mock-vision 的请求断言包含 computer；computer_native 由同一 ComputerUse 谓词保留的源码可见，但正路径用例没有直接断言该名称，不把它记为单独运行验证。

当前带图消息的 glm-5.2 在 mock Provider 下不能借用 glm-coding 的同名静态声明；有界 list_models 发现后仍缺能力，在 Engine / 内存 scope 登记之前返回 AppError::Provider，文案含模型 id 与 image input，不误报断线。启动 glm-5.3-flash 目录发现升级 image_input 后，当轮工具表重建，请求携图且含 computer，Provider 恰调用一次。恢复 history 的 CLI 调用同一入口由源码确认，本轮没有单独执行持久化恢复的门控用例。

三协议工具结果图像与嵌套图像拒绝复用 /tmp/cu11-test-fix.log 中对应测试成功记录；models negotiate.rs 与 providers request.rs 的 blob 和历史 a4978e71 一致。此复用只覆盖这两个未变目标，不把历史全包数当作当前 AppCore 的验证。

证据 /tmp/cu11-20261009/ 保留原始命令及退出码、源码 identity、失败和修复日志、完整交付。02-app-vision-gate.log 为 1632 bytes / SHA256 af0ef7596f3b0677dc93c4824034a9d77405695c9d8352403dfa5082388eeed8。当前 run.rs blob cb17bab8f0503a86c034d134ee3ff1c7e21603c4，与 CU-10 提交 20646a75 一致。

Validated: cargo test --offline --lib -p pawork-app --features pawork-app/ui-fixture -- vision_gate capability_gate_rejects_undeclared_web_search_and_image startup_model_discovers_current_provider_image_capability，4 passed / 303 filtered / exit 0；指挥追加 git diff --check 与文档相对链接检查。
Targeted regressions: 当前重跑文本工具过滤/臆造调用拒绝、视觉工具保留、缺能力图像/搜索拒绝、启动发现升级四项；三协议与嵌套图像拒绝仅按同版日志复用。
Full workspace gate: NOT RUN。

环境边界：没有真实 Provider、Desktop 窗口、Accessibility / Screen Recording 授权或隔离桌面连接；未改 auth/config/disabled_models。独立 gpt-6.1-sol / max 对话 01a12137-ee0e-70d0-9109-2b7c16ed344a、turn 01a12137-f1ce-7333-a8c1-bb2a73825bfb 返回 PASS，无必须修项；指挥提交。
