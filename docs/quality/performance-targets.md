# 性能目标

以下目标仅指 **Rust Core**，不包含 WebView 与模型网络时间。性能测试必须区分：Rust Core；Git 子进程；Provider 网络；模型生成；外部命令；GUI 渲染。

## 指标

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

## 相关文档

- [测试体系](testing.md) · [ROADMAP 实施波次与门禁节奏](../../ROADMAP.md#实施波次与门禁节奏)
- [ADR-020 性能与安全是发布门槛](../adr/ADR-020-performance-security-gate.md)
