# ADR-041:沙箱信任模型与执行面真隔离(macOS profile 形态 / PTY 入闸 / K-09 终局 / shell 分类)

- **状态**:Proposed(2026-08-23 起草,待用户确认)
- **日期**:2026-08-23

## 背景

S4 沙箱以「诚实标签」交付(能力有限但如实上报),V2 期间未再加深。R7 任务书 [plan/R7-sandbox-isolation.md](../../plan/R7-sandbox-isolation.md) 判定其为结构性债务:执行面须从「标签诚实」升级为「语义诚实」。本阶段是 V3 两个高风险契约阶段之一(R6/R7),ADR 先行。

2026-08-23 波 0 三路只读核查 + 主代理亲读源码,实态(已回写任务书,任务书 §1 证据为 R1 扁平化前快照,多处漂移):

- **macOS profile 实态已是 deny-default 白名单**(非任务书所述「default allow + 枚举 deny」):[`crates/exec/src/os/macos.rs`](../../crates/exec/src/os/macos.rs) 32-128 `generate_seatbelt_profile` 输出 `(deny default)`(:36) + process/sysctl/mach 显式 allow(:37-42);写仅 `write_roots` 获 `(allow file-write* subpath)`(:73-83);secret deny 列表叠加 `(deny file-read*/file-write*)`(:92-103);**读因 Darwin 25+ firmlink/cryptex 放开整盘** `(allow file-read* (subpath "/"))`(:62-65,注释已记载白名单读致 /bin/echo SIGABRT 134)。能力标签如实为 `HardWritesAndNetwork`(sandbox.rs 437-440)。生产 policy 在 `crates/tools/src/run_command.rs` 232-251 构造(read/write roots=workspace、network Enforce、deny=default_secret_paths,`network_allow_hosts` 走 Default 空值)。
- **K-09 实态**:`network_allow_hosts` 不是用户可配置项——workspace 配置 schema、fixtures、README 均零命中;它是 `SandboxPolicy` 内存字段(sandbox.rs:61)且**无任何生产路径赋非空值**(run_command.rs 用 ..Default::default())。唯一消费者在 os/macos.rs:105-115:Enforce 下输出 `(deny network*)`(:107),字段非空时仅落一条 profile 注释(:110-115)。决议对象是这个字段,不是「配置项」。
- **Linux/Windows**:Landlock 白名单式(os/linux.rs:118-135 建 ruleset,读枚举 SYSTEM_READ_PATHS 于 :13-29/:708-713,嵌套 deny 无法从 allow 根做减法 → 硬拒绝 fail-closed :681-693;Landlock 无网络强制,标签 `HardFilesystemOnly`(sandbox.rs:350-358);bwrap 路径才有 `--unshare-net`(linux.rs:473));Windows AppContainer 探测恒不可用,Job Object 实施 KILL_ON_JOB_CLOSE/内存/时限,标签 `Degraded` + 诚实 note(sandbox.rs:379-384)。
- **PTY 裸路径**:[`crates/app/src/gui_host/handlers/terminal.rs`](../../crates/app/src/gui_host/handlers/terminal.rs) 123-157 `terminal_create` 直接构造 `PtyCreateSpec` 调 `adapter.pty.create`,全程不经 PolicyEngine/审批,响应如实回 `uncontrolled:true, "本机不受控终端:不经沙箱与审批"`。进程组回收:Unix 靠 waiter 线程/cleanup_handles 显式 `terminate()`(非任务书所述「依赖 drop」),Windows 侧 Job 句柄 drop 触发 KILL_ON_JOB_CLOSE。另:MCP stdio 经 `spawn_interactive` 已过软限制,与 PTY 裸路径语义不同。
- **shell 分类**:[`crates/policy/src/shell.rs`](../../crates/policy/src/shell.rs) 固定词表 + 空白 tokenize + 嵌套引用兜底正则;`$(...)` 间接展开、`curl|python`、拼接变量名可漏分类。灾难地板(catastrophic)在 NeverAsk/ReadOnly 直 Deny、其余模式升 AskUser(engine.rs:56-67);**AskForDangerous 下误分类即静默放行**——分类精度只影响升档,不改变地板。

本机实测(2026-08-23,macOS 26.6.2 / Darwin 25.6.0,sandbox-exec 原型,数据进 D1):

| 实测项 | 结果 |
| --- | --- |
| 读侧全白名单(deny-default + 系统根全枚举 /usr /System /bin /sbin /dev /private /Library /Users /opt /etc /var) | `/bin/echo` SIGABRT(134);仅 `(subpath "/")` 可运行——读侧白名单在 Darwin 25+ 不可行,与仓内注释及上游 codex/srt「读=allow+挖洞」一致 |
| 写侧 deny-default 白名单(allow: workspace、tmp、$TMPDIR、/dev) | workspace 写 OK;$HOME、workspace/.git(洞)、~/.ssh 写全拒;~/.ssh 读挖洞生效 |
| 工具链兼容(上述写白名单 profile 下) | clang 编译、git status(仓内)、cargo --version、brew --version 全部通过 |
| 网络闸 | `(deny network*)` 下 curl 解析即失败(Could not resolve host);`(allow network*)` 下 200——Seatbelt 级 egress deny 有效 |
| spawn 开销 | 100 次对拍:裸 0.138s vs sandbox-exec 0.711s,约 5.7ms/次,命令执行场景可忽略 |

参照:codex `(deny default)` base sbpl + 可写根参数化 + require-not 挖洞;sandbox-runtime(srt) 写=allow-only、读=deny-then-allow 挖洞、.git/hooks/.env 永久禁写([references.md](../references.md) §7 R7 行)。

## 决策

### D1 — macOS profile 形态:写侧白名单正式化;读侧整盘 allow + 挖洞,放弃读白名单

- 写侧:维持并正式化现状 deny-default + `write_roots` 白名单;正式白名单 = workspace + 临时目录(tmp/$TMPDIR)+ /dev(本机实测 profile 的 allow 集,工具链编译依赖 TMPDIR 写),叠加 .git 写洞(workspace 根内 deny file-write*,对齐 F01「读写工具均拒 .git」语义)与 srt 式永久禁写清单(.env、.git/hooks 等,清单随波 A golden 钉死)。
- 读侧:放弃全白名单(实测不可行,见背景表),正式采用「整盘 allow + 敏感路径挖洞」;洞清单 = 现有 `default_secret_paths`(read+write 双拒,已在位)按 S13-F02 语义复核扩充。
- 能力标签:`HardWritesAndNetwork` 词汇与 note 随波 A 如实化(读=整盘 allow+洞,不再隐含读收敛);**标签/metadata.sandbox JSON 形状演进必须 golden 先行**(见「验证原则」)。
- Linux/Windows 语义对齐:macOS 读模型向 Landlock 枚举制看齐不可行(Darwin 限制),三平台标签各自如实,语义对齐目标是「写白名单 + 网络 Enforce + 标签诚实」而非读侧同构。Windows 维持 Degraded 诚实标签。
- 否决支:读侧全白名单——Darwin 25+ firmlink/cryptex 下连 /bin/echo 都无法启动,维护成本无上限;上游两家(codex/srt)读侧同样不是全 deny。

### D2 — PTY 信任模型:创建动作入 policy 闸,会话内容如实标注

- 采纳任务书推荐:GUI `terminal_create` 与 agent 工具无本质区别,**创建动作**入 PolicyEngine——以「启动交互 shell」为一次风险分类输入(capability=Process 级),适用 ApprovalMode 五档:NeverAsk/ReadOnly 下拒绝创建(fail-closed 只紧不松),AskFor* 档按风险升 AskUser。
- 创建后的会话内逐条输入不再逐条审批(与直接打开终端等价,技术上也不可行);响应与 GUI 如实标注「创建已经审批/沙箱语义,会话内容不受控」,替换现 `uncontrolled` 裸语义。
- `TerminalCreate` 响应形状(含现 `uncontrolled`/`note` 字段)当前无 golden;波 B 改形状前先钉 golden(冻结契约面)。
- 豁免支(「PTY = 用户亲手终端自担风险」维持裸路径)留给用户拍板;选豁免则本决议改为「维持现状 + 文档如实标注」,波 B 仅剩 shell 分类一轨。
- 登记(不在本决议):MCP stdio `spawn_interactive` 已过软限制;`TerminalCreate/TerminalWrite/TerminalResize`/`TerminalOutput` 帧 golden 缺口(波 B 先行补)。

### D3 — K-09 egress 终局:删除 `network_allow_hosts` 字段(选项 b)

- 删除 `SandboxPolicy.network_allow_hosts` 字段与 os/macos.rs 的 K-09 注释;`NetworkMode` 三档(Off/Hint/Enforce,默认 Enforce)语义不变——网络只有 allow-all/deny-all 两档事实,标签如实。
- 依据:字段无配置入口、无生产赋值(恒空)、唯一消费者是注释行;保留它持续制造「可按 host 白名单」的假象。
- 选项 (a)(egress broker:本地策略代理 + 沙箱内仅放行 loopback 代理端口 + 域名白名单,参照 codex-network-proxy + srt 两层模型)登记 ROADMAP §3.3 候选,激活时另立任务书。
- 否决 (c)(维持全拒+文档标注):死字段不删除,诚实标签与 API 面继续失真。

### D4 — shell 风险分类:手写轻量 tokenizer 替换空白切分

- 手写管道/重定向/引号/变量感知的 tokenizer 作为分类前置(不引入 tree-sitter-bash 大依赖);固定词表保留为分类输入。
- **分类只影响「是否升档审批」的语义不变**:灾难地板(NeverAsk/ReadOnly 直 Deny)不动;AskForDangerous 误分类静默放行的现状随波 B 收敛(tokenizer 后常见绕过形态进分类),绕过种子(引号/管道/变量)测试收紧为红线回归。
- 否决支:保留词表 + 提高审批档位兜底——审批疲劳恶化 UX,且不解决分类失真本身。

## 验证原则(各波共同遵守)

- **golden 先行面**(波 A/B 改动前必须先钉,现状均无文件 golden 或断言不足):`metadata.sandbox` JSON 形状、`IsolationLevel` 全词汇 serde/as_str、投影 `sandbox_timeline_detail` 与 CLI `sandbox_fallback_notice` 用户可见字符串、Seatbelt profile 生成器整体输出(现为 contains 式弱断言)、`TerminalCreate` 响应与 `TerminalOutput` 帧、`default_secret_paths` 清单快照。
- 安全红线种子随波落地:workspace 越界/symlink/.git 写/审批 deny 不执行/探测失败 fail-closed 既有种子全量保持;新增白名单 profile 下工具链可用性、PTY 闸审批、绕过种子(引号/管道/变量)。
- 平台探测 & fallback 语义不变:探测失败 fail-closed;ADR-031 可观测回退(标签诚实 + CLI/GUI 展示)保持。
- Policy 契约(PolicyDecision 四变体 / ApprovalPrompt+RiskLevel / ApprovalMode 五档默认 ReadOnly)形状不动;wire/信封不动。

## 后果

- 波 A(exec `sandbox/`):macOS 读洞/写洞与标签如实化 + Linux 语义对齐复核 + Windows 标签复核;串行,安全内核单一 owner。
- 波 B(并行 ×2):PTY 创建入闸(app gui_host PTY 装配 + exec pty)∥ shell tokenizer(policy shell)。
- 波 C:K-09 字段删除(exec + 文档)+ 三平台定向回归 + 沙箱逃逸种子全量复跑。
- ADR-041 Accepted 是波 A 开工前提;本 ADR 不含 schema/wire 变更。

## 相关

- [plan/R7-sandbox-isolation.md](../../plan/R7-sandbox-isolation.md)(任务书:决策点、波次拆分、验证、退出标准)
- [ROADMAP.md](../../ROADMAP.md) §2 R7 行、§3.2 K-09
- [v2-summary.md](../v2-summary.md) §4/§5(ADR-031 可观测回退、S13-F01/F02 .git 拒写与诚实标签)
- [design.md](../design.md) §3.2(Policy 冻结契约)、§4 S3/S4 行
- [references.md](../references.md) §7 R7 行(codex sandbox / srt 策略语义 / codex-network-proxy)
