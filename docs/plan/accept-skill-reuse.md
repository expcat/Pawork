# AC-04 · 技能录制产物后续 Run 发现验收

> 状态：✅ 完成（模拟验收）；独立 review PASS；真实 Provider 与非 Unix 待验。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

证明录制技能能被真实后续 Run 作为资源发现。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[workspace Spec](../spec/crates/workspace.md)、[app Spec](../spec/crates/app.md)、[Desktop Spec](../spec/desktop.md)。

## 验收范围

- 先生成真实录制产物，再在独立后续 Run 检查资源发现和引用。
- 以产物、Host 加载事实和实际 Run 联合核对，不只验收录制按钮或文件存在。

## 完成条件

- 后续 Run 能发现正确技能与来源，路径和不可信边界保持正确。
- 需要真实模型时固定使用验证规格的当次模型参数，并记录实际终态。

## 完成记录

### 本轮链路与边界

沿用 `recorded_operations_save_and_load_as_real_skill`，录制来源是种入 SQLite 的 `read_file` 操作历史，未实际执行该原操作。正式 Host preview / save 创建真实 `.pawork/skills/read-project/SKILL.md` 与 `manifest.toml`，校验内容、ID、版本和描述，并保留重复不覆盖断言。

测试 Core 显式安装现有 `ExtensionService::resource_loader_for`，对登记工作区在内存设置信任。后续不同 run ID 走正式 `AppCore::chat_turn_with_run_id` → RunService → Engine → Storage；仅外部 Provider 用 `ScriptedProvider` 模拟。联合核对实际请求中的 `msg-resources` 技能正文、持久 `resources.injected` 的 `active_skill` / ID / 字节数和 `RunCompleted`。撤销工作区信任后再次 Run，正文与诊断均不得含该技能。

本项不证明原操作实际执行、Desktop 录制按钮、真实 Provider 推理或非 Unix 安全写入；非 Unix `Unsupported` 裁决与平台待验边界保持原状。

### 实际验证

2026-10-12，macOS Unix 分支，执行对话 `01a12bd1-4abf-7061-9fd3-ef73f7beb80a`（`xai/grok-4.7 xhigh`），基线 `73738717f2f8a368e22707aea3e1d1dd9bbd5b2a`。只改五份 Rust 的测试代码和必要构造，不改生产加载、保存、wire 或依赖。

两轮均执行 `cargo test -p pawork-app --offline --lib --features ui-fixture gui_host::handlers::recording::tests -- --test-threads=1`，均 exit 0、2 passed / 0 failed / 307 filtered。首轮编译 3m 27s、测试 0.10s；首轮可能只读默认目录的全局技能，不作为完全隔离的证据。第二轮通过 `python3 /tmp/pawork-ac04-skill-reuse/run_test2.py` 的环境副本将 `PAWORK_DATA_DIR` 指向新的 `/tmp/pawork-ac04-data-d6kmrytl`，核实 `default_data_dir` 消费该覆盖；该目录运行后仍为空，测试 0.10s。没有改 `HOME` / `CODEX_HOME`、认证或持久配置。

证据目录 `/tmp/pawork-ac04-skill-reuse/evidence`：两轮 `cargo-test-1` / `cargo-test-2` 的 `.log` / `.exit` 和第二轮 `.data-dir`。五份 Rust 的 `rustfmt --check --edition 2021` 初次排版失败保留在 `rustfmt-check-1.log` / `.exit`，应用格式后 `rustfmt-check-2` exit 0；`git-diff-check-1` exit 0。隔离复核的两次辅助读取分别因 zsh `=== not found` 与 Python 引号语法失败，随后静态读取和 `run_test2.py` 成功；原始工具输出保留，不当作测试成功。没有未解决验收失败。

```text
Validated: 上述 app 定向回归，两轮各 2 passed / exit 0；五份 Rust 的 rustfmt --check；git diff --check。
Targeted regressions: 正式录制保存 / 产物、后续 Run 请求与持久事实、撤销信任不注入；既有 Secret / 路径 / source / symlink 拒绝。
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

2026-10-12 独立 `gpt-6.1-sol max` 审查对话 `01a12be6-e9e8-7832-bcaa-4e07a8d7367f` 最终 PASS（turn `01a12be6-ecac-7b92-8962-f75271db1473`，completed / error=null），阻断与非阻断问题均无。受审八文件快照 19345 字节，SHA256 `4c7789fed07ce4bb7270e859e068c6fe0537775e3d5a258223fdca3f58f8b53e`，已逐字节核对；PASS 后只同步本计划与 ROADMAP 状态，Rust 与 app Spec 不变。`PENDING.json` / `.diff`、`REVIEW-PASS.json`、原始工具输出及最终提交记录保留在上述证据目录。

