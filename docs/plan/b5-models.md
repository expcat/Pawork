# B5 · Models 四个预留 API 裁决

> 状态：✅ 完成（裁决、实现、定向验证与独立 review PASS）。更新：2026-10-08。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

核实模型库四个 Spec 记录 API 的使用情况并裁决。

## 当前线索与读取范围

登记项为 `validate_context` / `merge_provider_source` / `capability_snapshot` / `filter_by_purpose`；原 ROADMAP 标为零消费者。

[models Spec](../spec/crates/models.md)、[模型网关](../spec/model-gateway.md)。

## 实施范围

- 重新核对生产调用、公开用途和测试锚点，逐项决定保留、收窄或删除。
- 与 [CU-11 模型图像门控](cu-11-model-vision-gate.md) 的消费需求对齐，避免清理后再造同一能力。

## 完成条件

- API 与 Spec 同步，没有破坏模型目录、能力协商或用途筛选。
- 只验证实际受影响行为，不为保留接口补镜像测试。

## 完成记录

### 基线与消费核实（2026-10-08）

本轮起点为 `main / 62ba363535cc2d66cb365ae932d92e3fd95d3d83`，工作区干净；B1–B4 不重做。先读本任务、models Spec、CU-11、模型网关用途契约与架构 §3，用 CodeGraph 定位后补齐未覆盖的限定路径调用、内联测试与文档引用。全仓 Rust 引用核实的结果为：四个登记入口均无生产调用，但各自承担的目录 / 能力 / 用途语义已有生产入口，不能连同包装口一起删除。

| 登记 API | 当前消费与公开用途依据 | 裁决与保留的实际入口 |
| --- | --- | --- |
| `validate_context` | 仅 `context_validation_respects_window` 测试调用；实现只是 `resolve` 后比较输入 token 与窗口。CLI / Host 没有接入这个布尔校验。 | 删除方法及只验证该包装的测试；保留 `resolve`、`CatalogEntry::context_window_tokens` / `max_output_tokens` 与现有预算消费者，不新增上下文检查。 |
| `merge_provider_source` | 仅目录合并回归调用；`ProviderCapabilitySource` 只有 Vec 实现、该方法与根导出消费。App `provider_assembly::assemble_registry` 生产调用的是 `merge_provider_models`。 | 删除方法、trait、Vec 实现与导出；合并实现保持原样，既有回归直接调用 `merge_provider_models`，保留 v2 证据、跨 Provider 同 id 不覆盖与未知模型追加断言。 |
| `capability_snapshot` | 仅未知模型 / 三源快照两个测试调用；原 Spec 的 negotiator 初始化、诊断 / 展示描述没有真实消费。App Run / Gateway 和 Anthropic adapter 都按单模型取 `capability_evidence`。 | 删除全量枚举口及仅属于该口的排序 / 枚举断言；三源、probe-only、override-only 与 Provider 锚点断言改用既有单模型入口，保留 `CapabilityEvidence::merged` / `merge_capabilities`。 |
| `filter_by_purpose` | 仅用途回归调用；ADR-064 的公开承诺是 canonical 用途、CLI `--purpose` 与 Gateway `?purpose`。`cli::run_models`、App `gateway_models` 实际调用 `capabilities_support_purpose` 并对多个用途取 `all` 交集。 | 删除 registry 包装；保留 `purpose_requirements` / `capabilities_support_purpose`、`filter` / `caps_satisfied`。既有回归用 `filter(purpose_requirements(...))`，不另造能力映射。 |

### 公开面、冻结与写入集

删去四个公开 Rust 方法及一个 trait / Vec 实现 / 重导出。workspace `publish = false`；这些入口不是 domain 的 `ModelProvider` 契约，也不承担 GUI、HTTP、CLI 或磁盘格式。`CatalogEntry`、`CapabilityEvidence`、`ProviderProbe` 的字段与 serde 属性、canonical `ModelPurpose`、依赖与 feature 均未修改，无 schema / wire 演进。

CU-11 的生产链保持 `capability_evidence` + `merged()`：文本模型在工具定义 / 派发前 fail-closed，视觉模型保留 computer，未知目录发现成功后按最终证据重建允许表；这些消费点和 negotiate 实现均未修改。本轮不把 CU-11 或用途筛选的人工验收标为完成。

精确写入集：`crates/models/src/{registry,lib}.rs`、`docs/spec/crates/models.md`、本任务与 `docs/ROADMAP.md`；另只修正 `docs/spec/crates/providers.md` 中紧相关的目录 / 证据消费说明，消除对已删除包装口的现行承诺并对齐保留入口。App / providers 源码无需修改。

### 验证与回归

日志目录：`/tmp/pawork-b5-20261008-084629/`。两次 Cargo 调用前均核对无其它 Cargo，顺序执行，沿用 `.cargo/config.toml` 的 kache；无真实 Provider 请求。

| 实际命令 / 检查 | feature 与结果 | 证据 |
| --- | --- | --- |
| `git diff --check`；`rustfmt --edition 2021 --config skip_children=true --check crates/models/src/lib.rs crates/models/src/registry.rs`；Rust 引用与修改文档的相对链接检查 | 通过；删除的四入口 / trait 在 Rust 中无残留引用。首次格式检查 exit 1，只发现两处换行差异，修正后通过。 | `format-initial.log`、`prechecks.log`、`final-checks.log` |
| `bash scripts/test.sh --log /tmp/pawork-b5-20261008-084629/models-test.log models` → `cargo test --offline --tests -p pawork-models` | models 无 feature；exit 0，52 passed / 0 failed / 0 ignored / 0 filtered out。 | `models-test.log`、`models-test.json` |
| `cargo check --offline -p pawork` | 正式宿主默认装配；App 依赖带齐 providers 九通道 feature；不启用 `ui-fixture` / `live-smoke`。exit 0，App / providers / CLI 等下游装配通过；这是类型检查，不计作运行测试。 | `host-check.log`、`host-check.json` |

定向锚点保留通过：

- `registry::tests::merge_provider_catalog_feeds_v2_caps_into_evidence_without_name_match`：静态 v2 能力、跨 Provider 冲突拒绝、未知模型追加。
- `registry::tests::capability_evidence_covers_all_three_sources`（由快照测试改用单模型入口）、`evidence_for_unknown_model_is_none`、`evidence_resolves_via_alias`：三源与仅 probe / override 证据、未知模型和别名解析。
- `registry::tests::override_cannot_amplify_static_capabilities`、`override_can_narrow_static_capabilities`、`probe_merges_with_static_and_cannot_amplify`、`merge_allows_empty_tool_tags_to_narrow_capabilities`：保守交集与能力收窄。
- `registry::tests::purpose_filter_covers_four_capabilities_and_intersects`：单用途（含 video_input）、多个用途交集、text 排除生图条目和 image_output 筛选；既有 negotiate 图片 / 嵌套工具图片 gate、推理与计价回归同批通过。

未新增测试。仅移除已删除布尔包装的上下文测试与全量快照排序 / 枚举断言；不删安全语义锚点。共享生产实现与 CU-11 / 用途消费均未改变，因此不扩大到 App / providers 行为测试、GUI、live Provider 或全 workspace。

视频验证边界：models 的 52 条测试包含视频用途能力判定，不包含嵌套视频请求 gate 专项；`content_part_has_video` 的递归实现保持原样，providers 的 `video_reference_maps_to_documented_chat_wire` / `video_rejects_unverified_endpoint_and_unsafe_references_before_network`、App 的 `video_reference_survives_run_resume_and_rejects_old_clients` 均未修改、未在本轮执行。正式宿主类型检查只证明生产链路编译，不能计作这些视频行为回归通过。

Validated: 上表实际命令与引用 / 链接 / diff 检查。

Targeted regressions: models 52 条，保留上述目录、证据、能力收窄、多模态和用途交集锚点。

Full workspace gate: NOT RUN（当前未设置全量门禁）。

### 独立审查与完成记录（2026-10-08）

完成日期：2026-10-08。独立 `gpt-6.1-sol / max` 只读 review（对话 `01a11903-fbbd-7580-af58-cab4599e4637`）明确 PASS，未发现需修复的问题；实现与审查两轮均正常 completed、error=null。

审查核实四方法与 trait 的删除前消费、保留的生产入口、CU-11 图像门控、用途交集和冻结格式；原始日志支持 models 52 条通过与正式宿主类型检查，`final.diff` 与受审工作区字节一致。审查未编译、测试或修改文件，指挥只同步已验证的审查状态。

本任务的 API / Spec 同步与受影响验证条件均已满足，ROADMAP 同步为 ✅。实现者未提交；本地提交由指挥处理，不推送或发布。

无已知实现缺口。CU-11、用途 GUI 与真实 Provider 的既有人工 / 外部验收边界不因本次内部 API 清理而完成。
