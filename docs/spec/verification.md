# 验证与证据规格

> 基线日期：2026-08-31。本文定义如何证明 Spec。旧 R/Wave 结论只作历史线索；当前工作区与 [AGENTS.md](../../AGENTS.md) 优先。

## 1. 四个独立结论

任何能力都必须分别回答：

1. **是否实现**：生产调用链和用户入口是否存在？
2. **是否自动验证**：定向测试、golden、fixture 或静态断言是否覆盖核心行为？
3. **是否真实验收**：真实 Provider、真实 OS、真窗口、真实客户端或用户签字是否完成？
4. **是否可发布**：License、安装/升级、供应链、三平台和发布/回滚门禁是否明确并通过？

前一项不能替代后一项。当前未闭合验收见 [ROADMAP](../ROADMAP.md) 与 §5；历史测试通过不代表后续改动自动通过。

## 2. 证据等级

| 等级 | 定义 | 可支持的表述 |
| --- | --- | --- |
| E0 | 只有 Spec/计划，无生产实现 | 候选、已确认未排期 |
| E1 | 源码/依赖/生成物证明生产路径存在 | 已实现（代码层） |
| E2 | 定向单测、契约测试、golden、升级 fixture 或静态断言通过 | 自动化验证通过（注明命令/时点） |
| E3 | 隔离实例真实冒烟、真实 Provider/客户端/OS 或真窗口取证通过 | 对应环境验收通过（注明环境） |
| E4 | 用户人工签字或发布任务定义的全量门禁通过 | 人工验收/发布门禁通过 |

E3/E4 证据必须包含日期、环境/版本、输入范围、实际结果和可追溯位置。`/tmp` 截图或日志若未检入，只能作为当次本机证据，不能假装成仓库可复现门禁。

### 2.1 功能测试用模型

日常需要真实 Provider 的功能验证（真窗口 Run、流式输出、工具/审批、live smoke）固定使用：

| Provider | Model | 选择方式 | 禁止 |
| --- | --- | --- | --- |
| `opencode-go` | `glm-5.3-flash` | 当次 Host/CLI `--provider` / `--model` 临时覆盖 | 写入 Global/Workspace 持久默认；连接失败时改用未指定模型或伪造成功终态 |

产品示例默认仍是 `glm-coding` / `glm-5.2`。该约定只约束日常功能验证的默认真实模型，不替代四通道矩阵或发布级 Provider 门禁。证据须写明实际使用的 provider/model；网络/凭证失败按真实终态记录。

## 3. 需求追踪矩阵

| 需求族 | 实现锚 | 自动化锚 | 真实/人工锚 | 当前结论 |
| --- | --- | --- | --- | --- |
| PRD-CORE-01 / ARC-01～04 | `apps/pawork`、app/domain/engine、Cargo 依赖 | domain-only、desktop deny-list、依赖断言 | `pawork`/Desktop 冒烟 | 已实现；发布级红线矩阵未执行。 |
| PRD-CHAT-01 / CAP-CHAT-01 | cli/app/engine/providers | engine mock、provider contract、CLI tests | 四通道真实 chat 矩阵 | 已实现；四通道矩阵待专项验收。 |
| PRD-SESSION-01 / CAP-SESSION-01 | storage/app/cli | envelope、migration、export/import、projection golden | 真实 resume/fork/compact/import | 已实现；真实 fork/compact 仍有人工登记。 |
| PRD-TOOL-01 / CAP-TOOL-01 | tools/workspace/policy/exec | 九工具与路径/进程回归 | 真实仓库读写/命令冒烟 | 已实现；终局安全复跑未执行。 |
| PRD-SAFE-01 / SEC-* | policy/auth/exec/storage/app | 安全种子、Secret 扫描、Seatbelt golden、ledger 回归 | 平台探针、真实审批/PTY | 已实现；部分平台/人工项仍待验。 |
| PRD-PROVIDER-01 / CAP-PROVIDER-01 | providers/auth/app | adapter/negotiation/OAuth/脱敏测试 | ChatGPT/xAI/GLM/OpenCode 等真实请求 | 已实现；OAuth 自然临期 refresh 与真实 Anthropic/GLM Anthropic 端点待人工。 |
| PRD-SETTINGS-01 / CAP-SETTINGS-01 / SET-* | protocol/app/auth/providers/workspace/desktop | protocol golden/typegen、Secret 负断言、provider verify、config writer、Desktop/AX | Network GUI 保存、Host 重启与真实 Provider 请求；真实认证/目录矩阵 | 已实现；四家真实认证 / 目录矩阵与完整 Settings 人工签字待验。 |
| PRD-GIT-01 / CAP-GIT-01 | git/app/cli/Desktop Changes | git/checkpoint/diff 定向测试、Desktop projection | 真窗口 Changes、真实 rollback | Core 已实现；Desktop 写操作未实现，横滚人工项待验。 |
| PRD-RESOURCE-01 / CAP-RESOURCE-01 | workspace/tools/app/Desktop | resources/import/MCP contract | 外部配置、MCP stdio、真窗口 Resources | 主流程已实现；部分 GUI 出口为候选。 |
| PRD-CLIENT-01 / CAP-CLIENT-01 | protocol/app/client/cli | frame/headless/ACP golden、registry、probe | Desktop probe、Zed ACP、json-stdio | 已实现；发布级客户端矩阵未执行，probe 有已登记偶发超时。 |
| PRD-DESKTOP-01 / DESK-* | desktop/client/protocol | projection/controller、U0/U1、AX 模型/映射测试 | 真 Host/Desktop、三张阶段目标设计图、AX/IME、用户签字 | 已实现；用户验收与跨平台专项取证见 [ROADMAP](../ROADMAP.md)。VoiceOver 系统朗读验收不在范围内。 |
| PRD-OPS-01 / CAP-OPS-01 | cli ops/service、app data_dir | 路径/状态/doctor 定向测试 | macOS/Linux/Windows service 与恢复演练 | 入口已实现；无发布级三平台/恢复门禁。 |

## 4. 三类不可推迟的回归

每个测试须对应真实操作的结果、失败边界或恢复完整性；测试数量和内部常量副本不作为完成证据。按包分批检查，删除须有无效性依据或明确替代锚点。mock 服务自测只证明测试工具自身可用，不证明 Pawork 的 Provider / Host 路径通过。

| 类别 | 最低覆盖 |
| --- | --- |
| 安全红线 | 路径越界、symlink、`.git` 写、审批 deny、灾难地板、Sandbox 探测/fallback、Secret 脱敏与外部输入 Secret 拒绝 |
| 持久化与重放 | envelope、SQLite 迁移、append-only、branch lineage、PWB1、checkpoint、export/import、projection、CommandLedger 崩溃/重试 |
| 协议与解析 | GUI frame、版本协商、registry fail-closed、headless JSON、ACP、MCP、配置六层、usage dedup、外部格式解析 |

普通任务只跑写入集的定向命令；触及上述面时，对应关键回归必须同批更新，不能推迟到全量门禁。当前默认命令和单 Cargo 进程纪律见 [AGENTS.md](../../AGENTS.md) §5、§10。

### 4.1 按用途选择入口

| 改动范围 | 命令 | 证据边界 |
| --- | --- | --- |
| 文档/格式 | `bash scripts/mock/gate.sh --level 0` | ROADMAP 与当前所有改动 Markdown 的相对链接存在性、diff；不证明产品行为 |
| 包内或紧相关操作 | `bash scripts/test.sh <包名>...` | 生产代码驱动的单测/集成；自动补必要 feature，实际选择用 `--print` 查看 |
| CLI/SDK 子进程链 | `bash scripts/test.sh --host` | 先构建当前 pawork 并经 artifact 消息定位本次产物（config 的 target-dir/build.target 不会导致误测旧二进制），再运行真实 headless 子进程；缺二进制不能跳过成成功 |
| Desktop 状态/操作/布局 | `bash scripts/test.sh desktop` | GPUI 模拟操作与实际布局；不能证明系统 IME、WebKit 页面效果或真窗口像素 |
| mock/fixture/构建入口变化 | `bash scripts/mock/gate.sh --level 0,2` | 测试工具的 HTTP 回放、凭证/配置恢复、脱敏、UI 扫描与脚本调度；quota 探针对空窗、失败、过期缓存与缺失 provenance fail-closed；不算真实 Provider 验收 |
| 正式 Host/Desktop 构建 | `bash scripts/pawork-desktop.sh build` | 同一次 Cargo 调用构建两个正式二进制，按 artifact 消息校验产物；不证明真窗口行为 |
| 真实 Provider/OS/窗口/隔离桌面 | 对应包 Spec 的专项步骤 | 必须记录外部效果和前置条件；未执行不算通过 |

GUI 连接测试归 gui-server，ACP fixtures / floor 归 acp，MCP 测试归 mcp，网关 token 测试归 gateway；AppCore + HTTP / 模拟上游联调在 app。正式宿主用 `--host` 验证，包内 MockHost 不能代替。依赖边界检查使用 `cargo metadata` / `cargo tree -p pawork --edges normal`，核对无环、Desktop / Engine 边界和第三方闭包。

不使用测试数或覆盖率配额驱动删减。便宜且有独立边界意义的单测可以保留；golden 检查外部格式兼容，不属于应删除的实现副本。

UI 取证工具须提供真实失败信号：`ui-fixture.sh desktop` 清除旧 `timeline_stable` 后等待新实例就绪——barrier 必须是本进程启动后写入的有效 JSON（`settle_seq>=1`、`at_ms` 不早于启动时刻），进程提前退出、PID 归属变化或超时均失败；fixture 与 desktop 构建产物同样经 artifact 消息定位。`ui-ax-dump.swift` 无窗口、AX 权限不足、没有应用 identifier 或动作失败均非零；`ui-key-event.swift` 投递前确认目标 PID 在前台且具备事件投递权限。操作效果仍须由窗口状态与外部事实核对。

`test.sh --host`、`ui-fixture.sh` 和 `pawork-desktop.sh` 共用 [cargo-build.sh](../../scripts/cargo-build.sh) 定位产物；缺产物、构建失败均非零，测试 harness 不能当正式二进制。`--print` 不运行 Cargo；Desktop 的 `runtime_shaders` 仅在 macOS 选择，启动参数在编译前校验，内容相同的 bundle 文件不重复覆盖。Rust 子进程回归由实际 spawn / 握手判断 Host 可用。

mock 服务端点、媒体与场景回归在 [server_smoke.py](../../scripts/mock/server_smoke.py)；usage 形状共用 `capture.check_usage`，覆盖三窗、percent、日历与 1970 下界拒绝。回归覆盖实际 HTTP / 流内错误、截断、定速与复位。OAuth 自测退出时关闭线程与临时目录；mock 八通道 seed 显式指定隔离 home。L2 包含 UI 扫描回归。

### 4.2 构建时间与缓存空间

- 用 `bash scripts/test.sh --log /tmp/pawork-tests.log <包名>...` 一次保存完整 stdout/stderr；日志追加，每条命令记录耗时和退出码，失败仍非零退出。`--host` 与 `desktop` 同样支持；`--print` 不创建日志。不要用 `... | tail` 丢掉中间结果和原始退出码，也不要为补日志重复整批验证。
- 同批相关包放在一次调用中以统一 feature；单个缺陷优先按包 Spec 选 `--lib` / `--test` 和过滤器。类型检查用 `cargo check`，需要实际行为证据时仍跑测试。保持单 Cargo 进程，复用默认 `target/`。
- 构建统一经仓库 `.cargo/config.toml` 的 `rustc-wrapper = "kache"`。kache 生效时自动关闭并清理 incremental；dev/test 仍为 `debug="line-tables-only"`、`split-debuginfo="unpacked"`，不要为日常提速反复改 profile、RUSTFLAGS、toolchain 或切换链接器，避免制造不同构建指纹。kache store 须与仓库同 APFS 卷（本机 `~/.config/kache/config.toml` → `/Volumes/SSD/.kache`），容量由 kache GC 自动管理。
- target/ 死代清理继续用脚本（kache 只接管 incremental，deps/build 死代仍随指纹变化累积）：空闲时先运行 `python3 scripts/clean-stale-incremental.py --dry-run`，审阅后执行同一命令去掉 `--dry-run`；不与 Cargo 同时清理，不运行 `cargo clean`。incremental 按 7 天年龄清理，deps/examples/build 还保留每组最新 hash 构建代；年龄不是不可达证明，较少使用的 feature 组合可能需要重编译。不要每天清理仍在使用的缓存。
- 真窗口与 mock 验收结束后使用对应脚本的 stop/清理入口，并核对原实例 PID 已退出。对遗留进程先核对命令和实例再定点停止，避免后台空转持续占用 CPU。

## 5. 当前验收缺口

| 缺口 | 状态 | 完成条件 |
| --- | --- | --- |
| 配置根闭环 | 未执行 | git 根/子目录/非 git 三态与六层配置文档一致，偏差已修或登记。 |
| ChatGPT/xAI 自然临期 OAuth refresh | 待真实账号/临期窗口 | refresh → retry → success 与 `invalid_grant` 清理均有真实证据。 |
| 三类关键回归发布矩阵 | 未立项 | 发布任务明确命令和环境后执行；普通改动仍同批跑受影响的定向种子。 |
| 真实客户端/Provider 矩阵 | 未执行 | 四通道 chat、GUI/Desktop、Zed ACP、headless json-stdio、doctor 实际通过或明确 fail-closed。 |
| 真实 Anthropic、fork/compact、PTY/审批恢复等历史人工项 | 待后续任务 | 实际执行，或由用户明确接受延期并在对应 Spec/收口摘要登记。 |
| Settings 模型与供应商 | 真实矩阵与用户签字待验 | [settings.md](settings.md) 四家真实认证 / 目录矩阵及 E4 用户签字。 |
| 完整视觉与 Accessibility 签字 | 用户验收与跨平台待专项 | 当前工作台、Settings 九页、100 / 125 / 150%、键盘 / AX 与系统 IME 按对应环境取证；不沿用旧阶段签字。 |
| 发布级验证 | 未立项 | 发布不在当前任务范围；用户另行授权后先定 License，再定义三平台、供应链、安装/升级/回滚门禁。 |

## 6. 证据记录格式

每个任务收尾至少记录：

```text
Implemented: <生产路径/用户入口，或 none>
Validated: <实际执行命令、测试数/关键结果，或 none + 原因>
Targeted regressions: <覆盖的安全/持久化/协议种子，或 none>
Real-world evidence: <Provider/OS/客户端/窗口/人工签字，或 pending + 原因>
Known gaps: <未覆盖、flake、环境/凭证阻塞与登记位置>
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

禁止只写“tests passed”而不列实际命令；禁止把缺凭证后的 mock 结果写成真实冒烟；禁止把重跑即绿的 flake 隐去。

## 7. Spec 文档自身验证

纯文档任务不运行 Cargo。最低验证为：

- Markdown 相对链接解析到真实文件/目录/anchor（anchor 可按渲染器规则抽查）；
- 文档集导航无孤儿文档；状态词汇一致；
- 候选数量、版本号、命令列表和手工验收项从当前事实源复核；
- `git diff --check` 通过，diff 仅包含授权文档范围；
- 不出现真实 Secret、虚构日志或未执行的“通过”结论。
