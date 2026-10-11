# AC-06 · 模型用途筛选人工验收

> 状态：✅ 完成（模拟验收）；自动门禁与独立 review PASS；真实系统边界待验。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

验收 Composer 用途菜单和 Settings 能力徽标。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[模型网关](../spec/model-gateway.md)、[Settings Spec](../spec/settings.md)、[GUI 设计](../gui-design.md)。

## 验收范围

- 按 ADR-064 现行合同核对用途筛选、名称搜索叠加及实际候选目录。
- 检查 Settings 识图/生图/视频/搜索徽标与模型能力，避免筛选隐式切换模型。

## 完成条件

- 筛选只改变候选列表，模型选择行为符合现行合同。
- 徽标和目录事实一致，鼠标/键盘模拟验收有记录；真实系统窗口与目录来源独立待验。

## 完成记录

2026-10-12，按用户授权完成 GPUI 测试窗口模拟。执行对话 `01a12c46-cb97-7930-a545-7fda551ccd53` 使用 `xai/grok-4.7 xhigh`；首轮因 HTTP 429 中断，退避后在原对话完成，没有换模型。生产筛选与徽标逻辑未改，仅新增 `ui/accessibility/app.rs::model_purpose_menu_input_keeps_model_and_renders_settings_badges`，并同步 [Desktop Spec](../spec/crates/desktop.md)。

### 模拟输入与实际结果

- Host 边界 fixture 含三个已连接文本模型、一个生图专用模型和一个未连接供应商模型。鼠标打开 Composer 菜单：对话用途显示 `plain` / `eyes` / `clip`，排除 `text=false` 的 `paint` 与未连接的 `ghost`。
- 空搜索、未移动高亮时 Enter 保持菜单和本地选择。鼠标切到识图用途只剩 `eyes`，焦点回搜索；当前模型被用途排除后再按 Enter 仍保持本地选择。Tab 到视频 chip 后 Enter 激活，候选只剩 `clip`；键盘输入 `clip` 命中，清除按钮后输入 `eyes` 在视频用途下显示搜索无结果。各步 `pending_model` / effective 模型均为 `acme/plain`，草稿保持。
- Settings 路由由测试预置；供应商展开及 Manage models 通过鼠标走正式 handler。滚到实际可见位置后读取 AX 描述：`acme/paint · 生图`、`acme/eyes · 识图 · 搜索`、`acme/clip · 视频`、`acme/plain`，节点高度均大于 1。生图专用模型没有推理默认按钮，文本模型保留该按钮。
- 四用途统一拒绝非文本模型以现行 `ModelPurposeFilter::matches` 和既有纯函数回归为依据；本轮新增输入覆盖对话、识图和视频用途，没有宣称逐一激活搜索用途。语言仅测试线程中文状态，退出恢复，不写用户偏好。

### 验证与失败记录

```bash
cargo test -p pawork-desktop --offline --bin pawork-desktop --features gpui/runtime_shaders model_purpose_menu_input_keeps_model_and_renders_settings_badges -- --test-threads=1
rustfmt --check --edition 2021 apps/desktop/src/ui/accessibility/app.rs
git diff --check
python3 /tmp/pawork-roadmap-links.py
```

修后 Cargo exit 0：1 passed、0 failed、301 filtered，测试 8.18s，编译 20.53s。日志 `/tmp/pawork-ac06-model-purpose/evidence/gpui-model-purpose-menu-2.log` 与 `.exit`；格式、diff 和链接检查均 exit 0。保留既有编译警告，未跑全 Desktop 或全 workspace。

首轮 `gpui-model-purpose-menu.log/.exit` 为 exit 101：520px 高弹层裁剪离屏开关，直接读取 `paint` 节点失败。最小修正仅测试读取：先滚顶/底，必要时按产品布局框把中间行对齐到列表顶部，再读取可见节点。辅助查询的 exit 1、停止宽范围查找的 exit 143 按真实工具记录保存于 `AUXILIARY-FAILURES.json`；原模型 429 保存于 `EXECUTION-FAILED-429-1.json`。这些均不算通过门禁。

### 证据边界与审查

没有 GUI client；`selected_model` 始终为空，本地 pending 保持不证明 Host 切模或 `model_list` 请求成功。Settings 页面跳转不是本用例验收；弹层滚入为程序操作产品 scroll handle，不证明真实滚轮输入。AX 是 GPUI 测试平台发布结果，不证明原生 AX bridge、VoiceOver、系统窗口、像素、真实 Provider 或权威目录。真实系统边界独立待验，符合本轮人工验收用模拟的授权。

独立 `gpt-6.1-sol max` review 对话 `01a12c64-22ad-7d23-abb3-3aef875c4512`，turn `01a12c64-272e-7ab3-8a9a-b507aa3f99db` 为 completed / error=null，结论 PASS，阻断与非阻断问题均无。受审快照 `PENDING.diff` 为 27,725 bytes、SHA256 `34af90fd6af651826582d03a28b0490836aee57ef723fe245071bf97cd04df0c`；指挥核对差异和四文件 hash 一致后，只同步本文件与 ROADMAP 的审查状态。源码与 Spec 不变，由指挥精确本地提交，不推送或发布。

```text
Validated: 上述定向 Cargo（exit 0，1 passed）、rustfmt、diff 与链接检查（exit 0）
Targeted regressions: 用途与搜索叠加、未移动高亮 Enter、pending/草稿保持、非文本与未连接目录排除、Settings 四能力徽标
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

