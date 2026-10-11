# GUI3-03 · 代码块头、链接与表格人工验收

> 状态：✅ 完成（模拟验收）；独立 review PASS；真实平台边界待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收 Markdown 表格、代码复制和链接操作。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 用真实 Markdown 消息验收表头、按内容测量的列宽、长表横滚与窄窗显示。
- 核对代码语言标签 / 回退标题、悬停复制及键盘 / AX 入口；复制结果保留原始换行。
- 验收 HTTP(S) 行内链接与消息菜单打开 / 复制，未知协议保持现有边界；不把未实现的语法高亮计为能力。

## 完成条件

- 用户能阅读长表、复制代码和操作链接，结果与原消息一致。
- 真实窗口与复制文本 / 目标地址支持结论，字号变化无阻断。

## 完成记录

2026-10-11：按用户「人工验收使用模拟」授权执行，基线 `db3a4f98b5242f6710979f5a2359f50a9b7e7a76`。执行对话 `01a128f2-080d-7e33-bc16-14d4627303b4` 使用 `xai/grok-4.7 xhigh`。新增一条必要 GPUI 回归与仅测试使用的语言标签 selector，同批更新 Desktop 包级 Spec；生产 Markdown 行为保持现行范围，没有语法高亮。

| 模拟输入 | 正式产品路径与观察 |
| --- | --- |
| 实际助手 Markdown 正文含 rust 与无语言代码块 | 渲染后的标签分别为 `rust` 和 `Code`；反向标签不存在。 |
| 鼠标点 HTTP(S) 行内链接与消息菜单打开行 | `InteractiveText` 和菜单 handler 调到测试平台 `open_url`，`opened_url` 与原地址一致。 |
| 菜单复制前写入哨兵，再点复制链接行 | 测试平台剪贴板被原地址替换，复制不改变已打开地址。 |
| 正文含 `javascript:` 与 `file:`，点击对应正文 | 未生成打开动作，`opened_url` 保留前一个 HTTP(S) 地址。 |
| 长表列宽 / 表头 / 横滚、代码原始 CRLF、宽窄窗口与三档字号 | 复用 [GUI2-02](gui2-02-conversation-composer.md) 同版已审 `conversation_blocks_scroll_and_menus_keep_run` 证据；本项未重跑，不计入本次测试数。 |

实际命令：

```text
cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders markdown_code_labels_and_http_links_dispatch -- --test-threads=1
```

修后 `attempt-2` 为 1 passed / 0 failed / 294 filtered out，测试 6.88s；日志末行与当次工具会话退出码均为 0。`attempt-1` 因新增测试的 AX 树临时借用报 E0716，已修复；该次包装脚本误用 zsh 只读变量 `status`，没有捕获 Cargo 精确退出码，只登记编译失败，不补造 101、不使用工具层 0 宣称通过。原始失败日志保留。

证据：`/tmp/pawork-gui3-03-markdown-content/evidence/` 的 `attempt-1.log`、`attempt-2.log`、`closeout.json`。首次执行 turn 提前结束且没有测试的事实单列在 `EXECUTION-INCOMPLETE.json`；格式初检退出 1 的原输出保存在 `commander-rustfmt-1.log/.exit`。两文件定向 `rustfmt --check --edition 2021` 修后退出 0（`rustfmt-2.log/.exit`）；最后整理仅等价变量遮蔽与格式，不重跑语义用例。相对链接检查 122 项、missing=0；独立审查结果见下段。

独立 review（2026-10-11）：`gpt-6.1-sol max` 对话 `01a1291a-d4b1-7772-9ccf-939dc3e4bf41`，turn `01a1291a-d81e-7b72-a06b-88256287b229` completed、error=null，最终 PASS；阻断 / 非阻断问题均无。受审快照 27431 字节，SHA256 `64d4d94301c898ff8579306c2861e5b19772b37e385e140eda4598f6839a05d2`；reviewer 核对原日志与当次工具退出码、正式 handler 和复用证据。PASS 后仅同步本计划与 ROADMAP 的完成 / 审查状态，源码与 Spec 不变。审查结果和精确提交记录分别保存在 `REVIEW-PASS.json` 与 `COMMIT.json`。

本项只证明正式 GPUI 测试平台的渲染布局、handler、URL 与剪贴板效果；真实窗口像素、原生 AX、系统浏览器 / 剪贴板、Host / Provider 与真人确认仍待验。

```text
Validated: 上述定向 Cargo（修后 exit 0，1 passed）；两文件定向 rustfmt（修后 exit 0）；git diff --check；相对链接检查（122，missing=0）
Targeted regressions: 语言标签 / Code 回退、HTTP(S) 行内和菜单打开 / 复制、未知协议不分发；长表 / CRLF 复用同版已审证据
Full workspace gate: NOT RUN
```
