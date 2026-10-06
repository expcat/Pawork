# B19 · Desktop 终端响应与实例路径裁决

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

核实旧终端字段回退和零消费路径函数的必要性。

## 当前线索与读取范围

登记项为终端 create 响应仍将 `id` 回退为 `terminal_session_id`，以及 `default_socket_path` / `default_token_path` / `token_path_for_instance` 无生产调用。

[desktop crate Spec](../spec/crates/desktop.md)、[client Spec](../spec/crates/client.md)、[protocol Spec](../spec/crates/protocol.md)。

## 实施范围

- 核对协商版本、正式 Host 响应和实例启动路径，再裁决回退及函数。
- 避免用历史字段继续兼容已不存在的链路，也不能破坏仍受支持的版本。

## 完成条件

- 终端身份和实例 socket/token 归属正确，旧版本边界明确。
- 相关协议和终端生命周期回归通过，实际公开面变化同步 Spec。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

