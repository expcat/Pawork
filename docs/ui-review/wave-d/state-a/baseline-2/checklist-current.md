# State A checklist-current - baseline-2

Generated 2026-08-26T23:44:37Z; manifest: run-manifest.json.

Structural assertions (AX tree + frame probe):

- [PASS] initial/root-1440x1024 - root w=1440.0 h=1024.0 (contract 1440x1024 +/-0.5)
- [PASS] initial/rail-width - task-rail w=288.0 (contract 288 +/-4.32)
- [PASS] initial/inspector-width - inspector w=440.0 (contract 440 +/-6.6)
- [PASS] initial/inspector-x - inspector rel_x=1000.0 (expected 1000)
- [PASS] initial/statusbar-height - status-bar h=24.0 (contract 24 +/-0.5)
- [OBSERVED-FAIL] initial/composer-height - composer h=156.0 (contract 88-94, component tolerance [86,96]; known F-09 visual drift, recorded but nonblocking for the R1 harness baseline)
- [PASS] initial/composer-above-statusbar - composer bottom=1000.0 statusbar top=1000.0
- [PASS] initial/workspace-span - workspace x=288.0..1000.0 expected 288.0..1000.0
- [PASS] initial/three-column-skeleton - rail/workspace/inspector/composer/statusbar present
- [PASS] initial/no-unknown-role - role=? count=0
- [PASS] initial/focus-start - initial focus=composer-input (allowed: none, pawork-root, or composer-input; exactly one at most)
- [PASS] initial/initial-selected-observed - startup selected rows=none (observation only)
- [PASS] final/root-1440x1024 - root w=1440.0 h=1024.0 (contract 1440x1024 +/-0.5)
- [PASS] final/rail-width - task-rail w=288.0 (contract 288 +/-4.32)
- [PASS] final/inspector-width - inspector w=440.0 (contract 440 +/-6.6)
- [PASS] final/inspector-x - inspector rel_x=1000.0 (expected 1000)
- [PASS] final/statusbar-height - status-bar h=24.0 (contract 24 +/-0.5)
- [OBSERVED-FAIL] final/composer-height - composer h=156.0 (contract 88-94, component tolerance [86,96]; known F-09 visual drift, recorded but nonblocking for the R1 harness baseline)
- [PASS] final/composer-above-statusbar - composer bottom=1000.0 statusbar top=1000.0
- [PASS] final/workspace-span - workspace x=288.0..1000.0 expected 288.0..1000.0
- [PASS] final/three-column-skeleton - rail/workspace/inspector/composer/statusbar present
- [PASS] final/no-unknown-role - role=? count=0
- [PASS] final/session-selected - selected=1 rows: session-fx-ses-alpha-today
- [PASS] final/timeline-loaded - timeline-entry-evt-fx-ses-alpha-today-* count=17
- [PASS] final/focus-composer-after-select - focus after select=composer-input

Visual gate (R1 record; zone FAIL expected until R2 restoration):

- gate exit=1; zones passed 0/9 (threshold=0.99); global SSIM=0.336185 (auxiliary)
- [FAIL] zone taskrail ssim=0.377997
- [FAIL] zone header-left ssim=0.233781
- [FAIL] zone header-right ssim=0.305131
- [FAIL] zone timeline ssim=0.301488
- [FAIL] zone composer-left ssim=0.240907
- [FAIL] zone composer-right ssim=0.281251
- [FAIL] zone inspector-body ssim=0.408333
- [FAIL] zone inspector-right ssim=0.376173
- [FAIL] zone statusbar ssim=0.405016

Evidence pointers: current.png / ax-tree-initial.txt / ax-tree.txt /
geometry-initial.txt / geometry-final.txt / action-press-session.txt /
action-trace.txt / window-place.txt / diff/diff-report.json / logs/ /
barriers/ / normalize.json
