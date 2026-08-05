# P0-9：Mock Provider / Mock Tool

> Phase 0 · 架构与协议冻结 · 状态：🟢已完成 · 依赖：P0-4、P0-5

**最终目的**：提供可编程的 Mock Provider 与 Mock Tool，使 Agent Loop 在无真实网络下即可跑通，并覆盖多 tool call、partial JSON 等复杂场景。这是 Phase 0 退出标准「Mock 跑通最小链路」的关键。

**涉及范围**：`test-support`

## 细分步骤

1. **脚本化 Mock Provider** —— 按预设脚本产出 text/tool_call/complete。目的：可重现的 provider 行为。
2. **多 tool call 与 partial JSON 流** —— 分片输出 tool arguments。目的：覆盖流式组装难点。
3. **可断言 Mock Tool** —— 记录调用、返回可控结果。目的：测试调度与审批。
4. **断言辅助** —— 验证调用顺序/参数/取消。目的：回归测试基础。

## 主要产出物

- `test-support` crate：Mock Provider/Tool + 断言工具

## 验收标准

- [x] 支持多 tool call
- [x] 支持 partial JSON 流式
- [x] 可断言调用序列与取消

**相关文档**：[测试体系](../docs/quality/testing.md) · [providers](../docs/features/providers.md) · [ROADMAP](../ROADMAP.md)
