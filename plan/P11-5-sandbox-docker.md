# P11-5：Docker / Podman（P1）

> Phase 11 · Sandbox 与跨平台强化 · 状态：⚪已归档/推迟（P1） · 依赖：P11-1

**最终目的**：实现 Docker / Podman 容器沙箱后端，为强隔离场景提供容器级执行。标记为 P1，可在 MVP 后交付。

**涉及范围**：`sandbox-runtime`

## 细分步骤

1. **容器调用封装** —— 目的：容器化执行。
2. **镜像与挂载策略** —— 目的：可控环境。
3. **网络与资源限制** —— 目的：隔离。
4. **可用性检测与回退** —— 目的：无 Docker 时回退。

## 主要产出物

- Docker / Podman backend

## 验收标准

- [ ] 容器内执行隔离生效（或优雅回退）

## 归档记录（2026-08-09）

- Phase 11 选择平台原生后端作为当前交付边界；Docker/Podman 需要外部 daemon、镜像供应链与挂载/网络生命周期，维持 P1 归档，不进入 `SandboxSelector`。
- 路线图计数按约定包含归档任务；本记录不表示容器 backend 已实现。

## 替代路线 / 后续决策

- **Docker/Podman 不进默认 SandboxSelector**：依赖外部 daemon、容器 runtime、OCI 镜像供应链与 mount/network 生命周期，并引入部署依赖（需用户环境预装容器工具链），与 Phase 11 平台原生、daemonless 的交付边界冲突。除非未来出现充分证据，不重新进入默认选择链。
- **强隔离增强由 P11-1/2/3/4 后续增强承担**：guarantee 模型与 policy-aware planning（P11-1.E1/E2）、macOS 真实 L2 验证与 Desktop App Sandbox/XPC 研究（P11-2.E1/E2）、Landlock 能力升级与 bwrap 职责澄清（P11-3.E1/E2/E3）、Windows AppContainer/Job 完成与实验 API probe（P11-4.E1/E2/E3）在现有平台原生后端上补齐强隔离，不依赖容器运行时。
- **OCI/VM 属 Execution Environment**：若未来用于「可重复开发环境」，应视为 Execution Environment（当前进程运行在哪种系统/镜像/VM）而非 Sandbox Runtime（当前进程可访问什么）；本阶段不为此提前创建抽象（见 ADR-031 amendment）。
- **Non-goals 不变**：不实现 Docker daemon、不要求 Podman、不引 OCI image 作 shell 前提、不自实现完整 container runtime。

**相关文档**：[sandbox](../docs/features/sandbox.md) · [ROADMAP](../ROADMAP.md)
