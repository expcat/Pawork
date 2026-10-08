# B15 · App 查询、设置写盘与配置错误收敛

> 状态：✅ 完成。完成日期：2026-10-08。独立 Review / 复审 PASS；未推送、未发布。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决并收敛三个紧相关的 App 清理线索。

## 当前线索与读取范围

旧记录包含 gui_host diff_get `complete` 表达式死乘法（原记录位置为 `query.rs`，须定位现行文件）、`set_model_enabled` / `set_provider_models_enabled` 尾部 `cleared_roles` 重复写盘，以及两处 `config_unavailable` GUI 文案。当次源码还有第三处，在 `handlers/subagents.rs`。

[app Spec](../spec/crates/app.md)、[Settings Spec](../spec/settings.md)。

## 实施范围

- 先核对实际分支和文件，只有等价性成立才简化表达式或写盘。
- 统一同一失败含义的文案，保留真实错误状态和用户可执行入口。

## 完成条件

- 查询结果、默认角色清理和配置落盘行为不变；同一失败的文案说明原因，并给出可执行恢复动作。
- 现有相关测试足够时不新增。设置写路径跑既有定向回归；查询侧没有 diff_get 测试，等价性按 `paginate` 定义证明，编译由正式宿主类型检查覆盖。
- 独立 Review / 复审 PASS 后，由指挥同步完成状态。

## 基线

`main`，HEAD `9fa0327235a53d208b3438889ca74b5d360e87cb`。开始时工作区干净。没有做 B1–B14、B16 或 CU-10。

## 裁决

### 1. diff_get complete：简化为字面量 true

现行位置是 `crates/app/src/gui_host/handlers/query.rs` 的 `diff_get`，不是独立 diff.rs。CodeGraph 同名命中 Desktop `controller` 的 `diff_get`，未采用。

运算符优先级下，原表达式是 `(page >= paged.page && paged.files.is_empty()) || (paged.page * 1 >= paged.total_files)`。调用是 `paginate_diff(vec![file], page, 1)`。`paginate`（`crates/git/src/diff/service.rs`）在 page_size 非 0 时把页码钳到至少 1，`total_files` 等于输入长度。输入只有命中的那一个文件，所以 `total_files` 恒为 1，`paged.page` 恒大于等于 1。`usize` 乘 1 是恒等，不会溢出。第二项恒真，整个表达式恒真。

cursor 缺省或非数字先变成 1；`0` 被钳到 1；越页和 `usize::MAX` 只让 `files` 为空并保留请求页。这些路径的 `complete` 原来就是 true。无会话和路径缺失的早退本来也是 `complete: true`。

改成字面量 `true`。仍然调用 `paginate_diff`，仍然输出 `session_id`、`path`、`page`、`total_files`、`files`。不改 cursor 解析、latest workspace session、只读 Git 路径或分页模型。Desktop `parse_diff_file` 不读 `complete`。

### 2. 设置写盘：已落地，no-op

`set_model_enabled` 与 `set_provider_models_enabled` 在 `crates/app/src/gui_host/handlers/settings/catalog.rs`。每个函数计算 `clear_pairs` 后只调用一次 `write_provider_model_preferences`，然后同步内存并回执 `cleared_roles`。没有尾部第二次写盘。

重复写盘已在 `f6129240`（2026-09-07，OPT-3 review）去掉。当时先 `write_provider_disabled_models`，再按角色循环 `write_model_pair`。现在 denylist 与角色清除进同一次 `rmw_global_config`。写函数失败则不会执行后面的内存同步。

不改 `catalog.rs`。ADR-055 保持：Global denylist、其他实例已存选择、目录外已禁用项、固定角色序、盘上角色匹配、CLI 内存覆盖不误删、空目录全关 `catalog_unavailable` 不写盘、原子提交、写失败不改内存。不删 `cleared_roles` 回执。

### 3. config_unavailable：复用既有 helper，统一文案

三处都是 `pawork_workspace::config::global_config_path()` 返回 None，不是三种失败：

| 位置 | 原文案 |
| --- | --- |
| `handlers/mod.rs` 的 `global_config_file` | global config directory is not available on this platform |
| `settings/permissions.rs` 私有 `global_config_path` | global config directory is unavailable |
| `handlers/subagents.rs` 内联 | global configuration unavailable |

`global_config_path` 只在 `config_dir_for_app` 为 None 时为 None，也就是 directories 6.0.0 的 `ProjectDirs::from` 为 None。不能把 crate 概述里的 “no valid home directory” 当成 Windows 事实。macOS / Linux 的 `project_dirs_from_path` 才读 `home_dir`；Windows 同时要求 LocalAppData 与 RoamingAppData，任一失败即 None，并不查询主目录。旧的 “on this platform” 把原因说成平台不支持，也没有覆盖这两类目录的恢复动作。

permissions 和 subagents 改走已有 `global_config_file`。不新建错误类型或配置层。错误码仍是 `config_unavailable`，`host_error` 的 `retryable` 仍为 false。permissions 仍在写锁内、调用 writer 之前取路径；失败不写盘、不改内存。subagents 仍先取路径，再 `write_subagent_settings`，成功后才拿写锁改内存。

统一文案：`无法解析当前用户的 Global 配置目录；请检查用户主目录及系统应用数据目录配置后重试`

既有测试没有断言这三句旧文案。Desktop 不按 message 文本分支。CLI 使用同一个 `global_config_path`，不能绕过这次失败。

## 写入集

`crates/app/src/gui_host/handlers/query.rs`、`handlers/mod.rs`、`handlers/settings/permissions.rs`、`handlers/subagents.rs`、[app Spec](../spec/crates/app.md)、[Settings Spec](../spec/settings.md)、[ROADMAP](../ROADMAP.md)、本文件。

未改 `catalog.rs`、冻结 wire、Desktop、architecture 或 flows。

## 验证

日志目录 `/tmp/pawork-b15-app-small-cleanup`。Cargo 串行，每次前确认没有其它 cargo 进程。沿用 kache，不 `cargo clean`。测试在进程内把 HOME 指到临时目录；shell 没有改 HOME，不读用户 auth/config。

| 命令 | 结果 | 日志 |
| --- | --- | --- |
| `cargo test -p pawork-app --offline --lib -- set_approval_mode_updates_permissions_settings_within_same_session workspace_trust_toggles_session_trust_for_attached_workspace` | 2 passed，0 failed，297 filtered out，exit 0。编译 Finished `test` in 32.88s，测试 0.11s。`pawork-mcp` `with_auth_token` dead_code 与 `block` future-incompat 是既有提示。 | `settings-permissions.log` |
| `cargo check -p pawork --offline` | exit 0，Finished `dev` in 19.70s。同上两条既有提示。 | `pawork-check.log` |

`gui_host` 测试和 `crates/app` 里没有 diff_get 用例，所以没有查询行为回归可跑。`rustfmt --edition 2021 --config skip_children=true --check` 覆盖上述四个 Rust 文件，exit 0。不把未编译的 Desktop 或其它包记为通过。

未跑 Desktop、全 workspace 或真实 Provider。自动检查不表示人工验收。Review P2 只改上述文案、helper 注释和本文件、app Spec、Settings Spec。不重跑已经成功的 Cargo，也不把配置目录 `None` 补成运行验证。交接 `final.diff` 在日志目录，于本计划定稿后生成；字节数和 SHA256 写在交接里，不回写本文件。

## 保留边界

cursor 默认、非法、0、越页；latest workspace session；缺路径与无会话；files、page、total_files；只读 Git；分页模型；ADR-055 写盘与 `cleared_roles`；错误码、失败关闭、写盘与内存时序。

## 完成记录

2026-10-08：独立 reviewer `01a119e5-37b9-7831-af7f-2f11b42b9e67` 初审的 P2 跨平台原因文案与 P3 交接记录均已修正，同一 reviewer 增量复审最终 PASS，无剩余问题。指挥核对受审 `final.diff` 与工作区逐字节一致后，仅同步本页和 ROADMAP 的完成元数据。

沿用上表两条权限回归及正式宿主类型检查的成功记录；文字修复未重跑 Cargo。四 Rust 定向格式、diff 和四份 Markdown 链接检查通过。配置目录 `None` 的运行验证、Desktop、真实 Provider 与人工验收未执行；Full workspace gate: NOT RUN（当前未设置全量门禁）。
