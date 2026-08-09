# Desktop GUI

## 职责

`apps/desktop` 是 Pawork 的独立 Tauri + React 客户端，为本地与远程 `pawork` Host 提供可访问、可恢复的 Coding Agent 图形界面。它负责连接、展示和用户交互，不负责 Agent/Provider/Tool/Git 业务执行，也不保存权威业务状态。

## 设计要点

- **独立进程**：Desktop 不嵌入 Core，只经 `gui-client` / GUI Connection Protocol 连接 `pawork`。
- **单一事实源**：Snapshot 初始化 projection，严格按 `global_sequence` 应用 Event；重复事件幂等、缺口先补发，无法补齐则重新取 Snapshot。
- **命令回写**：所有业务动作构造 `AppCommandEnvelope`；pending UI 与最终 Event 分离，Core revision/Policy 结果优先。
- **安全 WebView**：最小 Tauri capability、严格 CSP、无远程脚本/raw HTML/通用 shell/fs/http/sql；Secret 与 Protected Blob 不进 renderer。
- **大数据有界**：Timeline、Diff、日志和列表虚拟化；Artifact/Terminal 分块流式，bounded queue 与 backpressure 可见。
- **多 GUI 一致**：CLI、本地窗口、远程 GUI 同时操作时显示 actor/source、revision conflict 和最新权威结果，不做客户端点对点同步。
- **渐进解锁**：页面按 capability snapshot 和 Core 已实现契约显示；不以静态假数据冒充可用能力。

## 信息架构与任务映射

| Surface | 主要能力 | 任务 |
| --- | --- | --- |
| Connection / App Shell | instance 发现、启动/连接、认证、状态、更新 | P19-1、P19-14、P19-15 |
| Navigation | Workspace、Session/Branch、搜索、恢复位置 | P19-4 |
| Timeline | user/assistant/thinking/tool/server-tool、citation、Artifact、stream | P19-5 |
| Composer | prompt、`@file`、附件、model/profile、send/cancel | P19-6 |
| Safety | Approval、Policy explanation、Workspace Trust、actor/revision | P19-7 |
| Changes | Diff、stage/unstage/discard、Checkpoint/Rollback、Review | P19-8 |
| Terminal | PTY、resize、reconnect、process/monitor output | P19-9 |
| Settings | ProviderAccount/Auth/Model/Quota/Usage/Tenant policy | P19-10 |
| Resources | AGENTS/Skills/Prompts/Profile、MCP、Plugin、Diagnostics | P19-11 |
| Workflow | Plan/Goal/Task/Automation/Monitor/Memory | P19-12 |
| Orchestration | Worker/TaskGraph/Budget/Merge/Teams | P19-13 |

## 状态与接口模型

```text
connect → handshake/auth → snapshot(N) → subscribe(after=N)
                                      ↓
renderer projection ← validated events N+1..M ← Tauri bridge
        ↓ pending command(command_id, expected_revision)
        └──────────────────────────────→ Core → response/event
```

Projection 至少分为 `connection`、`workspace`、`session`、`run`、`approval`、`diff`、`terminal`、`provider`、`resource`、`workflow` 与 `presence` slices。每个 slice 记录最后应用的 sequence/revision；本地持久化只包含主题、字号、面板尺寸、最近连接与无敏感信息的导航位置。

## 优先级（P0–P2）

- **P0**：P19-1～P19-9 与 P19-16 的主路径——连接、状态恢复、Workspace/Session、Timeline、Composer、Approval、Diff、Terminal、accessibility/security/performance gate。
- **P1**：P19-10～P19-12、P19-14、P19-15——账号/额度、资源/扩展、现代 Workflow、多窗口/远程、签名分发。
- **P2**：P19-13 的高级编排视图、复杂可视化布局、离线只读缓存与主题生态；不得阻塞基础 Desktop 发布。

## 验收标准

- [ ] Desktop 未链接任何 Core 业务 crate，关闭 GUI 不取消 Core 中的 Run/Task
- [ ] 从空 projection 经 Snapshot + Event Replay 重建与 Core 一致，重复/缺口/乱序/陈旧响应有确定行为
- [ ] 本地与远程 GUI、CLI 同时操作时 actor/revision 可见，审批和破坏性动作 fail-closed
- [ ] Timeline、100k Diff 与 Terminal stream 使用虚拟化/分块/backpressure，达到 [性能目标](../quality/performance-targets.md)
- [ ] 全部 P0 路径可键盘操作、读屏可理解、焦点与 reduced-motion 正确
- [ ] Tauri capabilities/CSP、内容 scheme、Secret/日志/本地存储与 updater 签名通过 [安全验收](../quality/security-acceptance.md)
- [ ] Windows、macOS、Linux 原生壳 E2E/visual/a11y 证据通过；浏览器 Mock 只作快速补充
- [ ] 三平台安装包可复现构建并验证签名/升级/失败回退

## 相关文档

- [ADR-034 Desktop GUI Client 边界](../adr/ADR-034-desktop-gui-client-boundary.md) · [GUI 连接](gui-connection.md) · [CLI Host](cli-host.md)
- [GUI Connection Protocol](../architecture/api-surface.md) · [workspace 结构](../architecture/workspace-layout.md)
- [性能目标](../quality/performance-targets.md) · [安全验收](../quality/security-acceptance.md) · [测试体系](../quality/testing.md)
- [ROADMAP Phase 19](../../ROADMAP.md)
