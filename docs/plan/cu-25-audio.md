# CU-25 · 显式授权的目标音频采集

> 状态：设计与契约拒绝模拟完成；独立 review / 复审 PASS；真实采集、系统授权与模型音频输入待环境。前置：[CU-02](cu-02-target-contracts.md)、[CU-16](cu-16-target-approvals.md)、[CU-20](cu-20-native-acceptance.md)。更新：2026-10-10。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 触发条件

用户选择音频使用场景并显式开启后开展，不默认采集。

## 目标

补齐目标音频录制或模型使用链路，并明确二者的能力边界。

## 当前依据与读取范围

LCU 音频是原 runtime 的可选录制 API；保存录音不等于把音频送入模型。Pawork 当前没有此 computer-use 音频链路。

[computer-use Spec](../spec/crates/computer-use.md)、[providers Spec](../spec/crates/providers.md)、[models Spec](../spec/crates/models.md)、[安全规格](../spec/security.md)、[LCU 音频说明](https://github.com/amontlabs/lcu/blob/21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a/README.md)。

## 实施范围

- 先确认录制给用户还是作为模型输入、目标范围、格式/时长/字节预算及权限；不顺势录制其它用户应用的系统音频。
- 以 Rust 系统采集 API 实现实际支持的目标范围；若无法隔离范围，明确不支持而非默认录全桌面。
- 停止、取消、撤销和任务结束释放采集。若交给模型，先验证模型/Provider 音频能力与完整编码通路。
- 需要演进媒体、持久化或 wire 时先做对应裁决和 golden；不把仅有文件保存记为模型听到音频。

## 完成条件与验证

- 真实音频来源、权限、预算和停止行为可核对，用户正常操作不受影响。
- 录制产物与模型实际收到的内容分开验收；未支持模型输入时保持明确限制。
- 未启用时零采集，取消或撤销后不继续录制，不含未授权应用内容。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

### 2026-10-10 设计与模拟边界

按用户限定，本轮只推进设计与可模拟拒绝边界：不启用目标音频、不申请麦克风或屏幕音频权限、不启动真实采集、不调用 Provider，不把保存文件冒充模型已听到音频。设计与原始证据在 `/tmp/pawork-cu25-audio-design/evidence/`。

当前生产路径没有目标音频入口：`Environment` 仍只有 `isolated_desktop` / `native_background`，十个 `NativeActionKind` 与两种 `ObservationKind` 均无音频，macOS 权限预检只有 Screen Recording / Accessibility，Host/Provider 也没有音频内容编码链。设计默认关闭；Once / ForRun 目标授权不能顺便开启音频；桌面、系统混音、窗口、网站与未证实隔离的应用范围一律 Unsupported，不回退录全桌面，麦克风保持关闭。关闭、停止、取消、撤销与任务终态先拒绝新请求；若流已启动，立即请求中止、拒收并丢弃迟到缓冲、释放系统资源与 Host 侧引用，结果未知时 fail-closed 如实报告。用户侧录音与模型输入分开，保存文件不等于模型听到音频。

实现与验证：生产代码无改动，新增测试内 fixture `audio_capture_is_rejected_by_the_contract` 模拟未启用、范围不可隔离、撤销后重试、停止、取消、任务结束、时长 / 字节超限与最终 NotImplemented；fixture 不启动流、不触发权限、不调用 Provider，也不覆盖在途中止。`cargo test -p pawork-computer-use --offline --lib` 101 passed / 0 failed、exit 0；这是契约与 fixture 模拟，不是系统授权或真实音频验收。

待真实环境：应用级音频是否可隔离、OS 音频权限授予与撤销、真实流开始后的停止与释放，以及单独裁决后的 Provider 音频往返。上述未验项未完成前本任务不标 ✅。

独立 review（2026-10-10，Sol max，thread `01a1254b-fee3-7952-9c54-eec1035c842c`）：R1 初审 turn `01a1254c-0329-7c62-a872-dd28beea6e5e` 报 1 个 P2（在途采集停止 / 释放设计不足）与 2 个 P3（测试可缩小、视频事实表述不准）。Grok xhigh thread `01a1253f-37e7-7250-943f-f72d0d2273cb` 补在途中止与迟到缓冲丢弃设计、删除恒零计数与静态标签扫描、修正 Chat 编码 video_url 的事实；design.md SHA256 `80d198773afb46692b806e7af016320cf9a877d22e0f43adddcb048e2385580a`，R2 快照 12,796 字节、SHA256 `9b38aa91f0230656bb8a4189a3f5a64b6f3261e58da13e1766e60a4b6683c39a`。复审 turn `01a12555-f42e-7482-80ab-abaa048dae05` PASS；结论限定设计与模拟，不代表目标音频能力已实现或验收。
