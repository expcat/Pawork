# 本机运行与运维规格

> 基线日期：2026-08-25。本文覆盖开发/本机实例的运行面；Pawork 尚无获授权的正式发行、安装器或发布级恢复 runbook。

## 1. 运行拓扑

| 模式 | 启动面 | 宿主关系 | 主要输出 |
| --- | --- | --- | --- |
| 交互 CLI | `pawork chat` | 当前进程加载 AppCore | TTY 流式文本/状态 |
| 单次 Run | `pawork run` | 当前进程加载 AppCore | 文本或 JSONL |
| GUI Host | `pawork gui serve` | CLI 进程加载 AppCore 并监听本机 socket | GUI protocol frames；日志 stderr |
| Desktop | `pawork-desktop` | 独立 GPUI 进程，经 `pawork-client` 连接 GUI Host | 单窗口 UI |
| Headless | `pawork headless --json-stdio` | 当前进程加载 AppCore | stdout-only JSONL |
| ACP | `pawork acp serve` | 当前进程加载 AppCore/ACP host | ACP stdio/连接语义 |
| Service | `pawork service install/start/stop` | 管理 GUI/service 启动形态 | 默认 dry-run；显式 apply 才变更系统 |

同一实例的持久化和 Run 生命周期由宿主持有。关闭 Desktop 不等于停止 Run；关闭宿主前应先查询状态并按用户意图取消/等待任务。

## 2. 数据目录与实例

数据目录选择以 `pawork doctor` 实际输出为准：`PAWORK_DATA_DIR` 覆盖 → 平台默认应用数据目录 → HOME 缺失时可观察回退。实例名默认 `default`，经规范化后形成独立实例目录。

| 路径 | 用途 | 备份属性 |
| --- | --- | --- |
| `<data_dir>/<instance>/session.db` | 会话、事件、投影物化、CommandLedger | 核心；SQLite 一致性快照 |
| `<data_dir>/<instance>/artifacts/` | checkpoint/rollback 内容寻址 Blob | 与 session 同批备份 |
| `<data_dir>/<instance>/protected/` | PWB1 protected reasoning | 与 key/引用关系同批备份；不可当明文读取 |
| `<data_dir>/<instance>/usage-ledger.sqlite3` | 用量账本 | 运维/审计数据 |
| `<data_dir>/<instance>/audit.jsonl` | 审计记录 | 追加日志；注意敏感元数据边界 |
| `<data_dir>/<instance>/tasks.json` | 后台任务快照 | 可恢复状态 |
| `<data_dir>/pawork-gui.sock` 或命名实例变体 | 本机 GUI socket（Unix；Windows 对应 named pipe） | 运行时文件，不备份 |
| `<data_dir>/gui.token` 或命名实例变体 | GUI 客户端认证 token | Secret；不要进入普通文档/日志/仓库备份 |
| `<data_dir>/<instance>/gui-serve.pid` | GUI host pid 记录 | 运行时文件，不作为存活唯一事实源 |

主 Provider Secret 默认不在上述实例目录：`$PAWORK_HOME/auth.json`，否则 `~/.pawork/auth.json`；MCP 使用独立 `mcp-auth.json`。两者均属于高敏 Secret 备份，必须独立加密和限权。

## 3. 配置与凭证

配置优先级固定为：

```text
Builtin < Global < Profile < Workspace < Session < Run
```

- 工作区配置位于仓库根 `.pawork/` 体系；从 git 子目录/非 git 目录发现根的闭环仍需专项复核。
- CLI provider/model/instance/approval 覆盖属于运行入口，不应写回包含 Secret 的配置。
- 功能测试用模型固定为 `opencode-go` / `glm-5.3-flash`：用当次 `--provider opencode-go --model glm-5.3-flash` 覆盖；不要把该对写入持久默认。产品示例默认仍是 `glm-coding` / `glm-5.2`。详见 [verification.md](verification.md) §2.1。
- `ProviderConfig` 无 `api_key`；凭证通过 `pawork auth` 或受控 env fallback 提供。
- 配置/会话导入先 dry-run/预览；扫描不得执行 hook、启动 MCP 或联网。
- 不要手工编辑损坏的 auth JSON 以“跳过”错误；应保留原文件、用受控恢复/重新登录重建。

首次使用前建议按顺序确认：

1. `pawork --help` 与子命令 `--help` 对应当前二进制；
2. 用 `pawork auth` 配置所需通道，避免把 token 放进仓库配置；
3. `pawork models` 验证运行期模型目录；
4. `pawork doctor --json` 确认 data_dir、instance、DB/socket/handshake；
5. 从 `ReadOnly` 或最小必要 approval mode 开始，再按任务提高权限。

## 4. GUI Host 运维

- `gui serve` 创建/读取受控 token，socket 目录权限为 `0o700`；自定义 socket 若位于数据目录外，需明确其权限与生命周期。
- Desktop 支持 `--socket`、`--instance`、`--probe`、`--probe-smoke`；生产使用缺 token 必须失败。
- Desktop Settings「高级」页只读显示当前连接状态、Host runtime ID、协商 API/capabilities、启动 endpoint、resume/ack，并在断线态复用 Reconnect；runtime ID 不是配置 instance。该页不显示 token/token path、不推断 data directory，也不替代或运行 `doctor`。
- host idle timeout 30s，Desktop 约 15s heartbeat；持续断线先检查 host 进程、socket/token、实例名和 handshake，而不是重建 DB。
- `status`、`watch`、`shutdown`、`doctor` 在加载完整 AppCore 前运行，适合诊断宿主不可启动的情形。
- pid 文件是提示，不是权威活性检测；以进程、socket 握手和 doctor 结果交叉确认。

## 5. 备份与恢复

当前没有发布级、跨平台自动备份命令。开发/本机备份遵守以下最小规则：

1. 先确认没有活动 Run/写事务；优先正常停止对应 host，而不是复制运行中的一组互相关联文件。
2. 同一实例的 `session.db`、`artifacts/`、`protected/`、usage/audit/tasks 作为一个时间点集合处理。
3. GUI socket、pid 不备份；GUI token 和 auth/MCP auth 作为单独高敏材料处理，不进入普通源码仓库或共享归档。
4. 恢复到隔离的 `PAWORK_DATA_DIR`/instance，先运行 `doctor`、只读 `sessions list/show`，再进行写操作。
5. 旧 DB 只通过内置 migration 打开；migration 失败保留原副本并停止，不手工改 schema_version。
6. export v3 可用于会话级迁移，但不能替代 artifacts/protected/usage/audit 的完整实例备份。

发布任务必须补充：一致性快照实现、恢复演练、版本降级策略、损坏库处理、Secret 轮换、RPO/RTO 与三平台证据。

## 6. 故障诊断顺序

| 症状 | 先检查 | 禁止的“修复” |
| --- | --- | --- |
| CLI 无法加载 | `doctor --json`、data_dir、config、auth 损坏、session migration | 删除整个数据目录、手改 schema_version |
| Desktop 无法连接 | host 状态、instance、socket、token、API 版本/handshake | 禁用认证、让 Desktop 直连 AppCore |
| Run 卡在审批 | 当前 approval mode、TTY/JSON 模式、pending event、GUI/CLI resume 差异 | 在无人值守模式自动 Allow |
| 命令被拒或降级 | Policy reason、trust、command risk、Sandbox isolation/note | 隐藏 fallback、将 NativeRestricted 称为强隔离 |
| Provider 401/429/超时 | `auth list` 脱敏状态、模型目录、Retry-After、通道配置 | 打印 token/request body、静默换用别的账号 |
| 会话/导入失败 | 文件版本、Secret 检测、损坏行、branch lineage | 部分导入后宣称完整成功 |
| usage 冲突告警 | history/backlog 中的 record_id/dedup 登记、重试队列 | 清空账本以消除告警 |

## 7. 服务、平台与发布边界

- `service` 默认 dry-run；只有用户明确选择并传入 apply 语义时才修改系统服务。
- Windows SCM 实机验收、Linux/Windows Sandbox/服务矩阵未构成当前发布证明。
- License 仍待定；crates.io 占名、安装器、自更新、签名、公证、SBOM、供应链与发布回滚均未立项。
- 当前 Settings 活动线明确不运行发布级 Workspace Full Gate。正式发布必须经用户另行授权后重新定义，不能沿用历史默认门禁。
- 远程 GUI、LAN/Web、Cloud 和外部账户池网关未交付；不要把本机 socket 服务暴露为远程控制面。

## 8. 运维完成条件

本机开发可用的完成条件是：凭证可解析、模型可列出、doctor 正常、目标模式可启动、拒绝/降级可见、数据可恢复。正式运维完成条件尚未定义；只有发布任务通过 License、安装/升级、备份恢复、三平台和供应链门禁后，才能把本文件升级为发布 runbook。
