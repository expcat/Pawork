# ADR-050：Settings 终端页 wire 与 config（TerminalSettings / SetTerminalSettings，API 1.8）

- **状态**：Accepted（用户 2026-09-03 确认，D1–D5 按拟议执行）
- **日期**：2026-09-03

## 背景

SET-6 逐页立项的第四页是「终端」。任务书（plan/settings.md SET-6 表）锁定的最小真实能力是「有明确宿主持久化语义的 shell/cwd/尺寸默认值」，明确不做 Desktop 直写 PTY 配置。2026-09-03 经主代理源码实读与两路 glm_explorer 独立只读核查三方确认的基线事实：

- **config**：`PaworkConfig` 顶层无 shell/terminal/cwd/尺寸键；未知键经 flatten 落入 `extra` 按键递归合并，全仓无代码读 `extra["terminal"]`（crates/workspace/src/config/schema.rs:16）。Global writer 已有三个同构先例（RMW + `CONFIG_WRITE_LOCK` 进程锁 + 原子写，crates/workspace/src/config/writer.rs:21-42）。Workspace 层 `<root>/.pawork/config.toml` 的未知键同样透传进 `extra`，但 workspace 包没有 Workspace 层写盘代码。
- **安全边界**：`strip_untrusted_layer`（crates/workspace/src/config/loader.rs:374）对非 Builtin/Global 层剥离 `trust_workspaces` / `proxy_url` / `providers[].base_url` / `mcp.servers.*.trusted|auto_start`。若 Workspace 层可设默认 shell，克隆恶意仓库后打开终端即执行仓库指定程序——等同任意命令执行，必须阻断。
- **wire**：`TerminalCreate` 仅 `workspace_id` + `working_directory`（Option），无 shell/size 字段（crates/protocol/src/app/command.rs:431）。Settings 类词汇惯例：snake_case wire 名、仅 GUI available、headless=None/acp=false、写命令 idempotent、注释标注 ADR 编号（general_settings V1_5 / permissions_settings V1_6 / mcp_test V1_7）。
- **运行时**：`terminal_create` handler 构造 `PtyCreateSpec` 只设 `owner_session`/`cwd`，shell=None 走 exec 兜底链（unix `$SHELL`→`/bin/sh`，Windows `cmd.exe`），size 恒为 `PtyWindowSize::default()` = 80×24 pixel 0（crates/app/src/gui_host/handlers/terminal.rs:216-234；crates/exec/src/pty/mod.rs:150-159、1024-1055）。策略闸 `classification_shell(spec.shell)` 已消费 spec.shell，shell 换源后 gate 自动跟随，gate 本身不改。
- **Desktop 冲突点**：新建终端在 `TerminalCreated` 回执后立即按自身 projection 默认 80×24 下发一次 `terminal_resize`（apps/desktop/src/ui/mod.rs:1272-1286）——若宿主按配置默认尺寸创建，这次 resize 会压掉配置默认，必须同批改为使用终端页生效值。resize 全链路只作用会话，无任何持久化。

## 拟议决策

### D1 — config 新增 `[terminal]` 段，Global 层唯一写入点，非 Global 层整段剥离

- `PaworkConfig` 新增声明字段 `terminal: Option<TerminalConfig>`（`shell: Option<String>`、`columns: Option<u16>`、`rows: Option<u16>`，均 skip_serializing_if None）。
- 写入只发生在 Global 层：writer 新增 `write_terminal_settings`（同 write_proxy_url 一族：`CONFIG_WRITE_LOCK` + RMW + 未知字段保留 + 原子写回）。
- `strip_untrusted_layer` 追加剥离顶层 `terminal` 键（非 Builtin/Global 层出现即剥 + ConfigWarning 如实告警）：防仓库投毒指定 shell，语义与 trusted/auto_start 先例一致。

### D2 — 新查询 TerminalSettings

- `AppQuery::TerminalSettings`，响应 `{ shell: Option<String>, columns: u16, rows: u16 }`：shell 为 Global 持久值，null = 跟随平台默认（exec 兜底链）；columns/rows 为生效值（未设置 = 80/24，与 exec/Desktop 既有默认一致）。
- registry 登记 since = V1_8、仅 GUI available，headless/ACP 不开放（沿用 ADR-046 D5 通道保守策略）。

### D3 — 新命令 SetTerminalSettings（全态写）

- `AppCommand::SetTerminalSettings { shell: Option<String>, columns: u16, rows: u16 }`：三字段必填（deserialize_with 取消 Option 隐式默认，缺字段解码错误，同 ADR-047 SetProxyUrl 先例），`shell: null` 显式清除回平台默认。
- 全态写而非部分更新：Desktop 总是先查后改、回传完整目标状态，消除 missing/null 二义。idempotent: true。
- Host 校验（fail-closed，非法即 Error 保旧，三处皆不动）：shell trim 后非空；含 `/` 时路径必须存在，不含 `/` 时须在 PATH 可解析；columns/rows ∈ 2..=1000（防 0 尺寸崩 PTY、防离谱值，范围之外用 resize 单会话调）。
- 定序（同 ADR-047 D2 先例）：校验 → Global 原子写 → 写锁内内存配置同步；写盘成功即权威，同会话重查一致、重启恢复。
- 响应 Data 回执写入后的完整 `TerminalSettings` 形状（回执无 Secret，进 ledger 响应缓存可接受）。

### D4 — terminal_create 应用配置默认；Desktop 初始尺寸取生效值

- `terminal_create` 在构造 `PtyCreateSpec` 处读取生效配置：`spec.shell = 配置 shell`（None 维持 exec 兜底），`spec.size = { 配置 columns/rows, pixel 0 }`。策略闸 `classification_shell(spec.shell)` 自动跟随，gate 语义不变。
- Desktop 新建终端的初始 projection 尺寸与创建后那次 `terminal_resize` 改用 `terminal_settings` 查询的生效 columns/rows，不再硬编码 80×24；未查询到时回落 80×24（现状行为）。
- 生效边界：只影响之后创建的终端；已存在终端不回溯调整（快照语义，页面文案如实标注）。

### D5 — 版本策略与 golden 先行

- 沿用 ADR-046 D5 用户已拍板口径：初始未发布版本不采取兼容策略。API minor 升 V1_8（SUPPORTED_API_VERSIONS 追加 1.8 仅作记账；不新增 GuiCapability 变体）。
- golden 先于 handler 检入：client 侧 `terminal_settings` 查询帧与 `set_terminal_settings` 命令帧（含 shell=null 清除帧）各一；server 侧响应/回执帧各一。typegen 重新生成三产物并过 --check。
- 定向回归上限：主路径两条（set → 重查一致；terminal_create 应用配置 shell/size）；关键失败路径一条（非法 shell/越界尺寸 fail-closed 保旧）；安全红线定向回归一条（非 Global 层 `[terminal]` 被剥离并告警，属三类不推迟测试）。

## 否决支

- **cwd 默认值**：有真实语义的形态是 per-workspace 默认 cwd（Workspace 层 `.pawork/config.toml`），但 workspace 包无 Workspace 层写盘代码，需新增 writer + 层语义，超出本片最小必要；登记为后续候选。
- **Workspace 层承载 `[terminal]`**：shell 可被仓库投毒利用，安全红线不允许；columns/rows 虽无害，为语义一致整段剥离。
- **部分字段 patch 更新**：missing/null 二义违背 ADR-047 已拍板先例，全态写更简单且足够。
- **resize 持久化 / 像素尺寸 / shell args / env 配置 / Desktop 直写 PTY**：均非本片范围；resize 保持会话级语义。
