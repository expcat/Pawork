# UI Fixture（R1 Wave B · 固定真实 fixture）

本目录是 Pawork R1 视觉合同测试的仓内资产：一个确定性、隔离、无真实
凭证的 Host 数据集定义，经真实 Host / GUI Connection Protocol /
projection 进入 Desktop。fixture 数据只通过协议到达 UI，生产代码中
不存在 `if demo` 分支；本目录内容不进任何生产二进制。

## 1. 目录布局

仓内资产（入库）：

| 文件 | 说明 |
| --- | --- |
| `seed.json` | 声明式数据集定义（schema v1，见 §2） |
| `pty-fixture.sh` | 确定性 PTY 程序（见 §5） |
| `README.md` | 本文档 |
| `expected/snapshot.json` | Phase C 由 snapshot-dump 生成并提交的期望快照 |

运行时 fixture root（一律不入库，示例中以 `<root>` 指代）：

```text
<root>/.pawork-ui-fixture   marker JSON（preparing → ready；
                            serve/self-check 只接受 ready）
<root>/manifest.json        {fixture_version, now_ms, seeded_at_ms, 文件摘要}
<root>/data/                隔离数据目录语义（session.db、gui.token、
                            pawork-gui.sock、checkpoints/ 等）
<root>/workspaces/<name>/   真实 workspace 目录（git 仓或非 git）
<root>/pty/pty-fixture.sh   seed 时从仓内 fixtures/ui/pty-fixture.sh 拷入
<root>/barriers/            barrier 文件目录（可用 PAWORK_UI_BARRIER_DIR 覆盖）
<root>/logs/                serve / self-check / desktop 日志
```

## 2. seed.json schema v1（冻结）

时间锚点：默认 `FIXTURE_NOW_MS = 1767225600000`（2026-01-01T00:00:00Z），
可被 `seed --now-ms` 覆盖；所有 offset 相对锚点重锚。workspace 路径只允许
`${ROOT}/<relative>`，diff/tool/PTY 路径只允许不含 `.` / `..` 的相对路径；
seed 在任何写入前校验引用、枚举与路径。
git 基线固定作者/日期/消息，并隔离用户/系统 Git 配置及 `GIT_*` 路由环境，
不触发全局签名或 hook，也不能被环境重定向到 fixture root 外。

```json
{
  "fixture_version": 1,
  "now_ms": 1767225600000,
  "workspaces": [
    {"id": "...", "name": "...", "path": "${ROOT}/workspaces/<name>", "git": true}
  ],
  "sessions": [
    {
      "id": "...", "workspace_id": "...", "title": "...",
      "created_offset_ms": -7200000, "updated_offset_ms": -600000,
      "state": "completed|failed|cancelled|pending_approval|tool_failed",
      "turns": [
        {
          "user": "...",
          "assistant": ["段1", "段2"],
          "stream_chunks": 3,
          "tools": [{"name": "...", "status": "pending|running|succeeded|failed",
                     "path": "...", "error": "..."}],
          "usage": {"input": 0, "output": 0},
          "stop": "completed|failed|cancelled"
        }
      ]
    }
  ],
  "diffs": [
    {"workspace_id": "...", "session_id": "...",
     "files": [{"path": "...", "action": "modified|added|deleted", "long_line": false}]}
  ],
  "pty": {"script": "pty-fixture.sh"}
}
```

字段说明：

- `title`：至少一个 200+ 字符超长样例，验证标题截断与窄窗表现。
- `created_offset_ms` / `updated_offset_ms`：相对锚点的负偏移；
  Today ≈ -2h，Yesterday ≈ -26h，Previous 7 days ≈ -3d，Earlier ≈ -10d。
- `state`：见 §3 状态矩阵；`turns[].stop` 是该 turn 的终态。
- 事件形状与 domain golden 一致；envelope 构造参照
  `crates/storage/src/session/test_support.rs` 与
  `crates/app/tests/timeline_projection_host.rs`。

## 3. 状态与数据覆盖矩阵（brief §4）

| 维度 | 覆盖 |
| --- | --- |
| workspaces | alpha-app（git，含 diff）、beta-lib（git，干净）、gamma-notes（非 git → Changes 诚实空态） |
| sessions | ≥6 个，跨 Today / Yesterday / Previous 7 days / Earlier 四桶，分布 ≥2 个项目；gamma 承担空项目态 |
| 标题 | 含 1 个 200+ 字符超长标题；含 ≥50 条 timeline 条目的长会话（供虚拟化） |
| completed | user 多段文本 + markdown 列表 + assistant 流式多 chunk（reducer 合并）+ tool succeeded + usage + completion 终态 |
| failed | provider 失败终态 → Error 条目 |
| cancelled | 取消终态 |
| pending_approval | ToolCallStarted + ToolApprovalRequested 无响应（snapshot 重建审批卡路径） |
| tool_failed | tool 执行失败终态 |
| tool 状态 | pending / running / succeeded / failed 全覆盖 |
| diffs | alpha 仓 4 个文件：modified×2（其一含 >200 字符长行，供横滚）/ added / deleted；working tree 与事件一致 |
| Terminal | 创建（TerminalCreate）、输入（回显）、输出（固定 banner + 回显）、resize（PtyResize）、停止（exit 输入或信号） |

已知能力缺口（诚实记录）：domain 当前无 unread 概念，fixture 不含未读态。
gamma-notes 的「空项目态」目前只到 host 侧：snapshot 的 workspaces 段
按 wire 现状只携带主 workspace（crates/app/src/gui_host/mod.rs），Desktop
Projects 视图可见项目为 alpha-app / beta-lib；gamma 要对 Desktop 可见需
扩 wire（workspaces 段携带全部已注册 workspace），属冻结契约演进，须先
过 ADR，暂记为已知缺口（R1-D 起引用本段）。

## 4. serve 模式 MockProvider 脚本（dev-only）

`ui_fixture serve` 使用 `pawork_testkit::MockProvider`，按 user 文本
首行前缀分派（脚本只存在于 dev example，不进任何生产二进制）：

| 首行前缀 | 行为 |
| --- | --- |
| （默认） | 3 个 TextDelta chunk + usage + completed |
| `fixture:hang` | wait_for_cancellation（供 running 态 / 取消场景） |
| `fixture:fail` | 立即 ProviderError（供 failed 场景） |
| `fixture:tool` | 单个 read_file tool_call（首请求 ToolUse 终态；工具结果回传后 completed） |

## 5. PTY fixture（确定性）

`pty-fixture.sh` 是冻结的确定性程序：启动输出固定 banner，逐行回显
输入（前缀固定），仅 `exit` 输出固定收尾行后以 0 退出（`quit` 等其它
输入走回显分支）；SIGTERM /
SIGINT 按默认语义终止。输出不含日期、随机数、主机名或环境信息，同输入
必然逐字节一致（验证：两次运行 sha256 相同）。resize 只改变窗口尺寸，
不改变输出内容。

## 6. Barrier 语义（冻结；文件名即合同）

所有 barrier 为文件，内容 JSON `{at_ms, detail}`；读侧轮询存在性 /
内容，禁止固定 sleep 猜稳定时机。目录默认 `<root>/barriers`，可用
`PAWORK_UI_BARRIER_DIR` 覆盖。

| 文件 | 写入方 | 语义 |
| --- | --- | --- |
| `host_ready` | example serve | bind 完成后写；读方据此确认 host 可连接 |
| `drop_socket.request` | 手动取证方 | 请求 host drop 全部连接句柄（保持监听） |
| `drop_socket.done` | example serve | 已完成 drop，读方据此继续 |
| `serve_stop.request` | 手动取证方 | 请求 host 优雅停机（abort accept 任务后 close listener） |
| `replay_complete` | example self-check | 二次连接 Resume Replay 验证通过后写 |
| `timeline_stable` | Desktop（W4） | timeline 稳定（settle_seq 单调自增、session_id、entry_count） |
| `approval_visible` | Desktop（W4） | 审批卡可见（含 tool 名）；消失时由 Desktop 删除该文件 |

Desktop 在开始连接、打开会话或收到任一 ControllerEvent 时先删除上一轮
`timeline_stable` / `approval_visible`，只在新的静默窗口重新写入，读方不得
把旧生命周期遗留的存在性当成本轮完成。

## 7. 直接运行 fixture

独立驱动和敏感信息扫描脚本已移除；数据集与 Rust example 保留。
必须显式指定隔离 `--root`，禁止指向默认数据目录。example 支持的 CLI：

```bash
cargo run -p pawork-app --offline --features ui-fixture --example ui_fixture -- seed --root <dir> [--now-ms <i64>]
cargo run -p pawork-app --offline --features ui-fixture --example ui_fixture -- serve --root <dir>
cargo run -p pawork-app --offline --features ui-fixture --example ui_fixture -- self-check --root <dir>
cargo run -p pawork-app --offline --features ui-fixture --example ui_fixture -- snapshot-dump --root <dir> --out <file>
```

自动化验收先用相同 feature 执行 `cargo build --example ui_fixture`，从
Cargo `--message-format=json` 的 `compiler-artifact.executable` 取本次
产物路径，再直接启动产物，避免长期存活的 `cargo run` 包装进程占用 Cargo。
`serve`、`self-check` 和 Desktop 需独立终端/进程，不能并发运行 Cargo。
需要 Desktop 时先运行 `scripts/build.sh`，再以 fixture 的 socket/token
和隔离数据目录启动该次构建的 Desktop；`scripts/start.sh` 只启动正式实例。

### expected/snapshot.json 再生

期望快照由 `snapshot-dump` 在干净 seed 上生成（volatile 字段归一化由子命令
内建：instance_id / generated_at 换占位、session_tree 过滤到 seed 会话）。
desktop 的 `ui_fixture_expected_snapshot_rebuilds_groups_and_status` 单测
直接读取该文件断言 TaskRail 四桶分组与 pending 审批状态：

```bash
ROOT="$(mktemp -d /tmp/pawork-ui-fixture.XXXXXX)"
cargo run -p pawork-app --offline --features ui-fixture --example ui_fixture -- seed --root "$ROOT"
cargo run -p pawork-app --offline --features ui-fixture --example ui_fixture -- snapshot-dump \
  --root "$ROOT" --out fixtures/ui/expected/snapshot.json
```

seed.json 数据集或 host snapshot 段结构变化后须再生并同批提交；再生不得
携带 `--now-ms`（golden 固定使用默认锚点 `FIXTURE_NOW_MS`）。

停止和清理由取证方负责：先请求 `serve_stop.request` 或停止自己启动的
进程，核对 PID 与 fixture root 后再处理临时目录；只清理自己创建且具有
`.pawork-ui-fixture` marker 的精确 root。使用 `/tmp` 短路径，避免 Unix
socket 长度限制。`scripts/clean.py` 只处理 Cargo 构建产物，不处理 fixture 数据。

## 8. 安全与红线

- 不访问网络、不使用真实凭证；占位 token 形状文本（例如伪造的 key 样例）
  一律禁止出现；检入前核对 token 前缀、key 赋值与 64 位 hex token 形状。
- fixture data 目录禁止出现 auth.json；seed.json 路径只允许 `${ROOT}`
  占位；文档中的用户目录示例一律写 `/Users/<name>`、`/home/<name>` 占位形。
- Secret（明文 token）不写入数据库、日志与取证报告。
- 本目录资产是测试输入，不改变生产 UI 行为；发现生产代码出现 demo 分支
  属于架构红线违规。
