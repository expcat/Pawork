# ADR-027：本地与远程 GUI 使用同一 GUI Protocol

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

若本地 GUI 与远程 GUI 走不同协议，会增加两套实现与测试成本，并在本地/远程混连时产生行为分叉。

## 决策

本地 GUI 与远程 GUI 使用同一 GUI Connection Protocol；差异只体现在 Transport 层（Local：Unix Socket / Named Pipe；Remote：可替换 Adapter）。CLI/Core 与 GUI Protocol 不感知远程传输的内部实现。

## 后果

- 协议只需冻结与测试一套；Transport 可独立替换与演进。
- 远程 Workspace、Secret、命令与文件均保留在 CLI/Core 所在设备。

## 相关

- [gui-connection](../features/gui-connection.md) · [ADR-022 GUI 经 CLI 连接](ADR-022-gui-connects-via-cli.md) · [ADR-028 远程可替换](ADR-028-replaceable-remote-transport.md)
