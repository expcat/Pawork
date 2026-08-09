# 基准方法论与计时口径

> P0-12 基准框架骨架配套文档 · 对齐 [性能目标](performance-targets.md)、[测试体系](testing.md) 与 [ADR-020 性能与安全是发布门槛](../adr/ADR-020-performance-security-gate.md)。

## 职责

为所有 Phase 的性能基准提供统一落点（`benches/` crate，`pawork-benches`）与统一计时口径，确保每个耗时数字都能回答「这是谁的耗时」。基准按六类计时域分组，与 ADR-020 要求的耗时来源划分一一对应；任何跨域混合的数字不得直接对照性能目标。

## 六类计时域

| 计时域 | 基准组 | 进程边界 | 计时口径（含 / 不含） |
| --- | --- | --- | --- |
| Rust Core | `core` | `pawork` 单二进制进程内 | **含**：进程内逻辑的墙钟时间（状态机、事件分发、Session 存取、Diff/Patch 解析、Token 预算等）；**不含**：任何子进程、网络、模型与 GUI 耗时 |
| Git 子进程 | `git` | `pawork` 派生的 `git` 子进程 | **含**：从 spawn 到进程退出的墙钟时间（fork/exec + git 自身执行）；**不含**：走网络的操作段（`git fetch`/`clone` 的远端交互须单列或排除） |
| Provider 网络 | `provider` | `pawork` ↔ Mock Provider | **含**：请求发出到响应收完的传输与协议开销；**不含**：真实公网 API。一律用 Mock Provider（wiremock）固定延迟，保证可回归 |
| 模型生成 | `model` | `pawork` ↔ Mock Provider（脚本化 token 流） | **含**：Core 消费/解析/分发 token 流的开销；**不含**：真实模型推理时间——无真实模型时模型生成不可直接测量，用 [测试体系](testing.md) 的 `MockScript` 脚本化 token 流模拟生成速率 |
| 外部命令 | `command` | `pawork` 派生的 Tool 子进程 | **含**：子进程 spawn/exec 与调度开销（spawn → 就绪/退出，含 PTY/管道建立）；**不含**：外部命令自身的业务耗时（不计入平台目标） |
| GUI 渲染 | `gui` | GUI 独立进程 + WebView | **含**：bridge→render commit、帧率、交互、DOM/内存；GUI 经 GUI Connection Protocol 连接 CLI，不嵌入 Core，**不经 criterion 测量**；Phase 19 使用 WebdriverIO Tauri + renderer performance marks，`benches/gui` 只保留跨域入口 |

### 口径要点

- **首 Token 延迟拆分**：用户感知的首 Token = Core 附加延迟 + Provider 网络 + 模型生成。性能目标中的「Provider 首 Token 的 Core 附加延迟 < 20 ms」只计 Core 段，在 `provider` 组内以 Mock Provider 零网络延迟口径测量。
- **冷测量 vs 热测量**：criterion 循环采样测的是热路径。冷初始化、崩溃后 Session 恢复、RSS 等目标要求每次迭代都是全新进程/状态，必须用「每次迭代派生新进程」的专用多进程 harness 测量；禁止用热循环数字冒充冷启动数字。
- **RSS**：在固定检查点（初始化完成 / 空闲稳定后 / 无活跃 Session）读取进程 working set，报告 P50/P95。
- **百分位**：目标中的 P50/P95 以多次独立运行的样本计算。criterion 默认报告均值与中位数区间；原始样本保存在 `target/criterion/`，P95 从样本计算，或在专用 harness 中直接输出。
- **可重复性**：基准不得依赖墙钟时间之外的真实外部系统（真实 API、真实模型、真实网络）；需要外部负载时走 Mock 或门禁开关（见下）。

## 目录结构与运行

```
benches/
  Cargo.toml          # crate 定义与六个 [[bench]] 目标（harness = false）
  src/lib.rs          # 共享辅助：GROUPS 常量与门禁开关约定
  core/bench.rs       # Rust Core
  git/bench.rs        # Git 子进程
  provider/bench.rs   # Provider 网络
  model/bench.rs      # 模型生成
  command/bench.rs    # 外部命令
  gui/bench.rs        # GUI 渲染（占位）
```

`benches` 已登记为根 Cargo workspace 成员，从仓库根运行：

```powershell
cargo bench -p pawork-benches                         # 全部六组
cargo bench -p pawork-benches --bench core            # 单组
cargo bench -p pawork-benches -- core                 # criterion 过滤器
cargo bench -p pawork-benches -- --save-baseline p0   # 保存基线
cargo bench -p pawork-benches -- --baseline p0        # 与基线对比
```

### 门禁开关约定

P0-12 的空基准占位不需要任何外部依赖，默认可运行。后续接入真实负载（Git 仓库、Mock 服务、子进程、GUI）后，默认仍必须可无外部依赖运行；真实负载以环境变量放行：`PAWORK_BENCH_GIT=1`、`PAWORK_BENCH_PROVIDER=1`、`PAWORK_BENCH_MODEL=1`、`PAWORK_BENCH_COMMAND=1`、`PAWORK_BENCH_GUI=1`（辅助函数：`benches/src/lib.rs::group_enabled`）。未放行时对应组运行占位基准并跳过真实负载。

完整基准属于 L3 维护/发布门禁，不要求每个功能任务运行。临时基准使用独立 `CARGO_TARGET_DIR=target/bench-gates`，结果记录或导出后执行 `cargo clean --target-dir target/bench-gates`；需要长期比较的人工确认 baseline 应导出为版本化证据，不与临时 criterion 缓存一起删除。

## 指标映射

性能目标（[performance-targets.md](performance-targets.md)）到计时域的映射：

| 性能目标 | 计时域 | 测量方式 |
| --- | --- | --- |
| Core 冷初始化 P50/P95、崩溃后 Session 恢复 | `core` | 冷测量多进程 harness |
| Core 空闲 RSS、无活跃 Session RSS | `core` | RSS 检查点采样 |
| 列出 10,000 个 Session、打开大型 Session 尾部、Agent Event 分发开销、已缓存 Diff 切换、100,000 行 Diff 解析 | `core` | criterion 热测量 |
| 中型仓库 Git status | `git` | criterion + 子进程计时（固定 fixture 仓库） |
| Provider 首 Token 的 Core 附加延迟 | `provider` | Mock Provider 零延迟口径，只计 Core 段 |
| Built-in Tool 调度开销 | `command` | criterion + 子进程计时 |
| Desktop cold start、Snapshot→interactive、Event→paint、10k Timeline、100k Diff、stream frame/DOM budget | `gui` | WebdriverIO Tauri + renderer performance marks；三平台原生壳，不经 criterion |

## 验收映射（P0-12）

- 「可运行空基准」：六组 `placeholder` 空基准均可用 `cargo bench -p pawork-benches` 运行。
- 「计时口径有文档说明」：本文档「六类计时域」与「口径要点」。

## 相关文档

- [性能目标](performance-targets.md) · [测试体系](testing.md) · [安全验收](security-acceptance.md)
- [Desktop GUI](../features/desktop-gui.md) · [P19-16 Desktop Gate](../../plan/P19-16-desktop-gate.md)
- [ADR-020 性能与安全是发布门槛](../adr/ADR-020-performance-security-gate.md) · [ROADMAP 实施波次与门禁节奏](../../ROADMAP.md#实施波次与门禁节奏)
