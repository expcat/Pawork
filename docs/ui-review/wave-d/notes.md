# R1 Wave D 进展记录（2026-08-27）

驱动：[scripts/ui-wave-d-state-a.sh](../../../scripts/ui-wave-d-state-a.sh) + [scripts/ui-wave-d-tools.py](../../../scripts/ui-wave-d-tools.py)。
定向回归：[scripts/test_ui_wave_d_tools.py](../../../scripts/test_ui_wave_d_tools.py)。

## 1. 已落地

- State A 闭环驱动：`seed → serve → ui-fixture desktop → timeline_stable → AX dump/frame → AXPress session-fx-ses-alpha-today → 再等 barrier → 截图归一 → ui-visual-diff → manifest/checklist`。同步只用 barrier/轮询，清理只删带 `.pawork-ui-fixture` marker 的 root。
- Desktop 启动走 [ui-fixture.sh desktop](../../../scripts/ui-fixture.sh)，token 由 socket 同目录 `gui.token` 解析，不把 `PAWORK_DATA_DIR` 套在 fixture 脚本上（会触发 root 安全校验）。
- AX identifier / barrier JSON 解析已按 Wave C dump 与 `settle_seq/session_id/entry_count` 对拍；3 个定向测试覆盖 identifier 提取、barrier 纯值输出、zones.json `current` 矩形回填。

## 2. 未完成（外部前提）

真窗口 U2/U3 闭环、两次从零基线 compare、`SIDEBAR_WIDTH` 故意漂移与 zones.json current 回填**尚未执行**。2026-08-26 取证时 macOS 屏幕锁定：loginwindow 全屏 onscreen、AX 树退化为自引用 `AXApplication`、`screencapture -l` 返回 `could not create image from window`。Host 连接与 `timeline_stable` 在锁屏下仍正常，已排除代码回归。

解锁屏幕后：

```bash
OUT_A=$(mktemp -d /tmp/pawork-wave-d-a.XXXXXX)
OUT_B=$(mktemp -d /tmp/pawork-wave-d-b.XXXXXX)
scripts/ui-wave-d-state-a.sh run --out "$OUT_A" --label baseline-1 \
  --write-zones docs/ui-review/state-a/zones.json
scripts/ui-wave-d-state-a.sh run --out "$OUT_B" --label baseline-2
scripts/ui-wave-d-state-a.sh compare --a "$OUT_A" --b "$OUT_B" \
  --report docs/ui-review/wave-d/repeatability.json
# 再把证据整理进 docs/ui-review/wave-d/state-a/ 与 drift/
```

视觉 zone FAIL 在 R1 是预期（R2 才做还原）；结构断言必须 PASS，漂移轮必须让 rail 宽度断言和/或 taskrail zone FAIL。
