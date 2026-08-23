# R7 — 执行面真隔离(T7,ADR-041)

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R7 行。根因:S4 沙箱以「诚实标签」交付(能力有限但如实上报),V2 期间未再加深。2026-08-23 波 0 实态:macOS Seatbelt 已是 deny-default 写白名单、读因 Darwin 25+ 放开整盘;`network_allow_hosts` 是无生产赋值的内存字段而非配置项(K-09);PTY 会话不经沙箱与审批闸(gui_host 直连 `pawork-exec` PTY);shell 风险分类靠固定词表 + 空白 tokenize。本阶段把执行面从「标签诚实」升级为「语义诚实」。安全域改动,全程定向回归护航,fail-closed 语义只紧不松。

## 1. 现状证据(2026-08-23 波 0 三路核查重验;波 A 收口后本节已二次回写为波 A 后实态,原 2026-08-18 快照多处漂移)

- macOS Seatbelt([`crates/exec/src/os/macos.rs`](../crates/exec/src/os/macos.rs):45 起):**波 A 已按 ADR-041 D1 正式化**——读 = 整盘 `(allow file-read* (subpath "/"))` 叠加 `default_secret_paths` 读+写双拒挖洞(原 :44-60 冗余系统读枚举已删);写 = deny-default 白名单(write_roots + `/tmp` + `/private/tmp` + `$TMPDIR` raw/canonical 双形态 + `/dev`),每个 write_root∪workspace_root 永久禁写 `.git`(subpath)与 `.env`(literal,均双形态);网络 Enforce 全拒。能力标签 `HardWritesAndNetwork` 词汇不变、note 已如实化;profile 整体输出由 `profile_full_output_golden` 钉死,真机行为种子 `seatbelt_enforces_formal_write_whitelist_and_holes` 护航(探测失败打印 SKIPPED 标记)。生产 policy 在 `crates/tools/src/run_command.rs`:232-252 构造(波 A 零改动)。**原「default allow + 枚举 deny」表述漂移**。
- K-09:`network_allow_hosts` 是 `SandboxPolicy` 内存字段(`crates/exec/src/sandbox.rs`:61),**非用户配置项**(workspace schema/fixtures/README 零命中)且无生产赋值(恒空);唯一消费者在 os/macos.rs:156-160(Enforce 下 `(deny network*)`,非空时仅落注释)。**原「配置存在但全拒 + 注释在 sandbox/mod.rs」表述漂移**;波 C 按 D3 删除。
- Linux Landlock:白名单式(`crates/exec/src/os/linux.rs` 建 ruleset,读枚举 SYSTEM_READ_PATHS :15 起),嵌套 deny 无法从 allow 根做减法 → Landlock 硬拒绝 fail-closed(:819 回归;bwrap 路径为空 tmpfs 覆盖,语义不同);Landlock 无网络强制(标签 `HardFilesystemOnly`,sandbox.rs:356;bwrap 在 Enforce 下才加 `--unshare-net` :479)。Windows:AppContainer 探测恒不可用,Job Object 实施资源限制,标签 `Degraded` + 诚实 note(sandbox.rs:380-385)。**波 A 复核:两平台标签/note 如实,行为零变更(仅跨平台单测的 dead_code lint 属性行)**。
- PTY:`crates/app/src/gui_host/handlers/terminal.rs`:123-157 `terminal_create` 直连 `PtyService::create`,不过 `PolicyEngine`,响应如实 `uncontrolled:true`;进程组回收 Unix 靠 waiter/cleanup_handles 显式 `terminate()`(非「依赖 drop」),Windows 靠 Job 句柄 drop。MCP stdio 经 `spawn_interactive` 已过软限制,语义不同。
- shell 风险分类:`crates/policy/src/shell.rs` 固定词表 + 空白 tokenize + 嵌套引用兜底正则;引号/变量/管道可漏分类;灾难地板在 NeverAsk/ReadOnly 直 Deny(engine.rs:56-67),**AskForDangerous 误分类即静默放行**。
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
- [ ] PTY 按决议入闸或显式豁免;进程组回收有回归
- [ ] shell 分类按决议落地;绕过种子(引号/管道/变量)测试收紧
- [ ] K-09 字段有终局(实现/删除/标注);全平台探测语义 fail-closed 不变
- [ ] 安全回归全绿;冒烟通过;v3_plan §3 更新
