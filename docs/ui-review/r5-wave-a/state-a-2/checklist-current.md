# State A checklist-current - r5a-2

Generated 2026-08-28T08:19:43Z; manifest: run-manifest.json.

Structural assertions (AX tree + frame probe):

- [PASS] initial/root-1440x1024 - root w=1440.0 h=1024.0 (contract 1440x1024 +/-0.5)
- [PASS] initial/rail-width - task-rail w=288.0 (contract 288 +/-4.32)
- [PASS] initial/inspector-width - inspector w=440.0 (contract 440 +/-6.6)
- [PASS] initial/inspector-x - inspector rel_x=1000.0 (expected 1000)
- [PASS] initial/statusbar-height - status-bar h=24.0 (contract 24 +/-0.5)
- [PASS] initial/composer-height - composer h=91.0 (contract 88-94, component tolerance [86,96]; not the frozen R1 baseline; blocking drift)
- [PASS] initial/composer-above-statusbar - composer bottom=1000.0 statusbar top=1000.0
- [PASS] initial/workspace-span - workspace x=288.0..1000.0 expected 288.0..1000.0
- [PASS] initial/shell-skeleton - shell skeleton present (phase=initial)
- [PASS] initial/no-unknown-role - role=? count=0
- [PASS] initial/focus-start - initial focus=composer-input (allowed: none, pawork-root, or composer-input; exactly one at most)
- [PASS] initial/initial-selected-observed - startup selected rows=none (observation only)
- [PASS] final/root-1440x1024 - root w=1440.0 h=1024.0 (contract 1440x1024 +/-0.5)
- [PASS] final/rail-width - task-rail w=288.0 (contract 288 +/-4.32)
- [PASS] final/inspector-width - inspector w=440.0 (contract 440 +/-6.6)
- [PASS] final/inspector-x - inspector rel_x=1000.0 (expected 1000)
- [PASS] final/statusbar-height - status-bar h=24.0 (contract 24 +/-0.5)
- [PASS] final/composer-height - composer h=91.0 (contract 88-94, component tolerance [86,96]; not the frozen R1 baseline; blocking drift)
- [PASS] final/composer-above-statusbar - composer bottom=1000.0 statusbar top=1000.0
- [PASS] final/workspace-span - workspace x=288.0..1000.0 expected 288.0..1000.0
- [PASS] final/shell-skeleton - shell skeleton present (phase=final)
- [PASS] final/no-unknown-role - role=? count=0
- [PASS] final/session-selected - selected=1 rows: session-fx-ses-alpha-today
- [PASS] final/timeline-loaded - timeline-entry-evt-fx-ses-alpha-today-* count=13
- [PASS] final/focus-composer-after-select - focus after select=composer-input

Visual gate (R1 record; zone FAIL expected until R2 restoration):

- gate exit=1; zones passed 0/9 (threshold=0.99); global SSIM=0.658124 (auxiliary)
- [FAIL] zone taskrail ssim=0.693805
- [FAIL] zone header-left ssim=0.939623
- [FAIL] zone header-right ssim=0.882710
- [FAIL] zone timeline ssim=0.679378
- [FAIL] zone composer-left ssim=0.423299
- [FAIL] zone composer-right ssim=0.618954
- [FAIL] zone inspector-body ssim=0.599085
- [FAIL] zone inspector-right ssim=0.739885
- [FAIL] zone statusbar ssim=0.617477

Evidence pointers: current.png / ax-tree-initial.txt / ax-tree.txt /
geometry-initial.txt / geometry-final.txt / action-press-session.txt /
action-trace.txt / window-place.txt / diff/diff-report.json / logs/ /
barriers/ / normalize.json
