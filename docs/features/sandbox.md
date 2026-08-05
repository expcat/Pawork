# Sandbox Runtime

## 职责

为工具与子进程提供受控执行环境，按 capability 约束文件、网络、进程与 Secret 访问。

## Sandbox Backend Trait

```rust
#[async_trait]
pub trait SandboxBackend {
    async fn spawn(
        &self,
        spec: SandboxProcessSpec,
    ) -> Result<SandboxProcess, SandboxError>;
}
```

## 后端优先级

- **P0**：`NativeRestricted`；Workspace 路径限制；Environment 清洗；Process Resource Limit；网络策略提示；命令审批。
- **P1**：macOS Sandbox Profile；Linux Bubblewrap；Windows AppContainer 或 Job Object 限制；Docker；Podman。
- **P2**：轻量虚拟机；远程 Sandbox；用户定义 Sandbox Provider。

## Sandbox Policy

可配置：

```text
filesystem.read
filesystem.write
process.spawn
network.connect
environment.read
secret.read
git.write
clipboard
browser
```

## 验收标准

- 未信任工作区默认受限
- Sandbox capability 测试通过
- 取消能清理进程树

## 相关文档

- [process](process.md) · [policy](policy.md) · [plugins（capability）](plugins.md)
- [ROADMAP Phase 11](../../ROADMAP.md)
