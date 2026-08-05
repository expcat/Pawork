# P11-5：Docker / Podman（P1）

> Phase 11 · Sandbox 与跨平台强化 · 状态：⚪P1（可推迟）· 依赖：P11-1

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

**相关文档**：[sandbox](../docs/features/sandbox.md) · [ROADMAP](../ROADMAP.md)
