# GUI2-04 · 当前对话查找与回合定位人工验收

> 状态：✅ 完成（模拟验收）；独立 review 初审 3 项 P2 修复后复审 PASS。真实 Host、真窗口、原生 AX bridge、系统 Cmd+F 真实像素与真实长历史分页待验。更新：2026-10-11。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收已加载正文查找、长对话回合目录与阅读位置保持。

## 当前依据与读取范围

迁移前 ROADMAP 已登记本项实现与历史自动检查；代理真窗口观察和用户验收分开跟踪，本次仅拆分剩余验收任务，没有重新执行产品验证。实施验收前以现行合同与真实窗口复核。

[GUI 设计](../gui-design.md)、[Desktop 包级 Spec](../spec/crates/desktop.md)、[Desktop 产品 Spec](../spec/desktop.md)、[视觉基准](../../design/README.md)。

## 验收范围

- 用 Cmd+F / Header 入口查找当前已加载用户和助手正文，核对命中消息计数、上下项循环、无结果与历史加载中范围说明。
- 验收长对话回合目录的滚动、选择与定位；流式转持久消息后定位身份仍正确。
- 核对定位后的阅读模式、手动滚动、显式回底恢复跟随，以及切任务清空命中和关闭回焦。

## 完成条件

- 用户确认命中与目标消息一致，长对话定位未跳到其它回合。
- 加载范围、阅读模式和焦点有真实交互结果，不以 AX 节点存在替代可用性。

## 完成记录

2026-10-11 在真实 AppView 投影、GPUI `TestAppContext`、键盘 / 滚轮 / AX 分发路径中模拟验收，基线 `11a2c59c0c99d08c774c34f90d6da4979c2e8288`。产品逻辑无改，仅扩展既有 `conversation_find_identity_reading_and_turns` 并同步 desktop Spec。

- **模拟验收通过**：Header「查找」打开后关闭回焦到该按钮；Composer 聚焦时 `Cmd+F` 关闭回 Composer。已加载用户与助手正文参与查找并忽略英文大小写，思考正文不入结果；命中计数为 2 / 12 与 1 / 12，上一条绕到 12 / 12、下一条回到 1 / 12，无结果为 0 / 0 且上下按钮禁用、AX 不发布 Press。
- **回合定位**：12 条回合超出目录视口，聚焦末条后目录滚动且首条离开 AX 裁剪范围；对已聚焦可见末项模拟 Enter 后目录关闭并定位第 11 行，前插思考后同一持久身份移至第 12 行且定位保持。
- **身份与阅读模式**：流式 delta 提交并刷新后唯一命中为完整助手正文，选中项、锚点与滚动位置一致；定位进入阅读模式，滚轮手动滚动解除锚点且不恢复跟随，显式回底才恢复跟随。
- **切任务与草稿**：保持有效命中和锚点经真实会话点击路径切到另一任务，导航查询 / 命中 / 锚点清空，原任务草稿保存，切回后恢复；关闭查找回焦不写 Composer 草稿。
- **自动门禁通过**：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders conversation_find_identity_reading_and_turns -- --test-threads=1` exit 0，1 passed / 289 filtered（测试 7.76s）；定向 rustfmt 与 `git diff --check` exit 0。
- **Review 记录**：Sol max 初审发现切任务绕过产品路径、提交后唯一命中断言缺失、目录滚动与实际选择未连通三项 P2，以及一个 unused `window` P3；Grok xhigh 修复后同 reviewer 复审 PASS，阻断与非阻断均无。
- **待验边界**：真实 Host、真窗口、原生 AX bridge、系统 Cmd+F 真实像素与真实长历史分页未验；本记录不登记为真人窗口验收。

证据：`/tmp/pawork-gui2-04-conversation-navigation/evidence/`（命令、输出、diff 与 SHA256）。独立 review（Sol max，thread `01a1280a-8952-7d12-9725-a680ddcea5ed`）turn `01a1281a-7b26-75f3-8f38-d98fec0b3c19` PASS。本地提交由指挥用 `/tmp/pawork-final-commit.py` 精确执行；提交哈希记录在证据 `COMMIT.json`，不推送。
