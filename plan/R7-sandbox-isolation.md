# R7 — 执行面真隔离(T7,ADR-041)

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R7 行。根因:S4 沙箱以「诚实标签」交付(能力有限但如实上报),V2 期间未再加深:macOS Seatbelt 用「整盘只读 + 枚举 deny」的粗粒度 profile,`network_allow_hosts` 配置存在但全拒(K-09);PTY 会话不经沙箱与审批闸(gui_host 直连 `pawork-exec` PTY);shell 风险分类靠固定词表字符串匹配。本阶段把执行面从「标签诚实」升级为「语义诚实」。安全域改动,全程定向回归护航,fail-closed 语义只紧不松。

## 1. 现状证据(执行时重验;路径为 R1 合并后位置)

- `execution/exec/src/sandbox/macos.rs`:Seatbelt profile 生成——default allow + `(deny file-write*)` 枚举白名单外路径;`network_allow_hosts` 解析后未落到 profile(全拒:`sandbox/mod.rs` K-09 注释)。
- Linux Landlock:白名单式(较好),但与 macOS 语义不对齐;Windows:Job Object 资源限制,文件系统无隔离(诚实标签 `filesystem: none`)。
- PTY:`gui_host` PTY 会话直接 spawn,不过 `PolicyEngine` 审批/风险分类;进程组回收依赖 drop(F17 修过 kill 竞态)。
- shell 风险分类:`execution/policy/src/shell.rs` 固定词表(`rm -rf`、`git push` 等)+ 子串匹配;引号/变量展开/管道可绕过分类(fail-closed 兜底是审批,但分类精度影响 UX 与审批疲劳)。

## 2. ADR-041 决策点(波 0;须用户确认)

1. **macOS profile 白名单化**:deny-default + 显式 allow(workspace 写、临时目录、必要系统读)——兼容性代价(Homebrew/工具链路径、Darwin 25 行为)以实测数据进 ADR;不可行处保留枚举 deny 并如实标注能力等级。
2. **PTY 信任模型**:PTY 会话入 policy 闸(spawn 前风险分类 + 审批档位适用)还是维持「PTY = 用户亲手终端,自担风险」显式豁免?推荐前者(GUI 发起的 PTY 与 agent 工具无本质区别),豁免须用户点头。
3. **K-09 egress**:三选一——(a) 实现按 host 白名单的 egress broker(代理进程/DNS 解析前置);(b) 删除 `network_allow_hosts` 配置项(诚实:只有 allow-all/deny-all 两档);(c) 维持全拒 + 文档标注。推荐 (b)(单机产品下 host 级白名单收益低、实现重),留 (a) 为候选。
4. **shell 分类**:结构化解析(tree-sitter-bash 或手写 tokenizer)替换词表,还是保留词表 + 提高审批档位兜底?推荐手写轻量 tokenizer(管道/重定向/引号感知,不引入大依赖),分类只影响「是否升档审批」,兜底语义不变。

## 3. 波次拆分

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| 0 | ADR-041(含 macOS 白名单 profile 原型的本机实测数据)→ 用户确认 | docs/adr/ | 串行 |
| A | macOS profile 重设计 + Linux Landlock 语义对齐 + Windows 标签复核(平台探测 & fallback 语义不变:探测失败 fail-closed) | exec(sandbox/) | 串行(安全内核单一 owner) |
| B | PTY 入闸(按 ADR 决议)∥ shell 风险分类结构化 | app(gui_host PTY 装配)、exec(pty)∥ policy(shell) | 并行 ×2 |
| C | K-09 落地(按 ADR 决议);三平台定向回归 + 沙箱逃逸种子(路径越界/symlink/`.git` 写/网络外呼)全量复跑 | exec、workspace(config 项增删)、docs | 串行 |

## 4. 验证

- 安全红线定向回归全量:S3/S4 既有种子(workspace 越界、symlink 逃逸、`.git` 保护、审批 deny 不执行、探测失败 fail-closed)+ 本阶段新增(白名单 profile 下工具链可用性、PTY 闸审批、egress 决议行为)。
- 本机(macOS Darwin 25)真实验证:沙箱内 `run_command` 读写/网络行为矩阵;Linux 走 CI 或容器定向;Windows 仅编译 + 单测(F03 环境限制沿用)。
- 真实冒烟(矩阵一组):chat 内工具调用(sandboxed)+ Desktop PTY 面板一次会话。

## 5. 退出标准

- [ ] ADR-041 Accepted;macOS profile 按决议落地且能力标签与实际一致
- [ ] PTY 按决议入闸或显式豁免;进程组回收有回归
- [ ] shell 分类按决议落地;绕过种子(引号/管道/变量)测试收紧
- [ ] K-09 配置项有终局(实现/删除/标注);全平台探测语义 fail-closed 不变
- [ ] 安全回归全绿;冒烟通过;v3_plan §3 更新
