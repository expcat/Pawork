# CU-11 · 图像能力的前置工具门控

> 状态：已实现；自动回归通过，等待人工验收（P3）。前置：[CU-02](cu-02-target-contracts.md)。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

在工具暴露和执行前拒绝不支持图像的模型，避免截屏后才失败。

## 当前依据与读取范围

现有协商会扫描工具消息中的图像；文本模型可能先看到 computer 工具，截图进入下一轮请求后才被拒绝。

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

- 门控落点：`crates/app/src/services/run.rs` 的 `chat_turn_with_run_id`——工具定义组装前按当前 Provider 的三源能力证据（与 VISION-1/SEARCH-1 前置闸门同一份；未知模型按空证据 fail-closed）判定 `image_input`，未声明时 `ComputerUse` 标签工具（computer）不进本轮工具定义与派发允许表。模型臆造的 computer 调用由既有 loop_ctx 允许表在调度器之前拒绝（"this model cannot use this tool"），桌面后端访问次数为零。GUI / CLI / 子代理 child run 同走该入口，能力裁决只在 app/models，不按 Provider 名称分支。
- 错误语义：`crates/models/src/negotiate.rs` 的 `image_input` 拒绝理由携带模型 id（model `X` does not declare image input capability），切模型 / 恢复已有图像上下文时指明实际缺失，不误报后端断开。
- 视觉模型不受影响：声明 `image_input` 的模型照常暴露 computer 并沿用既有三协议图像编码；嵌套工具图像回归保留。
- 与 B5 协调：仅复用既有公开面 `ModelRegistry::capability_evidence` + `CapabilityEvidence::merged()`，未新增或收窄任何 B5 待裁决 API（`validate_context` / `merge_provider_source` / `capability_snapshot` / `filter_by_purpose` 均未触碰）。
- 测试配套：`crates/testkit` 的 `MockProviderCallRecord` 增加 `tool_names`（断言请求实际暴露的工具名）；`chat_controls` 的 computer 持久化回归改为声明 `image_input` 的 mock 模型（新契约下视觉操作须视觉模型）。

**实际命令与结果**

- `cargo check --offline --tests -p pawork-app …`（带九通道 feature）：通过。
- `bash scripts/test.sh --log /tmp/cu11-test.log models providers tools app testkit`：606 passed / 0 failed / 1 ignored（既有 opt-in 网络测试），13 个测试目标全绿。
- `bash scripts/test.sh --log /tmp/cu11-test-downstream.log gui-server pawork`：通过（下游 Run 消费方回归）。

**定向回归**

- `services::run::tests::vision_gate_withholds_computer_and_never_dispatches_for_text_models`：文本模型请求不含 computer 定义；臆造调用收到明确拒绝；MockTool 零调用＝桌面后端零访问。
- `services::run::tests::vision_gate_keeps_computer_tool_for_vision_models`：视觉模型照常暴露 computer。
- `services::run::tests::capability_gate_rejects_undeclared_web_search_and_image`（扩充）：图片拒绝错误含模型 id 与能力名，Provider 零调用。
- 既有回归保留通过：`negotiate::tests::capability_gate_rejects_nested_tool_result_image_without_declaration`（models）、`request::tests::tool_result_images_map_across_chat_responses_and_anthropic`（providers）、`gui_host::tests::chat_controls::computer_approval_image_persistence_and_resume_do_not_repeat_input`（app）、tools `computer::tests` 全组。

**复审修复（2026-10-07，sol 审查 2 项）**

- P2：目录发现成功且最终证据升级 `image_input` 时，`chat_turn_with_run_id` 按最终证据重建本轮工具定义与派发允许表（发现只发生在空证据之后，无降级路径）——启动时缺当前 Provider 证据的视觉模型当轮即可用 computer；`startup_model_discovers_current_provider_image_capability` 扩充断言请求保留 computer。
- P3：`docs/spec/crates/testkit.md` 的 `MockProviderCallRecord` 字段清单补齐 `tool_names` 及记录请求工具名的语义。
- 修复后复跑 `bash scripts/test.sh --log /tmp/cu11-test-fix.log models providers tools app testkit`：606 passed / 0 failed / 1 ignored，13 个测试目标全绿（含扩充后的启动发现回归）。

**真实 / 人工结果**

未执行（本任务为离线实现轮）。待人工项：真实 Host + 文本模型（如 glm-5.3）确认工具列表无 computer 且零桌面连接尝试；真实视觉模型 + compose desktop（按验证规格 `--provider opencode-go --model glm-5.3-flash`）确认截图链路照常。人工验收完成后再将 ROADMAP 标为 ✅。

**遗留缺口**

无已知实现缺口；人工验收未完成（见上），ROADMAP 如实标注「自动回归通过，等待人工验收」。
