# ENV-06 · 非 Unix 技能录制写入前置与验收

> 状态：✅ 完成（维持非 Unix Unsupported 的范围裁决分支）；macOS 两条既有回归与独立 review PASS，非 Unix 原生未验、不表示平台支持。完成日期：2026-10-09。

[返回 ROADMAP](../ROADMAP.md#缺环境与外部条件)

## 目标

补齐非 Unix 安全写入原语，或明确维持 Unsupported 边界。

## 当前依据与读取范围

状态承接迁移前 ROADMAP；本次未执行对应环境验收。

2026-10-09 当前事实：workspace 的非 Unix `write_new` 明确返回 `io::ErrorKind::Unsupported`、没有写入降级；App `save_files` 却把它归到权限 / symlink 的泛化文案，现有主回归还无条件假定所有平台保存成功。root 先经 CodeGraph 定位，再核对相关现有回归；不把 CodeGraph 的「未找到覆盖」当作测试不存在。

[workspace Spec](../spec/crates/workspace.md)、[app Spec](../spec/crates/app.md)、[安全规格](../spec/security.md)。

## 验收范围

- 原记录指出非 Unix 未实现安全目录写入原语，入口明确返回 Unsupported。
- 取得目标平台与安全等价性依据后实现最小写入；不能用普通覆盖写绕过路径和 Secret 边界。

## 完成条件

- 若实现，真实平台路径拒绝、目录安全和产物完整性均有对应证据。
- 若维持 Unsupported，须有明确范围裁决并同步文档，不能把未实现写成支持。

## 完成记录

范围裁决（2026-10-09）：本轮选择上述「维持 Unsupported」分支。缺少等价安全目录相对写入证据，不实现非 Unix 写入、不采用普通路径覆盖降级；Unix fd-relative / NOFOLLOW / EXCL / 0600 / manifest-last / sync 保持原样。后续若新增平台支持，仍须按原条件取得安全等价性与真实平台证据；本次裁决不表示 Windows 或其它非 Unix 已支持、已原生验收。

实施线程 `01a11d39-b6aa-7ed3-8ce3-f915e8daf349`，明确 `xai/grok-4.7` / xhigh，turn `01a11d39-b931-7543-a9ff-bc19fb8f8774` 已 completed / error=null。唯一源码写入为 App `gui_host/handlers/recording.rs` 的固定 Unsupported 提示与紧相关既有回归；workspace 源码只读，现行 `skill_recording` 错误码、LocalGui / API 1.24、名称 / 字节 / Secret / workspace 校验不变。文档由 root 维护，确定性核验先于唯一独立 `gpt-6.1-sol` / max 只读 review，不先行标完成。

Host 区分 AlreadyExists、Unsupported 与其它写入错误。Unsupported 固定文案为 `Skill recording is not supported on this platform`；不回显原始 IO 错误、绝对路径或凭据。Unix 既有主回归保留保存、ResourceLoader 加载、重复不覆盖并锁住 AlreadyExists 文案；非 Unix 主分支在 `cfg(not(unix))` 下断言明确拒绝、现行错误码 / 文案和零目录 / SKILL.md / manifest.toml，不 skip、不提前 return。本轮 macOS 不编译或执行该非 Unix 分支，不能冒充原生验收。第二条既有回归仍覆盖 Secret、路径与 source 拒绝，symlink 部分维持 Unix 条件。

实际验证：`rustfmt --edition 2021 --check crates/app/src/gui_host/handlers/recording.rs` exit 0；`cargo test -p pawork-app --offline --lib --features ui-fixture gui_host::handlers::recording::tests` exit 0，2 passed / 302 filtered，测试运行 0.04s。仅一个串行 Cargo，未跑 Host check / Desktop / 全 App / workspace / 真实 Provider / 交叉编译。既有 mcp dead_code 与 block future-incompat 警告保留。测试发生在保留 CU-10 未提交改动的工作区，不证明 CU-10 或其它平台通过。

证据目录 `/tmp/pawork-env06-unsupported.XHohtD` 保存真实 Cargo 原输出、退出码、前置快照及待裁决 CU-10 十四源码 hash。实现者 `final.diff` 为单源码 unified diff，6256 字节，SHA256 `a190d535a463ac15e0c41231403c842eac690cb36fe62e70cdd99852dfd07e0d`；patch 重建与当前源码 cmp 一致。当前源码 17549 字节，SHA256 `05d6ae8ec8c049901733836192cf37284f5ae4dfc96034e9a7b5115b3ce3dad0`，不能混作 diff hash。root 核对 CU-10 十四源码 hash 全部不变；ENV 增量送审 / 提交须从本任务 baseline 隔离，不能把 App Spec / ROADMAP 中的 CU-10 未提交改动混入。

辅助失败如实保留：首 rustfmt wrapper 用 zsh 只读变量 `status` 收尾，exit 1、没有有效 rustfmt 退出码，随后独立有效检查 exit 0；末尾 `rg` 未转义 `+++`，exit 2，root 以正确转义表达式核对两条 diff 头恢复 exit 0。`diff -u` exit 1 仅表示内容变化；不是产品失败。未覆盖历史日志，手写 NOTES 由 apply_patch 创建。root 首次读取 PASS 时因原始 Markdown `**PASS**` 被过严前缀比较误拒，改为解析已保存的完整 final 恢复；不重跑 review 或 Cargo，记录在 `ROOT-PASS-READ-NOTES.txt`。以下完成状态只关闭本轮裁决分支，不能借此关闭其它平台任务或 CU-10。

独立 review（2026-10-09）：唯一 `gpt-6.1-sol` / max 线程 `01a11d47-0917-76d3-b9c3-f67d4bb29368`，turn `01a11d47-0c7b-7f13-9237-63ab7ef54a5f` completed / error=null，最终 PASS，blocking / nonblocking 均 none。只读审查未写文件、未编译或测试；原始 final 保存为 `review-r1-PASS.txt`。受审 ENV-only HEAD 投影 `root-r1-review.diff` 精确五文件、19687 字节，SHA256 `03a001dad15eaccf7fceb363069926b6a93da8a110cbf98375f595537fad37bd`；源码 Git diff 6385 字节，SHA256 `110f7bacbf01051337d2d45853c46cd07a6ee84ebee7dd01b499ae62294482fc`，独立投影 cmp 0。163 相对链接 missing 0，diff / 投影 cached check 通过，CU-10 十四源码 hash 不变、真实索引空；root 收到 PASS 后再次核对五份工作文件 hash 均一致，随后仅同步本计划与 ROADMAP 的审查元数据，受审源码及 App / workspace Spec 不变。

已满足原 plan 明确允许的 Unsupported 裁决分支：范围选择、Host 诚实提示、既有回归与对应 Spec 同步完成且独立 review PASS。拟本地提交只含 recording.rs 与本任务四份文档增量，App Spec / ROADMAP 中的 CU-10 改动保持未提交，不推送或发布。

Validated: 上述 rustfmt / 最窄 recording tests；root 与 reviewer 的只读 diff / SHA256 / cmp / 链接与五文件提交隔离检查。Targeted regressions: 两条既有 macOS 回归 2 passed；非 Unix 分支未编译或执行。Full workspace gate: NOT RUN（当前未设置全量门禁）。
