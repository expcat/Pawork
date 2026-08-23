# pawork-policy

安全内核：路径越界、shell 风险分类、审批决策。依赖 `pawork-domain`。ADR-039 不合并清单成员。

## 职责

把「工具能力 + 输入 + 信任状态 + 审批档位」映射为冻结的 `PolicyDecision`。文件工具的唯一路径入口在本包：`workspace_id` 语义下的相对路径解析、symlink / `.git` / TOCTOU 再检查。不执行命令、不碰 Git、不依赖 `pawork-exec`。

## 模块树

```
src/
  lib.rs
  decision.rs    # PolicyDecision / ApprovalPrompt / RiskLevel
  mode.rs        # ApprovalMode
  engine.rs      # PolicyEngine::decide
  path.rs        # resolve_workspace_path
  shell.rs       # classify_command(手写 tokenizer 前置,R7 波 B)
```

无 `tests/` 目录；红线回归在各文件 `#[cfg(test)]`。

## 对外入口/API 面

crate 根 re-export（无私有 `pub mod`）：

- `ApprovalMode`：`AlwaysAsk` / `AskForWrites` / `AskForDangerous` / `NeverAsk`（serde alias `"on_failure"` 只进不出）/ `ReadOnly`（默认）。严格度：`NeverAsk` < `AskForDangerous` < `AskForWrites` < `AlwaysAsk` < `ReadOnly`。
- `PolicyDecision`：`Allow` / `Deny { reason }` / `AskUser { prompt }` / `AllowWithConstraints { constraints }`。
- `PolicyEngine` + `PolicyInput`（`capability`、`input`、`trusted`、`allowed_in_untrusted_workspace`、`approval_mode`）。
- 路径：`resolve_workspace_path(roots, relative)` → `ResolvedPath`；`PathSafetyError`（`AbsolutePath` / `Traversal` / `SymlinkEscape` / `GitInternals` 等）；另有 `canonicalize_platform` / `path_within_root` / `relative_to_root`。
- `classify_command(program, args) -> CommandRisk`（`Safe` | `Dangerous`）。
- `hits_danger_floor` 为 `pub(crate)`，不是公开 API。

## 依赖与被依赖

- **依赖**：`pawork-domain`。`serde` / `dunce` / `regex`。无 feature。**不**依赖 exec / tools / workspace。
- **被依赖**：`pawork-tools`、`pawork-workspace`、`pawork-app`。
- `pawork-exec` 刻意不依赖本包（路径小复制在 exec 内部）。

## 红线与注意事项

- 冻结契约：`PolicyDecision` / `ApprovalMode`；安全回归不推迟。
- `resolve_workspace_path` 是文件工具唯一入口：拒绝对路径、`..`、越 root 的 symlink、`.git`、非普通文件；canonicalize 后再查（TOCTOU）。
- 灾难地板：即使 `trusted + NeverAsk`，`rm -rf /` / `mkfs` / `dd of=/dev/...` 仍 `Deny`。
- 未信任 workspace：除非 `allowed_in_untrusted_workspace`，否则 deny。
- `ReadOnly` **能力**过信任门后 Allow；`ReadOnly` **档位**拒绝非只读能力。
- `Deny.reason` 不得含 Secret。旧 CLI 档位 `on-failure` 只读入映射 `NeverAsk`。
- shell 分类解析层为手写 tokenizer(引号/转义/管道/重定向/`$()`/变量感知,R7 波 B),固定词表保留为分类输入;灾难地板集合不变;launcher(env/nohup/xargs)不解包为已登记残余局限。
- PTY 创建入闸已于 R7 波 B 落地(闸在 app gui_host,capability=Process;AskUser fail-closed 落 Deny);macOS 白名单 profile 已于 R7 波 A 正式化。

## 相关文档

- [docs/design.md](../../docs/design.md) §3.2 Policy 行
- [plan/R7-sandbox-isolation.md](../../plan/R7-sandbox-isolation.md)
- [AGENTS.md](../../AGENTS.md) §8
- [代码地图总索引](../../docs/code-map/README.md)
