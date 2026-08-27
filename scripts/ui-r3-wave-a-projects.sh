#!/usr/bin/env bash
# Pawork R3 Wave A - State C (Projects grouping) U2/U3 driver (scripts-only).
#
# Scenario: seed -> serve -> desktop (isolated PAWORK_DATA_DIR +
# PAWORK_UI_BARRIER_DIR via ui-fixture) -> wait timeline_stable -> AXPress
# session row fx-ses-alpha-today -> wait NEW timeline_stable round for that
# session (anti-stale seq) -> AXPress task-rail-grouping -> poll grouping-menu
# open in AX tree (record pre-switch value=Timeline + resolve the Projects
# menu-item identifier actually published) -> AXPress that item -> phase
# projects assertion polling (grouping=Projects, menu closed, project blocks
# present, date buckets absent, session/timeline retained) -> window
# screenshot -> normalize current.png -> ui-visual-diff against frozen
# docs/ui-review/state-c zones/mask -> diff-report evidence.
#
# 同步只用 barrier/轮询（相位断言本身充当布局收敛轮询），禁固定 sleep。
# Projects 菜单项 identifier 从树上实测解析：brief 冻结口径 group-Projects，
# 现网代码发布 group-projects；两种都接受，取在场者，证据留在
# ax-tree-grouping-menu.txt。清理只删自建 fixture root 与临时工具目录；
# 失败保留证据并退出非零。
#
# Usage: scripts/ui-r3-wave-a-projects.sh run --out <dir> [--label <name>]
#
# Exit codes: 0 structural pass (visual zone FAIL is recorded, not fatal);
#   2 usage error; 3 infrastructure failure; 4 structural assertion failure.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
FIXTURE="$SCRIPT_DIR/ui-fixture.sh"
SESSION_ID="fx-ses-alpha-today"
SESSION_ROW="session-$SESSION_ID"
BARRIER_TIMEOUT_SECS="${PAWORK_UI_BARRIER_TIMEOUT_SECS:-180}"
WINDOW_TIMEOUT_SECS="${PAWORK_UI_WINDOW_TIMEOUT_SECS:-120}"
PHASE_TIMEOUT_SECS="${PAWORK_UI_PHASE_TIMEOUT_SECS:-30}"

die() { echo "ui-r3-projects: $*" >&2; exit 3; }
usage() {
  echo "usage: scripts/ui-r3-wave-a-projects.sh run --out <dir> [--label <name>]" >&2
  exit 2
}

pick_python() {
  local candidate
  local chosen
  chosen="$(printenv PAWORK_WAVE_D_PYTHON || true)"
  if [[ -n "$chosen" ]]; then
    "$chosen" -c 'import PIL, numpy' >/dev/null 2>&1 \
      || die "PAWORK_WAVE_D_PYTHON lacks Pillow/numpy: $chosen"
    PY="$chosen"
    return
  fi
  for candidate in /tmp/pawork-wave-d-venv/bin/python python3; do
    if command -v "$candidate" >/dev/null 2>&1 \
        && "$candidate" -c 'import PIL, numpy' >/dev/null 2>&1; then
      PY="$candidate"
      return
    fi
  done
  die "no python with Pillow/numpy found; set PAWORK_WAVE_D_PYTHON"
}

MODE=""
OUT=""
LABEL="r3-wave-a-projects"
while (( $# )); do
  case "$1" in
    run) MODE="$1"; shift ;;
    --out) OUT="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "ui-r3-projects: unknown argument $1" >&2; usage ;;
  esac
done
[[ -n "$MODE" ]] || usage
[[ -n "$OUT" ]] || usage
if [[ -e "$OUT" && ! -d "$OUT" ]]; then
  die "--out must name a new or empty directory: $OUT"
fi
if [[ -d "$OUT" ]]; then
  shopt -s nullglob dotglob
  out_entries=("$OUT"/*)
  shopt -u nullglob dotglob
  (( ${#out_entries[@]} == 0 )) \
    || die "--out must be new or empty to prevent stale evidence: $OUT"
fi

pick_python
PY_TOOLS="$SCRIPT_DIR/ui-wave-d-tools.py"
[[ -f "$PY_TOOLS" ]] || die "missing $PY_TOOLS"
[[ -x "$FIXTURE" ]] || die "missing executable $FIXTURE"

mkdir -p "$OUT/logs" "$OUT/barriers"
OUT="$(cd "$OUT" && pwd)"
TRACE="$OUT/action-trace.txt"
: > "$TRACE"
trace() {
  echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" | tee -a "$TRACE" >&2
}

WORK="$(mktemp -d /tmp/pawork-r3-projects-tools.XXXXXX)"
ROOT="$(mktemp -d /tmp/pawork-ui-fixture.XXXXXX)"
ROOT="$(cd "$ROOT" && pwd -P)"
DESKTOP_PID=""
copy_runtime_evidence() {
  cp "$ROOT/logs/serve.log" "$OUT/logs/serve.log" 2>/dev/null || true
  cp "$ROOT/logs/desktop.log" "$OUT/logs/desktop.log" 2>/dev/null || true
  for barrier_file in "$ROOT"/barriers/*; do
    [[ -f "$barrier_file" ]] && cp "$barrier_file" "$OUT/barriers/" 2>/dev/null || true
  done
}
fixture_teardown() {
  local status=$?
  trap - EXIT
  copy_runtime_evidence
  unset PAWORK_DATA_DIR
  if [[ -n "${DESKTOP_PID:-}" ]] || [[ -f "$ROOT/desktop.pid" ]]; then
    "$FIXTURE" down --root "$ROOT" >/dev/null 2>&1 || true
  fi
  "$FIXTURE" clean --root "$ROOT" >/dev/null 2>&1 || true
  rm -rf "$WORK"
  exit $status
}
trap fixture_teardown EXIT

trace "run start label=$LABEL out=$OUT"
trace "compile helpers into $WORK"
swiftc -O -o "$WORK/ui-ax-dump" "$SCRIPT_DIR/ui-ax-dump.swift" 2>"$WORK/swiftc-axdump.err" \
  || { cat "$WORK/swiftc-axdump.err" >&2; die "ui-ax-dump compile failed"; }
swiftc -O -o "$WORK/ax-frames" "$SCRIPT_DIR/ui-ax-frames.swift" 2>"$WORK/swiftc-frames.err" \
  || { cat "$WORK/swiftc-frames.err" >&2; die "ax-frames compile failed"; }
AXDUMP="$WORK/ui-ax-dump"

trace "seed fixture root=$ROOT"
"$FIXTURE" seed --root "$ROOT"
trace "serve fixture (wait host_ready barrier)"
"$FIXTURE" serve --root "$ROOT"

trace "launch desktop via ui-fixture"
"$FIXTURE" desktop --root "$ROOT"
DESKTOP_PID="$(cat "$ROOT/desktop.pid")"
trace "desktop pid=$DESKTOP_PID"

wait_desktop_ready() {
  local deadline=$(( SECONDS + WINDOW_TIMEOUT_SECS ))
  local probe_attempts=0 probe_rc
  while :; do
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --out "$WORK/probe.txt" >/dev/null 2>&1
    probe_rc=$?
    set -e
    if (( probe_rc == 0 )) && grep -q 'identifier="session-list"' "$WORK/probe.txt"; then
      trace "desktop window ready (session-list rendered)"
      return 0
    fi
    probe_attempts=$(( probe_attempts + 1 ))
    if (( probe_attempts % 10 == 1 )); then
      trace "probe not-ready rc=$probe_rc attempt=$probe_attempts trusted=$(grep -o 'ax_trusted=[a-z]*' "$WORK/probe.txt" 2>/dev/null | head -1) windows=$(grep -c 'wid=' "$WORK/probe.txt" 2>/dev/null)"
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null \
      || { tail -5 "$ROOT/logs/desktop.log" >&2 || true; die "desktop exited early"; }
    if (( SECONDS >= deadline )); then
      cp "$WORK/probe.txt" "$OUT/ax-tree-probe-timeout.txt" 2>/dev/null || true
      copy_runtime_evidence
      tail -30 "$OUT/ax-tree-probe-timeout.txt" >&2 || true
      die "timeout waiting for desktop window/session-list (evidence in $OUT)"
    fi
    sleep 0.3
  done
}

# barrier 等待一律要求 seq 超过基线（新一轮 settle），防陈旧文件误过。
wait_timeline_stable() { # $1=min_seq(exclusive) $2=want_session(""=any) $3=min_entries $4=timeout_secs
  local min_seq="$1" want_session="$2" min_entries="$3" timeout_secs="$4"
  local deadline=$(( SECONDS + timeout_secs ))
  local seq session entries
  trace "wait timeline_stable (seq>$min_seq session=$want_session entries>=$min_entries)"
  while :; do
    seq="$("$PY" "$PY_TOOLS" barrier-read --file "$ROOT/barriers/timeline_stable" --field seq 2>/dev/null || true)"
    session="$("$PY" "$PY_TOOLS" barrier-read --file "$ROOT/barriers/timeline_stable" --field session 2>/dev/null || true)"
    entries="$("$PY" "$PY_TOOLS" barrier-read --file "$ROOT/barriers/timeline_stable" --field entries 2>/dev/null || true)"
    if [[ "$seq" =~ ^[0-9]+$ ]] && (( seq > min_seq )); then
      if [[ "$entries" =~ ^[0-9]+$ ]] && (( entries >= min_entries )); then
        if [[ -z "$want_session" || "$session" == "$want_session" ]]; then
          trace "timeline_stable ok seq=$seq session=$session entries=$entries"
          printf '%s' "$seq"
          return 0
        fi
      fi
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited while waiting timeline_stable"
    (( SECONDS < deadline )) || die "timeout waiting timeline_stable"
    sleep 0.2
  done
}

# 相位断言本身即布局收敛轮询：AX dump + frames + assert 反复重试直到相位
# 通过或超时；断言 JSON/树/几何文件落在 OUT 里兼作失败证据。
wait_phase() { # $1=phase $2=timeout_secs
  local phase="$1" timeout_secs="$2"
  local tree="$OUT/ax-tree-$phase.txt"
  local frames="$OUT/geometry-$phase.txt"
  local json="$OUT/assert-$phase.json"
  local deadline=$(( SECONDS + timeout_secs ))
  local attempt=0 dump_rc frames_rc assert_rc
  trace "wait phase=$phase (phase-aware assertion polling)"
  while :; do
    attempt=$(( attempt + 1 ))
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$tree" \
      --wid-out "$WORK/wid-$phase.txt" >/dev/null 2>&1
    dump_rc=$?
    "$WORK/ax-frames" "$DESKTOP_PID" > "$frames" 2>/dev/null
    frames_rc=$?
    "$PY" "$PY_TOOLS" assert --frames "$frames" --tree "$tree" \
      --phase "$phase" --out "$json" >/dev/null 2>&1
    assert_rc=$?
    set -e
    if (( dump_rc == 0 && frames_rc == 0 && assert_rc == 0 )); then
      trace "phase=$phase ok attempt=$attempt"
      return 0
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null \
      || { tail -5 "$ROOT/logs/desktop.log" >&2 || true; die "desktop exited during phase=$phase"; }
    (( SECONDS < deadline )) || {
      trace "phase=$phase TIMEOUT attempt=$attempt dump=$dump_rc frames=$frames_rc assert=$assert_rc"
      return 1
    }
    sleep 0.2
  done
}

require_phase() { # $1=phase
  wait_phase "$1" "$PHASE_TIMEOUT_SECS" || {
    echo "ui-r3-projects: structural assertion failed phase=$1 (evidence kept: $OUT)" >&2
    exit 4
  }
}

ax_press() { # $1=identifier $2=evidence-file $3=label
  trace "AXPress $1 ($3)"
  "$AXDUMP" --pid "$DESKTOP_PID" --press "$1" --action-only --out "$2" >/dev/null \
    || die "AXPress $1 failed (see $2)"
  grep -q 'result=0' "$2" || die "AXPress $1 result!=0 (see $2)"
  trace "AXPress $1 result=0"
}

# grouping 菜单打开轮询：菜单是本次 AXPress 的产物，存在性即新鲜性；
# 同一棵树同时取证切换前 grouping 值（必须 Timeline）与 Projects 菜单项
# identifier（冻结口径 group-Projects，现网发布 group-projects，取在场者）。
wait_grouping_menu() { # $1=tree-out
  local tree="$1"
  local deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
  local attempt=0 dump_rc
  trace "wait grouping-menu open"
  while :; do
    attempt=$(( attempt + 1 ))
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$tree" >/dev/null 2>&1
    dump_rc=$?
    set -e
    if (( dump_rc == 0 )) && grep -q 'identifier="grouping-menu"' "$tree"; then
      trace "grouping-menu open attempt=$attempt"
      return 0
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null \
      || { tail -5 "$ROOT/logs/desktop.log" >&2 || true; die "desktop exited waiting grouping-menu"; }
    (( SECONDS < deadline )) || {
      trace "grouping-menu TIMEOUT attempt=$attempt dump=$dump_rc"
      return 1
    }
    sleep 0.2
  done
}

resolve_projects_menu_item() { # $1=tree -> stdout identifier
  local tree="$1"
  local item
  for item in group-Projects group-projects; do
    if grep -q "identifier=\"$item\"" "$tree"; then
      printf '%s' "$item"
      return 0
    fi
  done
  return 1
}

wait_desktop_ready

trace "place Desktop window deterministically on the main display"
"$WORK/ax-frames" "$DESKTOP_PID" --place-main > "$OUT/window-place.txt" \
  || die "failed to place Desktop window (see $OUT/window-place.txt)"
grep -q '# place-main result=0 ' "$OUT/window-place.txt" \
  || die "Desktop main-display placement did not converge (see $OUT/window-place.txt)"

INITIAL_SEQ="$(wait_timeline_stable 0 "" 0 "$BARRIER_TIMEOUT_SECS")"

trace "open session before grouping switch"
ax_press "$SESSION_ROW" "$OUT/action-press-session.txt" "open session"
wait_timeline_stable "$INITIAL_SEQ" "$SESSION_ID" 1 "$BARRIER_TIMEOUT_SECS" >/dev/null

trace "open grouping menu"
ax_press "task-rail-grouping" "$OUT/action-press-grouping.txt" "open grouping menu"
GROUPING_MENU_TREE="$OUT/ax-tree-grouping-menu.txt"
wait_grouping_menu "$GROUPING_MENU_TREE" \
  || { echo "ui-r3-projects: grouping-menu not observed (evidence kept: $OUT)" >&2; exit 4; }
grep 'identifier="task-rail-grouping"' "$GROUPING_MENU_TREE" | grep -q 'value="Timeline"' \
  || die "pre-switch grouping value is not Timeline (see $GROUPING_MENU_TREE)"
trace "pre-switch grouping=Timeline confirmed"
PROJECTS_ITEM="$(resolve_projects_menu_item "$GROUPING_MENU_TREE")" \
  || die "no Projects menu item (group-Projects/group-projects) in $GROUPING_MENU_TREE"
printf '%s\n' "$PROJECTS_ITEM" > "$OUT/grouping-projects-identifier.txt"
trace "projects menu item resolved: $PROJECTS_ITEM"

ax_press "$PROJECTS_ITEM" "$OUT/action-press-group-projects.txt" "select Projects grouping"
require_phase "projects"

trace "capture State C (Projects grouping)"
WID="$(cat "$WORK/wid-projects.txt")"
trace "screencapture wid=$WID"
screencapture -x -o -l "$WID" "$WORK/shot-raw.png" || die "screencapture failed wid=$WID"
[[ -s "$WORK/shot-raw.png" ]] || die "screenshot empty wid=$WID"
"$PY" "$PY_TOOLS" normalize --shot "$WORK/shot-raw.png" \
  --tree "$OUT/ax-tree-projects.txt" --wid "$WORK/wid-projects.txt" \
  --frames "$OUT/geometry-projects.txt" \
  --out "$OUT/current.png" --json "$OUT/normalize.json" >/dev/null
trace "normalized -> $OUT/current.png"

trace "visual diff gate (state-c frozen zones)"
set +e
"$PY" "$SCRIPT_DIR/ui-visual-diff.py" \
  --reference "$REPO_ROOT/docs/ui-review/state-c/reference.png" \
  --current "$OUT/current.png" \
  --zones "$REPO_ROOT/docs/ui-review/state-c/zones.json" \
  --masks "$REPO_ROOT/docs/ui-review/state-c/mask.json" \
  --out "$OUT/diff" > "$OUT/diff-run.txt" 2>&1
GATE_EXIT=$?
set -e
cat "$OUT/diff-run.txt"
trace "visual diff exit=$GATE_EXIT (0=all pass / 1=zone FAIL / 2=invalid input)"
[[ "$GATE_EXIT" != 2 ]] || { cat "$OUT/diff-run.txt" >&2; die "ui-visual-diff invalid input (exit 2)"; }

copy_runtime_evidence
"$PY" "$PY_TOOLS" shell-manifest --dir "$OUT" --repo "$REPO_ROOT" \
  --scenario state-c-projects --label "$LABEL"

trace "teardown fixture root=$ROOT"
"$FIXTURE" down --root "$ROOT"
"$FIXTURE" clean --root "$ROOT"
DESKTOP_PID=""
rm -rf "$WORK"
trace "run done (structural pass; gate_exit=$GATE_EXIT)"
exit 0
