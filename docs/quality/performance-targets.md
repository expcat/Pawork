# 性能目标

Core 指标与 Desktop GUI 指标分别计量，不能把 Desktop GPU/窗口、网络或模型耗时归因给 Rust Core。性能测试必须区分：Rust Core；Git 子进程；Provider 网络；模型生成；外部命令；GUI controller/projection；GPUI 渲染。

## Rust Core 指标

| 指标 | 目标 |
| --- | ---: |
| Core 冷初始化 P50 | < 150 ms |
| Core 冷初始化 P95 | < 400 ms |
| Core 空闲 RSS | < 50 MB |
| 无活跃 Session RSS | < 70 MB |
| 列出 10,000 个 Session | < 100 ms |
| 打开大型 Session 尾部 | < 250 ms |
| Agent Event 分发开销 | < 2 ms |
| Built-in Tool 调度开销 | < 5 ms |
| 中型仓库 Git status | < 300 ms |
| 已缓存 Diff 切换 | < 50 ms |
| 100,000 行 Diff 解析 | < 500 ms |
| 崩溃后 Session 恢复 | < 1 s |
| Provider 首 Token 的 Core 附加延迟 | < 20 ms |

## Desktop GUI 指标（Phase 19）

| 指标 | 目标 |
| --- | ---: |
| Desktop 冷启动到可选择/连接实例 P50 | < 1.5 s |
| 已连接后 Snapshot 接收完到可交互 P95 | < 500 ms |
| Event 经 `gui-client` 进入 Desktop controller 到可见提交 P95 | < 50 ms |
| 10,000 条 Timeline 打开并定位尾部 P95 | < 750 ms |
| Timeline / Diff 稳态活跃渲染行（可见区 + overscan） | < 500 |
| 100,000 行 Diff 首屏可交互 P95 | < 1 s |
| 连续 30 token/s + tool output 流式渲染 | 无持续掉帧；P95 frame < 32 ms |

Desktop 指标在 Windows、macOS、Linux 的真实 GPUI 原生壳分别记录，并注明 GPU/驱动、显示缩放、窗口系统与软件渲染退化；headless/component harness 不替代真实平台数据。模型、远程 Transport 与 Core 计算时间从 GUI 指标中分段标注，不用端到端总耗时掩盖 UI 回归。

## 相关文档

- [测试体系](testing.md) · [ROADMAP 实施波次与门禁节奏](../../ROADMAP.md#实施波次与门禁节奏)
- [Desktop GUI](../features/desktop-gui.md) · [P19-16 Desktop Gate](../../plan/P19-16-desktop-gate.md)
- [ADR-020 性能与安全是发布门槛](../adr/ADR-020-performance-security-gate.md)
