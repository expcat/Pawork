# P19-6：Composer / Context 输入

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-2～P19-5、P1-8、P3-2、P6-6、P8-5、P13-8

**最终目的**：提供快速且可审查的 Prompt/Context 输入面，把文本、`@file`、图片/Artifact、模型/Profile 与发送/取消都映射到 canonical command，而不让 GUI 直接读取 workspace 或调用 Provider。

**涉及范围**：Composer editor、mention/context picker、attachment upload、model/profile selector、draft/pending state

## 细分步骤

1. **编辑器交互** —— 多行、IME、快捷键、撤销、粘贴、字符/token 估计和发送意图分离。目的：跨平台输入可靠。
2. **`@file`/资源选择** —— 通过 Core query 搜索 file-index/resource diagnostics，插入稳定 workspace-relative reference。目的：不由 GUI 扫描文件系统。
3. **附件** —— 本地/远程统一走 Artifact upload/chunk API，显示类型、大小、上传/取消/失败。目的：不在 command 内联大型二进制。
4. **模型/Profile/effort** —— 消费 capability snapshot，只展示合法组合；选择进入 canonical request。目的：避免 Provider-specific options 泄漏。
5. **发送/排队/取消** —— 使用 idempotency key、pending command 与 run event；重复点击不重复创建 Run。目的：网络重试安全。
6. **Draft 安全** —— 默认仅内存保存；若后续持久化必须显式 opt-in、无 Secret 并有独立 ADR。目的：避免 prompt 泄漏到 Desktop 本地偏好存储。
7. **输入测试** —— CJK/emoji/IME、超长 prompt、附件失败、离线重连、capability 变化。目的：锁定边界行为。

## 主要产出物

- Composer、context picker、attachment queue、model/profile controls
- AppCommand builder 与 pending/cancel interaction
- IME/accessibility/security/retry tests

## 验收标准

- [ ] CJK IME 组合阶段不会误发送，键盘快捷键可配置且可访问
- [ ] `@file` 与附件只经 Core query/Artifact API，GUI 不直接读取任意路径
- [ ] 不合法 model/profile/effort 组合在 command 前被 capability snapshot 阻止，Core 仍最终校验
- [ ] 重试/双击不创建重复 Run，取消状态由 Event 收敛
- [ ] Draft 默认不进入 Desktop 本地偏好存储/log/crash report

**相关文档**：[context](../docs/features/context.md) · [models](../docs/features/models.md) · [artifacts](../docs/features/artifacts.md) · [Desktop GUI](../docs/features/desktop-gui.md)
