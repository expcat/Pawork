# ENV-05 · Chrome / Edge 只读上下文环境复验

> 状态：当前核查完成；初审 P3 表述修正后独立复审 PASS；真实正文成功、长文本截断与 Desktop 消费侧待环境。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#缺环境与外部条件)

## 目标

用户手动授权后复验当前页正文读取。

## 当前依据与读取范围

本轮重核当前安装、运行条件和明确选中的本项测试页，经正式只读入口记录成功或拒绝；历史错误 12 仅作定位线索，不代替本次结果。

[browser Spec](../spec/crates/browser.md)、[Desktop Spec](../spec/desktop.md)。

## 验收范围

- 历史记录为浏览器错误 12；本轮重新记录实际结果，不沿用旧错误推断当前开关或授权。系统 Automation 权限与 Apple Events JavaScript 开关仍由用户手动处理。
- 复验 URL、标题和有界正文读取；本项是只读上下文，不是 [CU-15 外部浏览器控制](cu-15-external-browser.md)。

## 完成条件

- 真实 Chrome / Edge 正文读取成功，来源和不可信快照说明正确。
- JavaScriptDisabled / AutomationDenied 等真实拒绝仍准确显示，不自动代用户批准系统权限。

## 完成记录

2026-10-12，当前 macOS 已安装 Chrome `154.0.8037.98` 与 Edge `155.0.4283.45`。打开测试页前，窗口记录中没有 Chrome 可见窗口；本项随后打开测试页。Edge 原先运行，保留其已有窗口。测试页为 `http://127.0.0.1:63585/short`，标题 `ENV05-PAWORK-SHORT`，正文标记 `ENV05-SHORT-BODY-MARKER`；临时守卫在调用前记录匹配本项标题的最前窗口，结果若不属于测试页则不保存内容。

临时 Rust example 直接调用正式 `pawork_browser::capture_external_page`，没有复制采集实现或借用 CU-15 扩展桥。`cargo build -p pawork-browser --example env05_capture --offline` exit 0（43.18 秒）；随后的四次正式采集均失败：

| 原始证据标签 | 浏览器 | Driver exit | 实际结果 |
| --- | --- | --- | --- |
| `chrome-capture-short` | Chrome | 1 | Other / Apple Event -1712 |
| `chrome-capture-short-retry` | Chrome | 1 | Other / Apple Event -1712 |
| `chrome-capture-prompt` | Chrome | 1 | Other / Apple Event -1712 |
| `edge-capture-short` | Edge | 1 | Other / Apple Event -1712 |

第三次 Chrome 调用期间另有 `chrome-prompt-watch` 窗口记录，它不是第五次采集，也不能把第三次降为纯观察。相同失败的重复调用已停止，原始日志全部保留。`-1712` 仅表明 Apple Event 超时，原因未辨明；不是 JavaScriptDisabled（12）或 AutomationDenied（-1743）。UserNotificationCenter 的窗口计数和空标题不能证明本次授权弹框。没有取得 URL、标题、正文、truncated 或不可信快照说明；短页未成功，未继续长文本页。

发起进程是临时 CLI example `target/debug/examples/env05_capture`，签名回执为 adhoc / linker-signed，标识 `env05_capture-0eec5ca7ac474125`。本次未经 Pawork.app 的 Desktop 附件菜单发起，也未测定系统 TCC responsible application，不能用二进制路径或签名推断授权主体。执行者辅助 `env05-facts.json` 对 unsigned / TCC 主体的表述由 `coordinator-corrections.json` 校正，原摘要与失败回执保留。Desktop 的结构化错误显示仅复核源码，未执行真窗口或新的模拟 UI 验收，不能把 driver 超时当成 GUI 消费侧通过。

证据位于 `/tmp/pawork-env05-browser-context/evidence`，各标签有原 command / stdout / exit / summary；`sources/` 归档 driver、guard、测试页服务与清理源码。脚本初次写入失败仅有执行者说明，原错误回执未从 read_thread 恢复，不补造历史输出。已恢复的辅助失败及服务终止 exit 143 保留在 `coordinator-worker-failure-receipts.json`，不登记为检查通过。

清理分开记结果：执行者的 `edge-cleanup` 顶层 runner exit 0，但内部 osascript exit 124；随后 Cmd-W 与关闭按钮点击也未关闭测试窗，均不是成功。执行者退出后，指挥通过原生 UI 工具核对该窗 URL、标题和正文标记，点击其关闭按钮；新 AX 状态不含本项标题，窗口 inventory 中本项窗口为空，见 `coordinator-native-ui-cleanup.json`。浏览器 tab 绑定不可用与 bundle ID 歧义两个辅助失败也在该记录中。停止测试服务不会清空已加载正文，关闭前 AX 标记只是界面观测，不替代产品采集成功。与测试窗口对应的 Chrome 主进程 14199 已退出；未终止用户 Edge 或关闭其其它窗口。

初审发现临时脚本按首个空格截取可执行路径，无法匹配含空格的 Chrome 路径；在观察到测试窗 PID 14199 时，after_main_pids 仍为空。因此辅助摘要对「Chrome 原先未运行／本项新启进程」的推断不成立，文档只保留可见窗口与对应进程事实。原日志和 `env05-facts.json` 未改，Grok 原对话复核与 `worker-review-correction.json` 记录校正依据，无新的环境调用或编译。

临时 example 与测试服务已回收，仓库无生产源码差异。指挥实时核对 auth.json、config.toml、desktop.json 的摘要与已完成 ENV-04、亦即 ENV-05 开始前快照一致，mcp-auth.json 仍不存在；使用的是实际系统 Application Support 配置路径，见 `coordinator-current-hashes.json`。本项当前环境核查结束，因正文成功条件未满足，保持待环境状态。

执行对话 `01a12fda-2d6b-7ea3-b01f-5191d4d538a9`（Grok 4.7 xhigh）；唯一独立审查对话 `01a12ff7-3150-7dc1-8e93-830184ca0d88`（GPT-6.1 Sol max）在 turn `01a13000-3485-7d20-a93d-14e9db208d32` 复审 PASS，阻断与非阻断问题均无。R2 受审差异 8393 字节，SHA-256 `6b538f601a0b8ef2cb538a610602c3fc320fe6ba64ca97c9683a8a2a2653d1e8`；指挥核当前差异一致后仅同步审查元数据。原始 ISSUES 与 PASS 分别保存为 `REVIEW-ISSUES-1.json`、`REVIEW-PASS.json`。

```text
Validated: cargo build -p pawork-browser --example env05_capture --offline（exit 0）；4 次正式 capture（均 exit 1 / Other / -1712）；coordinator-current-state（exit 0）；原生 UI 测试窗关闭核验；doc-links / git diff --check
Targeted regressions: none（无生产源码改动；库调用超时未验证成功快照）
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

