# 安全与信任规格

> 基线日期：2026-08-25。本文定义 Pawork 的产品安全边界和验收要求；冻结枚举/路径语义以 [policy 源码](../../crates/policy/src)、[exec 源码](../../crates/exec/src) 与 ADR-041 为准。

## 1. 安全目标

| ID | 目标 |
| --- | --- |
| SEC-01 | 模型和外部内容不能绕过用户选择的工作区、信任状态与审批档位。 |
| SEC-02 | 明文 key/token 不进入日志、事件、session DB、usage/audit payload 或可提交配置。 |
| SEC-03 | 写文件、进程和终端等副作用可见、可拒绝、可取消，并在崩溃/重试时避免重复执行。 |
| SEC-04 | 本机 GUI 客户端必须认证，未登记/未授权协议能力 fail-closed。 |
| SEC-05 | Sandbox 的真实隔离级别与回退必须对用户可见，不夸大保护能力。 |
| SEC-06 | 事件、审批、降级与诊断可持久化/审计，恢复时不丢安全决策。 |

## 2. 资产与信任边界

| 资产/输入 | 默认信任 | 主要风险 | 控制 |
| --- | --- | --- | --- |
| Provider 输出与 tool arguments | 不可信 | 路径越界、命令注入、Secret 探测、资源耗尽 | canonical 校验、ToolDescriptor、Policy、相对路径、Sandbox、限额 |
| Workspace 内容与 AGENTS/Skills | workspace 可标 trusted/untrusted | 恶意指令、symlink、`.git` 写、外部引用 | trust gate、resource 边界、policy path、只读发现 |
| 外部 Provider/MCP | 外部系统 | token 泄漏、恶意响应、错误体回显、网络外传 | auth 分域、HTTP 错误脱敏、MCP auth 隔离、capability gate |
| 本机子进程/PTY | 高风险副作用 | 读写本机、网络访问、孤儿进程 | command risk、审批、SandboxSelector、进程树回收、PTY 创建闸 |
| GUI socket/token | 本机敏感控制面 | 未认证客户端、能力伪装、重放 | token proof、`0o700` socket 目录、版本/registry/command ledger |
| Session/Blob/Usage/Audit | 持久化敏感数据 | 篡改、损坏、Secret 落盘、分支污染 | append-only、migration/golden、Secret 扫描、PWB1、lineage、幂等 |
| auth 文件 | 最高敏感 | 明文 token 泄漏、权限过宽、损坏降级 | 独立文件、`0600`、原子 rename、损坏 fail-closed、日志脱敏 |

## 3. Policy 与审批

### 3.1 ApprovalMode

| 档位 | 预期语义 |
| --- | --- |
| `AlwaysAsk` | 可产生副作用的能力通常要求用户确认。 |
| `AskForWrites` | 写入类能力要求确认；只读能力按信任/descriptor 处理。 |
| `AskForDangerous` | 仅危险能力要求确认；安全命令可放行。 |
| `NeverAsk` | 不弹交互审批，但仍受 trust、descriptor、路径与灾难地板约束。 |
| `ReadOnly` | 默认档位；拒绝非只读能力。 |

Policy 输出固定为 `Allow`、`Deny`、`AskUser`、`AllowWithConstraints`。非 TTY 或 `--json` 场景使用 deny-all approvals；缺少交互通道时不得把 `AskUser` 静默改成 Allow。

灾难地板独立于信任和档位：即使 `trusted + NeverAsk`，`rm -rf /`、`mkfs`、`dd of=/dev/...` 等仍必须 Deny。shell 分类使用手写 tokenizer；已知限制是 `env`/`nohup`/`xargs` 等 launcher 不会被递归解包，触及该面时需重新评估。

### 3.2 工具与终端

- `ToolDescriptor` 必须准确声明 `requires_approval`、`read_only`、`allowed_in_untrusted_workspace`。
- 文件工具使用同一 policy 路径内核；执行前与 canonicalize 后均需防越界/TOCTOU。
- GUI 工具审批以 `run_id + tool_call_id` 关联并持久化；等待前先落事件，resume 不得重跑副作用。
- `terminal_create` 只有 workspace 级上下文，当前没有命令级交互审批 wire；Policy 返回 AskUser 时必须 fail-closed 为 Deny。只有满足对应档位与安全分类时才创建。

## 4. 路径安全

所有模型可控文件输入必须表示为 `workspace_id + relative_path`（资源请求另含 `root_index`）：

- 拒绝绝对路径、`..`、Windows 盘符/UNC/设备名等平台越界形态；
- canonicalize 后必须位于某个 workspace root；越 root 的 symlink 拒绝；
- `.git` 与受保护目标拒绝写入；写后/执行前做必要的再次核验；
- 资源加载不得借外部路径绕过 workspace Policy；本机会话扫描只列受控根、数量有界且不跟 symlink；
- 新调用点必须复用 `pawork-policy::resolve_workspace_path`，不得复制第四套弱路径判断。

## 5. Secret 生命周期

### 5.1 存储与解析

- 默认主凭证：`$PAWORK_HOME/auth.json`，否则 `~/.pawork/auth.json`；format v1，`0600`，原子替换，损坏即报错。
- MCP 凭证：独立 `mcp-auth.json` 与 `pawork.mcp.*` service 前缀。
- 配置 schema 无 `api_key`；env 只作受控 fallback，不回写配置。
- OS Keychain 已从当前产品移除；旧 `keychain_*` 仅保留一个版本期的读 alias，不代表 Keychain 后端存在。

### 5.2 输出与持久化

- credential/token 类型的 Debug、Display、Serialize 必须脱敏；错误消息不得复制 request body 或 token 回显片段。
- Agent 事件、opaque metadata、compat import 在持久化前进行 Secret key/值扫描；命中时拒绝或保形脱敏，不能“记录警告后继续明文落盘”。
- protected reasoning 只存于 PWB1 AEAD 信封；事件只保留 `ProtectedBlobRef`。
- 日志、usage、audit、CLI JSONL、GUI Diagnostic 与测试 fixture 均不得出现真实凭证。

### 5.3 Desktop Settings 输入

- Settings 的 API key 输入必须走待 ADR 锁定的**非重放 Secret 路径**：不得进入 command ledger payload、事件、数据库、响应 replay、诊断或 AX value。
- Desktop 只持有提交所需的瞬时缓冲，提交、取消或离开页面后清空；Host 验证成功后才原子写入既有 auth backend。
- 替换凭证失败必须保留旧条目；OAuth token 只由 Host 换取/刷新/持久化，Desktop 只见授权步骤和脱敏状态。
- 未协商 Settings capability、断线、未知 provider/auth method 或损坏状态均 fail-closed；禁止降级为 Desktop 直写 `auth.json` 或配置文件。
- 详细威胁与回归见 [settings.md](settings.md) §5；生产实现须先完成 ADR-046。

## 6. Sandbox 真实边界

`pawork-exec` 按平台探测 Seatbelt、bwrap/Landlock、AppContainer，并可回退 `NativeRestricted`：

- 回退必须形成可观察的 isolation/diagnostic 信息；ADR-031 的语义是“可观测回退”，不是一律拒绝执行。
- `NativeRestricted` **不是对抗性隔离**，不能阻止主动读取所有本机 Secret；不得在产品文案中称为强沙箱。
- macOS Enforce 当前采用“读整盘 allow + Secret 路径挖洞、写仅白名单 roots/tmp/dev，并永久拒写 `.git`/`.env`”；网络为全拒。域名 allowlist 字段已删除。
- egress broker/域名白名单代理属于候选，未实现；不能把已删除的 `network_allow_hosts` 当作可用配置。
- PTY 机制本身不做 Policy；唯一创建入口必须在 app host 先过 Policy。

平台保护能力和实机证据不完全对称。Windows Job/AppContainer、Linux 沙箱与部分真机种子仍需在相应平台/任务中复验，当前不能作为三平台发布证明。

## 7. GUI 与协议安全

- `gui serve` 在受控数据目录创建 socket 和 token；Desktop 缺 token 必须失败，不得匿名连接。
- 帧在分配前校验 1 MiB 上限；握手版本不匹配、未知 wire 名、缺 capability 均显式拒绝。
- 三通道可用性从 registry 派生；Desktop 不得通过直接依赖 app/protocol 实现绕过授权。
- command ledger 以作用域和 command ID 做幂等；相同 ID 不同 payload 冲突，不能复用历史成功结果。
- 断开 GUI 不取消 Run；安全决策与 Run 生命周期归宿主持有，不能依赖 UI 进程存活。

## 8. 安全验收最低集

任何触及 Secret、Policy、路径、进程、协议授权、持久化或破坏性动作的任务至少需要：

1. 明确资产、攻击者能力、信任边界与 fail-open/fail-closed 选择；
2. 对应安全种子的定向回归，包含允许与拒绝两侧；
3. 日志/事件/DB/fixture 的 Secret 泄漏检查；
4. 迁移、崩溃或重试场景的副作用/审批恢复证明；
5. 平台相关隔离的真实探针，缺平台时如实登记，不以 mock 代替；
6. 用户可见的拒绝/降级文案，不吞错、不静默 fallback；
7. 需要改 wire/schema/架构红线时先过 ADR。

发布级安全矩阵尚未立项；任何任务触及安全边界时仍须同批运行对应定向回归，不得推迟。Settings 的当前顺序见 [AGENTS.md](../../AGENTS.md) 与 [settings.md](settings.md)。
