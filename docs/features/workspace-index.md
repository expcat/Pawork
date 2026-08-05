# Workspace 与文件索引

## 职责

管理工作区生命周期与多 Root，并提供文件元数据索引与 `@file` 搜索。

## WorkspaceService

添加工作区；删除工作区；重命名；信任；多 Root；检测 Git；获取分支；获取 HEAD；Worktree；最近访问；工作区设置；Project Instructions；文件监听。

## File Index

索引内容：相对路径；文件类型；大小；修改时间；ignore 状态；语言；Git 状态；文本/二进制；可搜索名称；可选内容索引。

要求：初次异步扫描；增量更新；`.gitignore`；全局 ignore；工作区 ignore；symlink 策略；大型目录排除；`node_modules` 等默认排除；文件事件去抖；索引损坏重建。

## Phase 1 实现状态

`workspace-service` 已实现 Workspace 增删改、默认不信任、信任切换、多 Root 规范化去重、稳定快照与目录 / gitfile 两种 Git 仓库检测。`file-index` 已实现 blocking 池异步扫描、ignore 规则、大目录排除、文本/二进制与语言元数据、模糊搜索，以及通过有界通道合并的增量去抖更新；Windows 事件路径规范化已有回归覆盖。

## `@file` 搜索

文件名模糊搜索；路径搜索；最近访问排序；Git changed 优先；大小限制；文件预览；多文件选择。

## 验收标准

- 大目录扫描不阻塞 UI/Core 初始化
- ignore 规则与 Git 一致
- 文件事件去抖生效

## 相关文档

- [git-diff](git-diff.md) · [skills（resource-loader）](skills.md) · [policy（workspace trust）](policy.md)
- [ROADMAP P1-7 / P1-8](../../ROADMAP.md)
