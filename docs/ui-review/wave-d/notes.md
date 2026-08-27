# R1 Wave D 收口记录（2026-08-27）

驱动：[scripts/ui-wave-d-state-a.sh](../../../scripts/ui-wave-d-state-a.sh) + [scripts/ui-wave-d-tools.py](../../../scripts/ui-wave-d-tools.py)。
定向回归：[scripts/test_ui_wave_d_tools.py](../../../scripts/test_ui_wave_d_tools.py)。

## 1. 闭环结果

- State A 闭环驱动：`seed → serve → ui-fixture desktop → timeline_stable → AX dump/frame → AXPress session-fx-ses-alpha-today → 再等 barrier → 截图归一 → ui-visual-diff → manifest/checklist`。同步只用 barrier/轮询，清理只删带 `.pawork-ui-fixture` marker 的 root。
- Desktop 启动走 [ui-fixture.sh desktop](../../../scripts/ui-fixture.sh)，token 由 socket 同目录 `gui.token` 解析，不把 `PAWORK_DATA_DIR` 套在 fixture 脚本上（会触发 root 安全校验）。
- AX identifier / barrier JSON 解析已按 Wave C dump 与 `settle_seq/session_id/entry_count` 对拍；8 个定向测试覆盖注释去重、barrier 纯值、焦点边界、已知 Composer 偏差的非阻塞记录、ICC→sRGB 归一、zones `current` 映射及非空证据目录拒绝（防陈旧产物混入）。
- 驱动通过 AX 把 1440×1056 窗口确定性居中到主显示器，避免两块显示器的 ICC 差异污染重复性；截图读取 embedded ICC、显式转换为 sRGB，再输出无 ICC `RGB` 1440×1024 内容图。
- [baseline-1](state-a/baseline-1/) / [baseline-2](state-a/baseline-2/) 均从空 fixture 启动并通过结构门禁；两份 `current.png` 字节相同，zone/global 数值指纹完全一致（[repeatability.json](repeatability.json)）。State A 规范产物已同步到 [state-a](../state-a/) 根包。
- 视觉还原不在 R1 冒充完成：当前 9/9 zone 均低于 0.99，global 辅助 SSIM 0.336185；Composer AX group 实测 156px，作为 F-09 已知视觉偏差记为 `OBSERVED-FAIL`，R2 仍须修到 88–94px。

## 2. 故意漂移与恢复

- 临时把 `apps/desktop/src/ui/theme.rs` 的 `SIDEBAR_WIDTH` 从 288 改为 320；[drift](drift/) 初始/最终 `rail-width` 都硬失败，驱动退出 4。manifest 同时记录 `theme_rs_dirty=true`，与基线的数值指纹比较有 12 项差异（[drift-detection.json](drift-detection.json)）。
- token 随即恢复 288，`theme.rs` 无残留 diff；[recovery](recovery/) 再跑结构通过，截图与 baseline-1 字节一致、数值指纹一致（[recovery-compare.json](recovery-compare.json)）。

## 3. 复现

```bash
OUT_A="$(mktemp -d /tmp/pawork-wave-d-a.XXXXXX)"
OUT_B="$(mktemp -d /tmp/pawork-wave-d-b.XXXXXX)"
scripts/ui-wave-d-state-a.sh run \
  --out "$OUT_A" --label baseline-1
scripts/ui-wave-d-state-a.sh run \
  --out "$OUT_B" --label baseline-2
scripts/ui-wave-d-state-a.sh compare \
  --a "$OUT_A" --b "$OUT_B" --report /tmp/pawork-wave-d-repeatability.json
```

`--out` 必须是新建或空目录，防止上次运行的 diff/barrier 混入本轮证据；审核通过后再显式同步规范证据。屏幕必须解锁并授予 Accessibility / Screen Recording。视觉 zone FAIL 在 R1 是预期记录（R2 起还原）；正常轮结构断言必须 PASS，漂移轮必须非零退出。
