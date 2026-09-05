# OPT-D 统一设计稿提示词与覆盖状态

## 生成方式、参考与尺寸

- 使用内置 `image_gen`，未使用 CLI/API fallback。
- 共同视觉参考：`desktop-ui-p0-foundation-v4.png`、`desktop-ui-p1-run-review-v4.png`、`desktop-ui-p2-settings-v4.png`；局部修订另附对应上一版 OPT 图。
- 统一 token：dark native desktop、8px 节奏、细分隔线、克制圆角、蓝色 accent；TaskRail/Settings Rail 的设计 token 固定为 **288px**。
- 三张修订稿的 ImageGen 原始输出实际为 **1487×1058**。交付时先等比缩至宽 1440px，再居中裁到 1440×1024；没有非等比拉伸。六张最终文件实际尺寸均为 **1440×1024**。

## 覆盖

| 资产 | 覆盖状态 |
| --- | --- |
| `opt-workbench-inspector-collapsed-v1.png` | F1/F6/F7/F9：Inspector 收起、无项目 session、Composer 无项目、文件工具不可用、Local + gear。 |
| `opt-workbench-inspector-open-v1.png` | Inspector 打开：Changes/Terminal/Resources 与明显折叠控件。 |
| `opt-composer-enabled-model-menu-v1.png` | Composer 仅显示已启用模型并按供应商分组。 |
| `opt-settings-shell-default-roles-v1.png` | F2/F4/F8：全宽 Settings、稳定选中态、四个默认角色。 |
| `opt-settings-providers-expanded-v1.png` | F3/F8/F10/F11：展开凭证、Proxy Switch、默认角色、模型启用弹层与无数据 usage 槽。 |
| `opt-model-enablement-states-v1.png` | 已连接空目录、未连接、Z.AI 部分启用、全关后 Composer 空态。 |

## 本轮修订的实际完整最终提示

### 收起工作台

```text
Targeted correction only. Keep this exact Pawork Inspector-collapsed workbench visual system and all current layout intact. Move the “No project” workspace control and the helper text “File tools unavailable until a project is selected.” into the BOTTOM Composer itself, directly beside the model control. They must be inside the 92px Composer panel beneath the input placeholder. Remove that No project/helper row from the workspace header; the header should only show the New session title and Activity reopening control. Keep the selected UNASSIGNED New session row, unselected project sessions below, New task control, rename/archive action buttons, Local + gear footer (no user account), unavailable status fields, and no Inspector. Use exact 288px TaskRail. No numerical quota, no personal profile, no watermark.
```

### Provider 详情

```text
Use case: ui-mockup
Asset type: Pawork OPT-D correction of the Models & providers Settings screen, generate at 1440×1024.
Input images: Image 1 is the existing provider-page target to correct; Image 2 is the P2 visual token reference. Preserve the image 1 layout and the single Pawork dark native desktop language.
Primary request: Make only these corrections. In the Settings Rail, replace the “General” navigation item with “Network”; do not otherwise change the navigation structure. In the Default models list, Vision must show “Not set” as its selected value, because GLM-4V is disabled in the enabled-model popover; Conversation, Naming and Search stay enabled-model selections. Keep the Z.AI expanded provider detail, Proxy on switch, two status-only credential rows, Usage unavailable empty track, and anchored “Enabled models” popover with GLM-5.3 on, GLM-5.2 on, GLM-4V off, plus Enable all and Disable all. Bottom rail footer must be Local + gear only: no Jane Doe, no avatar, no profile or account controls. No raw secrets, endpoint, errors, account numbers, or quota numbers.
Constraints: preserve full-width Settings content, stable selected rail row, readable English, no change to any unrelated element, no watermark.
Avoid: personal profile, vision selecting a disabled model, General nav label, fake quota data.
```

### 模型启用状态板

```text
Use case: ui-mockup
Asset type: Pawork OPT-D correction of one four-state model-enablement reference board, generate at 1440×1024.
Input images: Image 1 is the existing 2×2 state board to correct; Image 2 is P2 token reference. Preserve Image 1’s single Pawork dark native desktop visual direction, its 2×2 structure, labels, and all unrelated content.
Primary request: Apply only these required state corrections across the board. In every Settings Rail shown on the board, replace “General” with “Network”. Panel 1 “Empty catalog”: the provider must be visibly Connected, but its catalog is empty. The anchored Enabled models popover must say “No models returned” and include a clear “Refresh catalog” action. Do not say Connect, do not suggest it is disconnected, and keep enable controls unavailable. Panel 3 “Partially enabled”: show the popover anchored to the Z.AI provider row/control specifically, not Anthropic; preserve GLM-5.3 on, GLM-5.2 on, GLM-4V off, and Enable all / Disable all. Panel 2 stays not connected and Panel 4 stays all off with an honest Composer “No enabled models” state. Bottom footers are Local + gear only, no Jane Doe or profile/avatar. No quota numbers, secrets, account details, fake model data, watermark, or other redesign.
Constraints: keep all panel labels readable and use the same dark surfaces, blue accent, thin dividers, restrained radii and 8px rhythm.
Avoid: Connect language in Empty catalog, a popover on Anthropic in Partially enabled, General navigation, personal account UI, clip/crop.
```

## 走查

- 收起图选中 `UNASSIGNED / New session`，Composer 内有 `No project` 与文件工具不可用提示。
- Provider 图 Vision 为 `Not set`，与 GLM-4V disabled 一致；导航为 `Network`。
- 状态板 Empty catalog 为已连接但 `No models returned`，有 `Refresh catalog`；部分启用弹层锚在 Z.AI。
- 所有本轮修订画幅底部为 Local + gear；无 quota 数字或凭证明文；P0–P2 基线未改。
