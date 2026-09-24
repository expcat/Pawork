# 包定位与收敛建议

> 基线与证据范围见 [Review 入口](README.md)。以下是建议，不改变 [架构](../architecture.md) 的包布局与冻结契约。

## 1. 24 个成员的定位

“消费者”指 Cargo 声明的直接内部生产消费者，optional/未装配另注明；包名省略 `pawork-` 前缀。每行 Spec 链接指向相应边界与 API 清单，实际依赖以各目录 Cargo.toml 为准。

| 包 / Spec | 应承载的职责 | 直接消费者 | 定位结论 |
| --- | --- | --- | --- |
| [domain](../spec/crates/domain.md) | canonical 类型、Provider/Tool trait、事件信封 | engine、protocol、app 等多个库 | 保留纯领域底座；禁止具体 IO、供应商和 GUI 进入 |
| [protocol](../spec/crates/protocol.md) | GUI/headless/core-api 契约、版本、共享投影 | app、cli、client | 保留共享协议边界；投影不复制到 Desktop |
| [engine](../spec/crates/engine.md) | 单 Run 模型/工具循环、审批、取消、上下文 | app、cli；生产内部依赖仅 domain | 保留执行语义；供应商差异留 providers，持久化装配留 app |
| [app](../spec/crates/app.md) | AppCore 装配、服务协调、Host 与事件持久化桥接 | cli | 保留宿主层；任务取消/终态协调应在这里统一，避免下层互相依赖 |
| [storage](../spec/crates/storage.md) | SQLite Actor、session 事件/命令账本、blob | app、cli | 保留存储边界；不接管所有领域状态机 |
| [workflow](../spec/crates/workflow.md) | Plan/Task 纯状态归约与快照 | app | 保留；名称较宽但当前 API 有消费者，不因仅一个消费者并入 app |
| [orchestration](../spec/crates/orchestration.md) | 多 worker 生命周期、预算、取消树；可选 DAG/worktree/merge | app | 保留 supervisor 主体；对未装配的可选模块作逐项保留/退出决定 |
| [control-plane](../spec/crates/control-plane.md) | tenant 策略、usage/audit、quota、credential lease/pool | app、orchestration | 保留控制语义；本地 ledger/内存 pool 与未接线 durable projection 分开描述 |
| [providers](../spec/crates/providers.md) | HTTP/SSE、供应商 wire、能力目录、计价与错误归一 | app；engine 仅 dev | 保留 adapter 边界；合并相同辅助逻辑，不合并不同 wire |
| [auth](../spec/crates/auth.md) | 凭证后端、账号、OAuth 刷新、Secret 解析 | app、tools | 保留 Secret 审计边界；credential pool 管租约，不等于凭证存储 |
| [policy](../spec/crates/policy.md) | 工具授权裁决和 workspace 路径安全内核 | app、exec、orchestration、tools、workspace | 保留公共安全内核；与 tools 合并会破坏依赖方向 |
| [exec](../spec/crates/exec.md) | 子进程树、平台沙箱、PTY | app、git、tools | 保留通用执行层；内部只依赖 policy 路径 helper |
| [tools](../spec/crates/tools.md) | AgentTool 适配、审批/调度、文件/shell/MCP/桌面操作 | app | 保留适配层；文件写安全与取消问题优先于模块整理 |
| [workspace](../spec/crates/workspace.md) | 工作区、索引、配置、资源、兼容导入 | app、tools | 保留工作区事实与资源生命周期；路径裁决委托 policy |
| [git](../spec/crates/git.md) | repo/status/diff/stage/worktree 的 Git 语义 | app；orchestration 的 optional git | 保留 Git 领域层；Stage/HunkStage 及 worktree 的产品装配按条件处理 |
| [transport](../spec/crates/transport.md) | 与业务无关的本地 framed 字节传输 | app、cli、client | 保留无内部依赖边界；连接生命周期不能由应用反复补漏 |
| [client](../spec/crates/client.md) | GUI typed client、headless SDK、响应配对与恢复 | desktop、cli | 保留客户端边界；协议超时后状态必须可解释 |
| [cli](../spec/crates/cli.md) | 命令解析、交互/JSON/ACP 通道、Host 启停 | pawork | 保留命令层；不把唯一可执行入口与库测试耦合 |
| [pawork](../spec/crates/pawork.md)（应用） | 唯一正式 Host 入口与全局脱敏 tracing | 用户/脚本；client 可 spawn | 保留最薄入口，内部仅依赖 cli |
| [desktop](../spec/crates/desktop.md)（应用） | GPUI 交互、界面状态与用户反馈 | 用户 | 保留独立进程；直接内部依赖仅 client、terminal、browser |
| [terminal](../spec/crates/terminal.md) | 终端行缓冲解析、键映射、尺寸估算 | desktop | 保留纯库；不是完整 VT emulator，也不负责 PTY 生命周期 |
| [browser](../spec/crates/browser.md) | 系统 WebView/导航/DOM 与原生视图生命周期 | desktop | 保留原生平台隔离；不因单消费者合入 GUI，不新增 JS runtime |
| [computer-use](../spec/crates/computer-use.md) | 专用虚拟桌面 RFB、观察授权与输入 | tools | 保留用户指定的独立包；不与 shell/PTY 或本机浏览器混合 |
| [testkit](../spec/crates/testkit.md) | dev-only mock 和公共契约断言 | app、client、engine、tools 的 dev 依赖 | 保留测试公共件；生产消费者为零符合定位 |

本轮没有发现必须通过整包合并解决的重复职责。app 与 desktop 的体积较大，但体积不能证明拆包必要；先围绕实际缺陷收敛内部调用关系。domain、policy、auth、protocol、transport 的边界具有明确共享或审计价值。

## 2. 值得合并或删除的局部重复

| ID | 证据与问题 | 最小收敛建议 | 进入实现的条件 |
| --- | --- | --- | --- |
| O-01 | [testkit/contract.rs](../../crates/testkit/src/contract.rs):28/46、[providers common](../../crates/providers/tests/common/mod.rs):114/132、[anthropic tests](../../crates/providers/tests/anthropic.rs):60/77 重复流契约辅助断言 | 比较断言语义后复用已有 testkit；保留通道 wire fixture 和差异断言 | 修改相关 Provider 回归时同批处理；不新增测试框架 |
| O-02 | [error_table.rs](../../crates/providers/src/error_table.rs):118 用 message 关键词分类，[retry.rs](../../crates/providers/src/net/retry.rs):14 则只返回 HTTP 状态文案 | 梳理调用点，删除普通 HTTP 路径不可达的正文规则，或消费白名单稳定错误码；不恢复原始 body | 与流错误安全收口同批核查；流内仍可达规则不能误删 |
| O-03 | [usage.rs](../../crates/control-plane/src/usage.rs):1172–1209 同步 SQLite 锁内全量取行再聚合 | 用代表性长账本测量；若有瓶颈，将等价聚合下推 SQL、避免阻塞 async runtime | 先证明延迟/内存问题，再选最小优化；保留幂等和金额精度，不搬整套存储 |
| O-04 | [Desktop session.rs](../../apps/desktop/src/controller/session.rs):53 起主会话/子代理分页相似，回执与 generation 不同 | 下次分页行为修改时共享纯页面收集部分，继续分开 UI 身份与过期回执判断 | 只有同时改两条路径且减少重复才抽取 |
| O-05 | workflow、orchestration 的 Cargo description 仍含已归档 Goal/Agent Teams；源码分别承载 plan/task 与 supervisor | 下次相关源码维护时只改 package description；修正“有 API 就有产品能力”的文档口径 | 本轮不改 Cargo；不借命名清理扩大包布局 |

同目录原子写出现在 tools、app、orchestration 等边界，各自的授权、格式和耐久要求不同。优先修复各实际漏洞；没有证据支持新建跨包文件 IO 框架。exec 与 domain 的 CancellationToken 双类型是现有边界，修复取消接线不要求统一所有类型。

## 3. 未装配模块的取舍

当前 app 使用 supervisor，但未调用 `with_task_graph` / `with_worktree_allocator` / `with_patch_merger` 接入完整工作树编排；orchestration 的 `git` feature 不由生产 workspace 成员启用。见 [supervisor](../../crates/orchestration/src/supervisor/mod.rs)、[app 装配](../../crates/app/src/orchestration_host.rs) 与 [orchestration Spec](../spec/crates/orchestration.md)。库测试不能替代产品装配证据。

| 范围 | 当前实际状态 | 后续决定与必要条件 |
| --- | --- | --- |
| TaskGraph / worktree / patch merge | 有库 API 与测试，没有当前 Host 完整装配 | 先明确要不要该产品能力。要接线则先解决下列 O-06/O-07；无活动消费者且无确定激活条件的模块按架构规则退出，不复活归档副本 |
| Git Stage/HunkStage | 无 workspace 生产调用点；Git diff/status 有真实消费者 | 绑定 GUI Git 写入的产品决定；只清理未使用模块，不删除整个 git 包 |
| durable credential pool/projection | 当前 app 为内存 pool，本地 usage ledger 持久化 | 只有恢复租约/共享实例成为需求时接入；未持久化租约本身不认定为当前故障 |
| workspace `ImportStatus::Disabled` / PromptTemplate 选择渲染 | 无相应构造/生产消费点；见 [workspace §8](../spec/crates/workspace.md) | 先核对公开 API/导入兼容，再删无用表面；不增加第二套模板系统 |

**O-06：TaskGraph 的前向引用可绕过跨租户检查。** [task_graph.rs](../../crates/orchestration/src/task_graph.rs):134–174 只在登记新任务时检查已存在的依赖。先登记 tenant A 的任务依赖尚不存在的 B，再登记 tenant B 的 B，可形成跨租户边。当前未接入 Host DAG，故作为库激活前置问题；保留模块时补反向依赖校验，用两种登记顺序验证同一拒绝结果。

**O-07：Git merge 缺少不可变 fork 基准。** [merge.rs](../../crates/orchestration/src/merge.rs):158–174 使用合并时父仓库 HEAD 读取 base，Git 读取失败还回退当前文件。父分支在 worker 工作期间提交后，冲突比较可能把新 HEAD 当旧基准；当前文件回退进一步掩盖错误。删除文件和未跟踪文件的收集也需明确语义。若激活，应保存真实 fork commit，读取基准失败明确报错，并覆盖父侧并发修改；[approve_patch](../../crates/orchestration/src/supervisor/mod.rs):405 附近先移除提案再执行，失败后的重试保留同批核对。未激活前不将其宣传为生产自动合并能力。

**O-08：Task reducer 对 Started 重放的边界。** [task/state.rs](../../crates/workflow/src/task/state.rs):112–131 会覆盖既有终态；[TaskManager::replay](../../crates/workflow/src/task/manager.rs):223 起可追加应用事件。迟到/重复 Started 能将完成任务重新变 Running，但本轮未证明当前有效快照会产生该顺序。只有支持增量/容错重放时明确拒绝或幂等语义，不能把合法新生命周期一概屏蔽。

此外，实例 ID 的跨宿主唯一性、远程 transport 的 unpublish/revoke 分工仍为条件项：分别在共享同库多宿主、恢复远程传输时复核，不计入当前产品故障数。
