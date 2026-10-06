# ENV-02 · 既有跨平台专项环境验收

> 状态：缺环境。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#缺环境与外部条件)

## 目标

补齐 Linux / Windows、WebKit、容器和真实 Provider 的既有专项矩阵。

## 当前依据与读取范围

状态承接迁移前 ROADMAP；本次未执行对应环境验收。

[验证规格](../spec/verification.md)、[gui-server Spec](../spec/crates/gui-server.md)、[transport Spec](../spec/crates/transport.md)、[Desktop Spec](../spec/desktop.md)。

## 验收范围

- 按当前实现选择对应平台与真实环境，不把一般本机测试推广为跨平台通过。
- Windows 核对实例锁 `share_mode`、listener close/connect 唤醒和连接回收。
- 新增 computer-use 平台能力独立见 [CU-21](cu-21-isolated-linux.md)、[CU-22](cu-22-windows-isolation.md)、[CU-23](cu-23-wayland-feasibility.md)，不重复声明已实现。

## 完成条件

- 各选定环境有实际启动、操作与回收证据，缺环境保持待验。
- 专项实例、连接和 Provider 真实终态与合同一致。

## 完成记录

待填写实际环境、输入、结果、日期与证据。前置或验收缺失时保持当前待办状态；满足全部完成条件后同步本文件，并将 ROADMAP 对应项标为 ✅。

