# R7 — 执行面真隔离(T7,ADR-041)

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R7 行。根因:S4 沙箱以「诚实标签」交付(能力有限但如实上报),V2 期间未再加深。2026-08-23 波 0 实态:macOS Seatbelt 已是 deny-default 写白名单、读因 Darwin 25+ 放开整盘;`network_allow_hosts` 是无生产赋值的内存字段而非配置项(K-09,波 C 已按 D3 删除);PTY 会话不经沙箱与审批闸(gui_host 直连 `pawork-exec` PTY;波 B 创建动作已入 policy 闸);shell 风险分类靠固定词表 + 空白 tokenize(波 B 已改手写 tokenizer)。本阶段把执行面从「标签诚实」升级为「语义诚实」。安全域改动,全程定向回归护航,fail-closed 语义只紧不松。

## 1. 现状证据(2026-08-23 波 0 三路核查重验;波 A 收口后本节已二次回写为波 A 后实态,原 2026-08-18 快照多处漂移)

- macOS Seatbelt([`crates/exec/src/os/macos.rs`](../crates/exec/src/os/macos.rs):45 起):**波 A 已按 ADR-041 D1 正式化**——读 = 整盘 `(allow file-read* (subpath "/"))` 叠加 `default_secret_paths` 读+写双拒挖洞(原 :44-60 冗余系统读枚举已删);写 = deny-default 白名单(write_roots + `/tmp` + `/private/tmp` + `$TMPDIR` raw/canonical 双形态 + `/dev`),每个 write_root∪workspace_root 永久禁写 `.git`(subpath)与 `.env`(literal,均双形态);网络 Enforce 全拒。能力标签 `HardWritesAndNetwork` 词汇不变、note 已如实化;profile 整体输出由 `profile_full_output_golden` 钉死,真机行为种子 `seatbelt_enforces_formal_write_whitelist_and_holes` 护航(探测失败打印 SKIPPED 标记)。生产 policy 在 `crates/tools/src/run_command.rs`:232-252 构造(波 A 零改动)。**原「default allow + 枚举 deny」表述漂移**。
- K-09:**已按 ADR-041 D3 删除(波 C,2026-08-23)**——`SandboxPolicy.network_allow_hosts` 字段(原 `crates/exec/src/sandbox.rs`:61,非用户配置项、无生产赋值)与 os/macos.rs Enforce 分支内 K-09 注释及死分支(非空仅落 profile 注释)已移除;Enforce 下 `(deny network*)` 全拒不变,profile 输出零 diff,egress broker 转 ROADMAP §3.3 候选。**原「配置存在但全拒 + 注释在 sandbox/mod.rs」表述漂移**。
- Linux Landlock:白名单式(`crates/exec/src/os/linux.rs` 建 ruleset,读枚举 SYSTEM_READ_PATHS :15 起),嵌套 deny 无法从 allow 根做减法 → Landlock 硬拒绝 fail-closed(:819 回归;bwrap 路径为空 tmpfs 覆盖,语义不同);Landlock 无网络强制(标签 `HardFilesystemOnly`,sandbox.rs:356;bwrap 在 Enforce 下才加 `--unshare-net` :479)。Windows:AppContainer 探测恒不可用,Job Object 实施资源限制,标签 `Degraded` + 诚实 note(sandbox.rs:380-385)。**波 A 复核:两平台标签/note 如实,行为零变更(仅跨平台单测的 dead_code lint 属性行)**。
- PTY(波 B 后实态):`terminal_create` 已经 PolicyEngine 闸(crates/app/src/gui_host/handlers/terminal.rs `decide_terminal_create`;capability=Process,信任语义镜像 loop_ctx;NeverAsk/ReadOnly 直拒;AskUser 一律 fail-closed 落 Deny——用户 2026-08-23 拍板选项 A,命令级交互审批待 wire ADR,登记 ROADMAP §4),响应以 `sandboxed:false` + `policy` + `approval_mode` + note 如实标注(替换原 `uncontrolled:true` 裸语义;TerminalCreate/Write/Resize/TerminalOutput 四帧 + 响应共六 golden 钉死);创建后会话内容不逐条审批。进程组回收 Unix 靠 waiter/cleanup_handles 显式 `terminate()`,Windows 靠 Job 句柄 drop(波 B 收口 exec 64 绿含回收回归)。MCP stdio 经 `spawn_interactive` 已过软限制,语义不同。
- shell 风险分类(波 B 后实态):`crates/policy/src/shell.rs` 手写 tokenizer(单双引号/反斜杠转义/管道/重定向/`$()`/反引号/变量感知,含 `-lc` 组合短选项簇提取)作统一解析前置,固定词表保留为分类输入;引号拼接程序名、程序位变量、curl|wget 管道进 python/perl 等绕过形态收紧为 Dangerous(仅升档);灾难地板集合不变(engine.rs:56-67)。残余局限(模块 doc + ROADMAP §4 如实登记):nohup/env/xargs 等 launcher 不解包,程序位 wrapper 内危险命令不升档。
- ADR-041 决策草案与本机 Seatbelt 原型实测数据(Darwin 25.6.0)见 [docs/adr/ADR-041-sandbox-trust-model.md](../docs/adr/ADR-041-sandbox-trust-model.md)(波 0 产出,Accepted 2026-08-23)。

## 2. ADR-041 决策点(波 0;须用户确认)

1. **macOS profile 白名单化**:deny-default + 显式 allow(workspace 写、临时目录、必要系统读)——兼容性代价(Homebrew/工具链路径、Darwin 25 行为)以实测数据进 ADR;不可行处保留枚举 deny 并如实标注能力等级。
2. **PTY 信任模型**:PTY 会话入 policy 闸(spawn 前风险分类 + 审批档位适用)还是维持「PTY = 用户亲手终端,自担风险」显式豁免?推荐前者(GUI 发起的 PTY 与 agent 工具无本质区别),豁免须用户点头。
3. **K-09 egress**:三选一——(a) 实现按 host 白名单的 egress broker(代理进程/DNS 解析前置);(b) 删除 `SandboxPolicy.network_allow_hosts` 内存字段(诚实:只有 allow-all/deny-all 两档;该字段不是用户配置项);(c) 维持全拒 + 文档标注。推荐 (b)(单机产品下 host 级白名单收益低、实现重),留 (a) 为候选。
4. **shell 分类**:结构化解析(tree-sitter-bash 或手写 tokenizer)替换词表,还是保留词表 + 提高审批档位兜底?推荐手写轻量 tokenizer(管道/重定向/引号感知,不引入大依赖),分类只影响「是否升档审批」,兜底语义不变。

## 3. 波次拆分

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| 0 | ADR-041(含 macOS 白名单 profile 原型的本机实测数据)→ 用户确认 | docs/adr/ | 串行 |
| A | macOS profile 重设计 + Linux Landlock 语义对齐 + Windows 标签复核(平台探测 & fallback 语义不变:探测失败 fail-closed) | exec(sandbox/) | 串行(安全内核单一 owner) |
| B | PTY 入闸(按 ADR 决议)∥ shell 风险分类结构化 | app(gui_host PTY 装配)、exec(pty)∥ policy(shell) | 并行 ×2 |
| C | K-09 落地(按 ADR 决议);三平台定向回归 + 沙箱逃逸种子(路径越界/symlink/`.git` 写/网络外呼)全量复跑 | exec、docs(字段删除;无用户配置 schema 项可删) | 串行 |

## 4. 验证

- 安全红线定向回归全量:S3/S4 既有种子(workspace 越界、symlink 逃逸、`.git` 保护、审批 deny 不执行、探测失败 fail-closed)+ 本阶段新增(白名单 profile 下工具链可用性、PTY 闸审批、egress 决议行为)。
- 本机(macOS Darwin 25)真实验证:沙箱内 `run_command` 读写/网络行为矩阵;Linux 走 CI 或容器定向;Windows 仅编译 + 单测(F03 环境限制沿用)。
- 真实冒烟(矩阵一组):chat 内工具调用(sandboxed)+ Desktop PTY 面板一次会话。

## 5. 退出标准

- [x] ADR-041 Accepted(2026-08-23);macOS profile 按决议落地且能力标签与实际一致(波 A 验证)
- [x] PTY 按决议入闸(2026-08-23 波 B:D2 落地,AskUser fail-closed Deny 用户拍板;进程组回收回归随收口 exec 64 绿)
- [x] shell 分类按决议落地(2026-08-23 波 B:D4 手写 tokenizer;引号/管道/变量绕过种子收紧为红线回归)
- [x] K-09 字段有终局(2026-08-23 波 C:按 D3 删除字段与死分支);全平台探测语义 fail-closed 不变(Enforce 全拒、Linux/Windows 零改动,msvc 交叉 check 绿)
- [x] 安全回归全绿(2026-08-23 波 C:exec 64 绿含 Seatbelt 真机逃逸种子无 SKIPPED、tools+app 全绿、`cargo check -p pawork` 绿);Desktop PTY 面板真实冒烟登记 ROADMAP §4 人工验收;v3_plan §3 已更新

## 6. 整阶段审计(2026-08-23;grok_explorer ×4 并行只读审计 + 主代理源码复核 + grok_worker ×2 串行修复)

R7 标记 🟢 后按 v3_plan.md 惯例做完成度审计。四轨:E1 exec 核心与 ADR-041 D1、E2 消费侧与诚实标注、E3 PTY 闸与 golden/probe、E4 shell tokenizer 与波 C 残留。explorer 结论由主代理逐条源码复核;与实态不符的论断不采信(§6.2),只修实证成立且在 R7 写入集内的缺陷。

### 6.1 确认并修复(均属波 B 写入集)

- **P1 反引号地板漏检(E4)**:`take_backtick`(crates/policy/src/shell.rs)把收尾反引号推入 inner 后才 break,而 `take_command_substitution` 的收尾 `)` 不进 inner;导致 \`rm -rf /\` 的 inner 尾 token 变为 `/`+反引号,`catastrophic_single` 的 `== "/"` 不命中,NeverAsk 档 `echo \`rm -rf /\`` 静默放行(同形 `$(...)` 正确拒绝)。修法:收尾符 break 前不入 inner(`w.text` 保留原文),`command_substitution_is_recursively_classified` 补 danger+floor 断言。修复后 `cargo test -p pawork-policy --offline --lib --tests` 73 passed 全绿。
- **P2 probe 零终端场景(E3)**:波 B 收口称「probe harness 配 AskForDangerous+trusted 保持终端场景走创建路径」,实态九场景无一含 `TerminalCreate`(golden 只钉成功线,拒绝路径无场景)。修法:新增 `terminal-gate` 场景(放行路:默认档断言 terminal_session_id 非空/sandboxed=false/policy/approval_mode/note 形状;拒绝路:ReadOnly 档断言错误文案含「禁止创建终端」与「fail-closed」),harness 增 `new_with_approval` 参数化档位,握手补 `TerminalStreaming` 能力位。修复后 `cargo test -p pawork-client --test probe --offline` 10 场景全绿。

### 6.2 否决改判(explorer 论断与源码实态不符)

- E1 报告「SandboxMode::FailClosed/sandbox_for/Sandboxable」:符号不存在;实态 `SandboxSelector::pick()` 总返回可运行后端,fail-closed 是 ADR-031 可观测回退(不是拒跑)。
- E2 报告「沙箱回退文案三处不一致」:实测 engine/cli/protocol 统一为「沙箱回退：…」中文串(与两处 golden 钉死一致)。
- E2 报告「run_command.rs +88 为生产改动」:+88 全在 `mod tests`(单 hunk);生产 SandboxPolicy 构造(run_command.rs:232-252)与波 A 前字节一致。
- E4「policy 测试 73→76」:macOS 实测编译 73 条全绿;76 为含 path.rs 三条 cfg(windows) 的全源码计数,不参与本平台编译;波 B 收口记录 73 正确。

### 6.3 登记不修(入 ROADMAP §4)

- E1 P2:Seatbelt 真机探针对 `/tmp|/dev` 写洞双形态、symlink 根、`(deny network*)` 无真机验证(golden 已全文钉死 D1 形状,生产实现复核 PASS)。
- E1 P3:macOS note 未点名 `/private/tmp`;`profile_emits_deny_for_default_secret_paths` 未断言 write deny;Windows `Job::create` 资源限制映射与 Job Drop 回收无单测(前 R7,3e95eb97)。
- E3 P3:AlwaysAsk→引擎 AskUser→Deny 路径无专项单测;ADR-041 D2 正文保留时点措辞(按 v3_plan 约定 ADR 不回改)。
- ADR-041/plan/v2-summary 仍叙述 network_allow_hosts:时点/历史文档不回改;代码与 fixtures 活引用零(E4 实测)。

### 6.4 验证

- 六包矩阵 `cargo test --offline --lib --tests`(exec/policy/app/tools/cli/protocol):23 目标 658 passed / 0 failed / 1 ignored(smoke env 门控,既有登记),日志 /tmp/r7-audit-5SVi/gate.log。该门禁跑于修复落盘之后(gate.log 编译阶段重编 pawork-policy,运行窗口 23:27→23:35,与他 cargo 运行无并发),policy 目标已含修复,与 worker 专项复跑一致:policy 73 绿(/tmp/r7-fix-backtick-floor.log)、probe 10 场景绿(/tmp/r7-probe-terminal-gate4.log)。
- 已知 flake `gui_host::tests::command_record_failure_is_counted_not_swallowed`(thread-local set_default + async 偷步竞态;波 B 已登记):审计门禁复跑复现 1 次(红),同目标复跑即绿(红→绿证据链:gate2.log→flake-rerun1.log),登记不变,修复候选入 ROADMAP §4。
- `cargo check -p pawork --offline`:绿(EXIT=0,修复落盘后运行,耗时 4m32s,日志 /tmp/r7-audit-5SVi/check.log;仅 pawork-tools 既有 dead-code 警告 2 条,与本写入集无关)。
