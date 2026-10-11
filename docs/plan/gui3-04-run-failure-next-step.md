# GUI3-04 · 失败卡与下一步人工验收

> 状态：✅ 完成（模拟验收）；独立 review PASS；真实平台边界待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收 Run 终态、错误原因、认证入口和 Review changes 条件。

## 当前依据与读取范围

本轮按用户授权以 GPUI 测试平台模拟输入与终态；核对当前源码和正式渲染 / handler 链路。真实 Host、Provider、Git、原生 AX 与像素证据独立待验。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 使用真实失败与取消结果核对紧凑错误 banner、完整原因访问和终态页脚；取消不误示为成功或失败。
- 认证类失败显示现有供应商设置入口并正确导航，不提供未实现的重试。
- 仅在当前会话存在真实可审阅文件时显示 Review changes，点击后进入对应 Changes；无文件、未知与其它会话差异保持诚实说明。

## 完成条件

- 用户验收非认证失败、认证入口和 Review 可用条件，显示与 Host / 文件事实一致。
- 原因可读且下一步可操作，长原因与字号变化不阻断使用。

## 完成记录

2026-10-11，Grok 4.7 xhigh 在既有 `failed_run_banner_geometry_and_provider_cta_preserves_draft` 补齐模拟验收；生产操作语义未改，Button 与 Changes 页签补 `debug_selector` 供模拟鼠标定位。GUI 设计同步当前 16px 警示图标、单行截断原因与 AX 全文。

- 长认证 / 非认证原因在 100% / 125% / 150% 下保持紧凑高度，AX description 保留完整原因；认证 CTA 在卡片内，150% 的正式 AX Press 进入 Settings Providers 并保留草稿。既有认证、超时与取消页脚断言保留。
- 未拉取、空文件、其它会话、缺失 session id 与 stale 不发布 Review；当前会话 Ready 且非空时，模拟鼠标点击实际渲染按钮进入 Inspector Changes，草稿保留。
- 文件清单由测试经 `apply_files` 投影；正式 refresh 因无 workspace 返回 `Failed("no workspace")`。这证明导航与刷新入口，不证明真实 Host / Git 文件回执。测试将 InspectorMotion 插到终宽后检查面板，不验证 180ms 中间帧；动效归 GUI3-08。

证据：`/tmp/pawork-gui3-04-run-failure-next-step/evidence/`。第 1 次 `--exact` 过滤失配，0 tests / exit 0，不计通过；第 2、3 次各 1 test 失败 / exit 101，原日志保留。第 4 次修后 1 passed / 294 filtered / 7.90s / exit 0（`desktop-failed-banner-4.log` 与独立 `.exit`）。LLDB attach / backtrace / detach 日志仅为编译诊断，不计验收。

Validated: `cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders failed_run_banner_geometry_and_provider_cta_preserves_draft -- --test-threads=1`；本次三份 Rust 文件定向 `rustfmt --edition 2021 --check`；`git diff --check`。独立 Sol max review PASS，阻断 / 非阻断均无；审查对话 `01a12941-39fb-7a72-857f-4884525470e5`，完成 turn `01a12941-3d2f-7d03-8ab7-76092aa35289`，原回执保存 `REVIEW-PASS.json`。

Targeted regressions: 上述 1 个 GPUI 主路径用例。Full workspace gate: NOT RUN（当前未设置全量门禁）。

本项按用户授权交付模拟验收；真实系统窗口 / 像素、原生 AX、Host / Provider / Git 与真人确认仍待环境，不登记为已真人验收或已发布。
