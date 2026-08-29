#!/usr/bin/env bash
# Pawork R6 Wave A - Connected State A/B real-window capture driver.
#
# Scenario: seed -> serve -> desktop (isolated PAWORK_DATA_DIR +
# PAWORK_UI_BARRIER_DIR via ui-fixture) -> wait session-list AX ready +
# timeline_stable -> AXPress session row (selected-session timeline, R1
# frozen State A semantics) -> State A (Inspector open, default Changes)
# structural assert -> screenshot -> normalize -> visual diff. The SAME
# desktop session then continues into State B without restarting:
#   1. AXPress inspector-collapse -> poll AX until the inspector node is
#      gone -> screenshot/normalize/visual diff (State B visual evidence).
#   2. AXPress inspector-toggle (collapsed Header Activity trigger) -> poll
#      until activity-popover appears -> assert r6-state-b-open -> archive
#      shot-activity-popover.png (evidence only, not in the SSIM gate).
#   3. AXPress activity-open-changes -> poll until the inspector node is
#      back -> assert r6-state-b-resumed.
#
# Sync uses barriers/AX polling only (no fixed sleeps for readiness).
# Cleans only fixture roots and tool dirs it created itself. On failure the
# evidence directory is preserved and the script exits non-zero.
#
# Usage:
#   scripts/ui-r6-wave-a-states.sh run --out <dir> [--label <name>]
#
# Exit codes: 0 structural pass (visual zone FAIL is a recorded known-gap,
#   SSIM gate closes in R8); 2 usage error; 3 infrastructure failure;
#   4 structural assertion failure.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
FIXTURE="$SCRIPT_DIR/ui-fixture.sh"
WAVE_D_TOOLS="$SCRIPT_DIR/ui-wave-d-tools.py"
VISUAL_DIFF="$SCRIPT_DIR/ui-visual-diff.py"
AXDUMP_SRC="$SCRIPT_DIR/ui-ax-dump.swift"
FRAMES_SRC="$SCRIPT_DIR/ui-ax-frames.swift"
BARRIER_TIMEOUT_SECS="${PAWORK_UI_BARRIER_TIMEOUT_SECS:-180}"
WINDOW_TIMEOUT_SECS="${PAWORK_UI_WINDOW_TIMEOUT_SECS:-120}"
PHASE_TIMEOUT_SECS="${PAWORK_UI_PHASE_TIMEOUT_SECS:-30}"
RUN_TIMEOUT_SECS="${PAWORK_UI_RUN_TIMEOUT_SECS:-900}"
SESSION_ID="fx-ses-alpha-today"
SESSION_ROW="session-$SESSION_ID"

die() { echo "ui-r6a-states: $*" >&2; exit 3; }
usage() {
  echo "usage: scripts/ui-r6-wave-a-states.sh run --out <dir> [--label <name>]" >&2
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
LABEL="r6-wave-a-states"
while (( $# )); do
  case "$1" in
    run) MODE="$1"; shift ;;
    --out) OUT="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "ui-r6a-states: unknown argument $1" >&2; usage ;;
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
[[ -f "$WAVE_D_TOOLS" ]] || die "missing $WAVE_D_TOOLS"
[[ -f "$VISUAL_DIFF" ]] || die "missing $VISUAL_DIFF"
[[ -f "$AXDUMP_SRC" ]] || die "missing $AXDUMP_SRC"
[[ -f "$FRAMES_SRC" ]] || die "missing $FRAMES_SRC"
[[ -x "$FIXTURE" ]] || die "missing executable $FIXTURE"
for state_name in state-a state-b; do
  for asset in reference.png zones.json mask.json; do
    [[ -f "$REPO_ROOT/docs/ui-review/$state_name/$asset" ]] \
      || die "missing visual asset docs/ui-review/$state_name/$asset"
  done
done

mkdir -p "$OUT/state-a/logs" "$OUT/state-a/barriers" \
  "$OUT/state-b/logs" "$OUT/state-b/barriers"
OUT="$(cd "$OUT" && pwd)"
TRACE="$OUT/action-trace.txt"
: > "$TRACE"
trace() {
  echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" | tee -a "$TRACE" >&2
}

WORK="$(mktemp -d /tmp/pawork-r6a-states-tools.XXXXXX)"
ROOT="$(mktemp -d /tmp/pawork-ui-fixture.XXXXXX)"
ROOT="$(cd "$ROOT" && pwd)"
DESKTOP_PID=""
RUN_WATCHDOG=""
copy_runtime_evidence() {
  local state_dir barrier_file
  for state_dir in "$OUT/state-a" "$OUT/state-b"; do
    cp "$ROOT/logs/serve.log" "$state_dir/logs/serve.log" 2>/dev/null || true
    cp "$ROOT/logs/desktop.log" "$state_dir/logs/desktop.log" 2>/dev/null || true
    for barrier_file in "$ROOT"/barriers/*; do
      [[ -f "$barrier_file" ]] && cp "$barrier_file" "$state_dir/barriers/" 2>/dev/null || true
    done
  done
}
fixture_teardown() {
  local status=$?
  trap - EXIT
  kill "${RUN_WATCHDOG:-}" 2>/dev/null || true
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

(
  sleep "$RUN_TIMEOUT_SECS"
  kill -TERM "$$" 2>/dev/null || true
) &
RUN_WATCHDOG=$!
trap 'kill "$RUN_WATCHDOG" 2>/dev/null || true; die "run watchdog timeout ($RUN_TIMEOUT_SECS)"' TERM

trace "run start label=$LABEL out=$OUT"
trace "compile helpers into $WORK"
swiftc -O -o "$WORK/ui-ax-dump" "$AXDUMP_SRC" 2>"$WORK/swiftc-axdump.err" \
  || { cat "$WORK/swiftc-axdump.err" >&2; die "ui-ax-dump compile failed"; }
swiftc -O -o "$WORK/ax-frames" "$FRAMES_SRC" 2>"$WORK/swiftc-frames.err" \
  || { cat "$WORK/swiftc-frames.err" >&2; die "ax-frames compile failed"; }
AXDUMP="$WORK/ui-ax-dump"
FRAMES="$WORK/ax-frames"

trace "seed fixture root=$ROOT"
"$FIXTURE" seed --root "$ROOT"
trace "serve fixture (wait host_ready barrier)"
"$FIXTURE" serve --root "$ROOT"
trace "launch desktop via ui-fixture (token from socket sibling gui.token)"
"$FIXTURE" desktop --root "$ROOT"
DESKTOP_PID="$(cat "$ROOT/desktop.pid")"
trace "desktop pid=$DESKTOP_PID"

is_ax_recursion() { # $1=probe-file: AX server registration failure signature
  local probe="$1" ax_app_lines
  [[ -f "$probe" ]] || return 1
  ax_app_lines="$(grep -c 'role=AXApplication' "$probe" 2>/dev/null || true)"
  [[ "$ax_app_lines" =~ ^[0-9]+$ ]] || ax_app_lines=0
  if (( ax_app_lines >= 3 )) && grep -q '# identifiers (none)' "$probe"; then
    return 0
  fi
  # axdump 已在进程内检测到递归并切 AXWindows 回退根；回退树仍无 session-list
  # 说明回退降级/为空，等同递归态，走 desktop-restart 兜底。
  grep -q '^# WARN ax-fallback=axwindows' "$probe" \
    && ! grep -q 'identifier="session-list"' "$probe"
}

desktop_alive_or_die() { # $1=context
  kill -0 "$DESKTOP_PID" 2>/dev/null \
    || { tail -5 "$ROOT/logs/desktop.log" >&2 || true; die "desktop exited early ($1)"; }
}

wait_desktop_ready() {
  local deadline=$(( SECONDS + WINDOW_TIMEOUT_SECS ))
  local probe_attempts=0 probe_rc ax_restarts=0
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
    if (( probe_attempts >= 30 )) && is_ax_recursion "$WORK/probe.txt"; then
      if (( ax_restarts >= 3 )); then
        cp "$WORK/probe.txt" "$OUT/ax-tree-probe-recursive-final.txt" 2>/dev/null || true
        die "AX recursion persisted after $ax_restarts desktop restarts (evidence in $OUT)"
      fi
      ax_restarts=$(( ax_restarts + 1 ))
      trace "AX recursion signature detected (not-ready=$probe_attempts); desktop-restart $ax_restarts/3"
      "$FIXTURE" desktop-restart --root "$ROOT"
      DESKTOP_PID="$(cat "$ROOT/desktop.pid")"
      trace "desktop restarted pid=$DESKTOP_PID (AX ready counter reset)"
      probe_attempts=0
    fi
    desktop_alive_or_die "AX ready polling"
    if (( SECONDS >= deadline )); then
      cp "$WORK/probe.txt" "$OUT/ax-tree-probe-timeout.txt" 2>/dev/null || true
      cp "$ROOT/logs/serve.log" "$OUT/logs-timeout-serve.log" 2>/dev/null || true
      tail -30 "$OUT/ax-tree-probe-timeout.txt" >&2 || true
      die "timeout waiting for desktop window/session-list (evidence in $OUT)"
    fi
    sleep 0.3
  done
}

barrier_field() { # $1=file $2=field
  "$PY" "$WAVE_D_TOOLS" barrier-read --file "$1" --field "$2" 2>/dev/null || true
}

wait_timeline_stable() { # $1=min_seq(exclusive) $2=want_session(""=any) $3=min_entries $4=timeout_secs -> stdout seq
  local min_seq="$1" want_session="$2" min_entries="$3" timeout_secs="$4"
  local deadline=$(( SECONDS + timeout_secs ))
  local seq session entries
  trace "wait timeline_stable (seq>$min_seq session=$want_session entries>=$min_entries)"
  while :; do
    seq="$(barrier_field "$ROOT/barriers/timeline_stable" seq)"
    session="$(barrier_field "$ROOT/barriers/timeline_stable" session)"
    entries="$(barrier_field "$ROOT/barriers/timeline_stable" entries)"
    if [[ "$seq" =~ ^[0-9]+$ ]] && (( seq > min_seq )); then
      if [[ "$entries" =~ ^[0-9]+$ ]] && (( entries >= min_entries )); then
        if [[ -z "$want_session" || "$session" == "$want_session" ]]; then
          trace "timeline_stable ok seq=$seq session=$session entries=$entries"
          printf '%s' "$seq"
          return 0
        fi
      fi
    fi
    desktop_alive_or_die "waiting timeline_stable"
    (( SECONDS < deadline )) || die "timeout waiting timeline_stable"
    sleep 0.2
  done
}

wait_tree_state() { # $1=pattern $2=want(present|absent) $3=timeout_secs $4=label
  local pattern="$1" want="$2" timeout_secs="$3" label="$4"
  local deadline=$(( SECONDS + timeout_secs ))
  local attempt=0 dump_rc=0 found=1
  trace "wait tree $want: $label"
  while :; do
    attempt=$(( attempt + 1 ))
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$WORK/wait-tree.txt" >/dev/null 2>&1
    dump_rc=$?
    if (( dump_rc == 0 )); then
      grep -q "$pattern" "$WORK/wait-tree.txt"
      found=$?
    fi
    set -e
    if (( dump_rc == 0 )); then
      if [[ "$want" == "present" ]] && (( found == 0 )); then
        trace "tree present ok: $label attempt=$attempt"
        return 0
      fi
      if [[ "$want" == "absent" ]] && (( found != 0 )); then
        trace "tree absent ok: $label attempt=$attempt"
        return 0
      fi
    fi
    desktop_alive_or_die "waiting $label"
    (( SECONDS < deadline )) || {
      cp "$WORK/wait-tree.txt" "$OUT/ax-tree-wait-timeout-$label.txt" 2>/dev/null || true
      trace "tree $want TIMEOUT: $label attempt=$attempt dump=$dump_rc"
      if (( dump_rc != 0 )); then
        die "AX dump unavailable while waiting $label (last rc=$dump_rc)"
      fi
      return 1
    }
    sleep 0.2
  done
}

ax_press() { # $1=identifier $2=evidence-file $3=label
  trace "AXPress $1 ($3)"
  "$AXDUMP" --pid "$DESKTOP_PID" --press "$1" --action-only --out "$2" >/dev/null \
    || die "AXPress $1 failed (see $2)"
  grep -q 'result=0' "$2" || die "AXPress $1 result!=0 (see $2)"
  trace "AXPress $1 result=0"
}

require_phase() { # $1=phase $2=tree $3=geometry $4=json (poll dump+frames+assert)
  # assert rc contract: 0=pass immediately; 5=blocking failure, retryable
  # until the phase timeout (AX tree may lag), then structural exit 4;
  # any other code (1/2/3...) is an infrastructure failure -> exit 3.
  local phase="$1" tree="$2" geometry="$3" json="$4"
  local deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
  local attempt=0 dump_rc=0 frames_rc=0 assert_rc=0
  trace "assert phase=$phase (poll dump+frames+assert)"
  while :; do
    attempt=$(( attempt + 1 ))
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$tree" >/dev/null 2>&1
    dump_rc=$?
    "$FRAMES" "$DESKTOP_PID" > "$geometry" 2>/dev/null
    frames_rc=$?
    if (( dump_rc == 0 && frames_rc == 0 )); then
      "$PY" "$WAVE_D_TOOLS" assert --frames "$geometry" --tree "$tree" \
        --phase "$phase" --out "$json" >/dev/null 2>&1
      assert_rc=$?
    fi
    set -e
    if (( dump_rc == 0 && frames_rc == 0 && assert_rc == 0 )); then
      trace "phase=$phase ok attempt=$attempt"
      return 0
    fi
    if (( dump_rc == 0 && frames_rc == 0 && assert_rc != 0 && assert_rc != 5 )); then
      die "assert phase=$phase exited $assert_rc (expected 0/5; see $WAVE_D_TOOLS usage)"
    fi
    desktop_alive_or_die "phase=$phase"
    (( SECONDS < deadline )) || {
      trace "phase=$phase TIMEOUT attempt=$attempt dump=$dump_rc frames=$frames_rc assert=$assert_rc"
      if (( dump_rc != 0 )); then
        die "AX dump unavailable during phase=$phase (last rc=$dump_rc)"
      fi
      if (( frames_rc != 0 )); then
        die "ax-frames dump unavailable during phase=$phase (last rc=$frames_rc)"
      fi
      return 1
    }
    sleep 0.2
  done
}

window_wid() { # stdout wid; refreshes $WORK/wid.txt for normalize
  local wid
  "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 \
    --out "$WORK/wid-dump.txt" --wid-out "$WORK/wid.txt" >/dev/null \
    || die "AX dump for window id failed"
  wid="$(cat "$WORK/wid.txt")"
  [[ -n "$wid" ]] || die "empty window id from AX dump"
  printf '%s' "$wid"
}

normalize_shot() { # $1=raw-shot $2=tree $3=geometry $4=state-dir
  local raw="$1" tree="$2" geometry="$3" state_dir="$4" wid
  wid="$(window_wid)"
  trace "screencapture wid=$wid -> $state_dir/current.png"
  screencapture -x -o -l "$wid" "$raw" || die "screencapture failed (wid=$wid)"
  [[ -s "$raw" ]] || die "screenshot empty (wid=$wid)"
  "$PY" "$WAVE_D_TOOLS" normalize --shot "$raw" --tree "$tree" \
    --wid "$WORK/wid.txt" --frames "$geometry" \
    --out "$state_dir/current.png" --json "$state_dir/normalize.json" \
    || die "normalize failed ($state_dir)"
}

archive_shot() { # $1=target-png $2=label
  local target="$1" label="$2" wid
  wid="$(window_wid)"
  trace "screencapture wid=$wid -> $target ($label)"
  screencapture -x -o -l "$wid" "$target" || die "screencapture failed ($label, wid=$wid)"
  [[ -s "$target" ]] || die "screenshot empty ($label, wid=$wid)"
}

wait_desktop_ready

trace "place Desktop window deterministically on the main display"
"$FRAMES" "$DESKTOP_PID" --place-main > "$OUT/state-a/window-place.txt" \
  || die "failed to place Desktop window on main display (see $OUT/state-a/window-place.txt)"
grep -q '# place-main result=0 ' "$OUT/state-a/window-place.txt" \
  || die "Desktop main-display placement did not converge (see $OUT/state-a/window-place.txt)"

INITIAL_SEQ="$(wait_timeline_stable 0 "" 0 "$BARRIER_TIMEOUT_SECS")"
trace "initial timeline_stable seq=$INITIAL_SEQ"

STATE_A="$OUT/state-a"
STATE_B="$OUT/state-b"

trace "State A: AXPress $SESSION_ROW (select session before capture)"
ax_press "$SESSION_ROW" "$STATE_A/action-press-session.txt" "select $SESSION_ID"
wait_timeline_stable "$INITIAL_SEQ" "$SESSION_ID" 1 "$BARRIER_TIMEOUT_SECS" >/dev/null

trace "State A: Inspector open with default Changes (r6-state-a)"
ASSERT_A_OK=0
require_phase r6-state-a "$STATE_A/ax-tree.txt" "$STATE_A/geometry.txt" \
  "$STATE_A/assert-r6-state-a.json" || ASSERT_A_OK=1
normalize_shot "$WORK/shot-state-a-raw.png" "$STATE_A/ax-tree.txt" \
  "$STATE_A/geometry.txt" "$STATE_A"
set +e
"$PY" "$VISUAL_DIFF" \
  --reference "$REPO_ROOT/docs/ui-review/state-a/reference.png" \
  --current "$STATE_A/current.png" \
  --zones "$REPO_ROOT/docs/ui-review/state-a/zones.json" \
  --masks "$REPO_ROOT/docs/ui-review/state-a/mask.json" \
  --out "$STATE_A/diff" > "$STATE_A/diff-run.txt" 2>&1
GATE_A=$?
set -e
cat "$STATE_A/diff-run.txt"
trace "visual diff state-a exit=$GATE_A (0=all pass / 1=zone FAIL known-gap / 2=invalid input)"
[[ "$GATE_A" != 2 ]] || { cat "$STATE_A/diff-run.txt" >&2; die "ui-visual-diff invalid input (state-a, exit 2)"; }

trace "State B step 1: collapse Inspector (same desktop session)"
ax_press inspector-collapse "$STATE_B/action-press-inspector-collapse.txt" "collapse inspector"
wait_tree_state 'identifier="inspector"' absent "$PHASE_TIMEOUT_SECS" inspector-collapsed \
  || { echo "ui-r6a-states: inspector did not collapse (evidence kept: $OUT)" >&2; exit 4; }
"$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$STATE_B/ax-tree-collapsed.txt" >/dev/null \
  || die "AX dump failed (see $STATE_B/ax-tree-collapsed.txt)"
"$FRAMES" "$DESKTOP_PID" > "$STATE_B/geometry-collapsed.txt" \
  || die "ax-frames dump failed (see $STATE_B/geometry-collapsed.txt)"
normalize_shot "$WORK/shot-state-b-raw.png" "$STATE_B/ax-tree-collapsed.txt" \
  "$STATE_B/geometry-collapsed.txt" "$STATE_B"
set +e
"$PY" "$VISUAL_DIFF" \
  --reference "$REPO_ROOT/docs/ui-review/state-b/reference.png" \
  --current "$STATE_B/current.png" \
  --zones "$REPO_ROOT/docs/ui-review/state-b/zones.json" \
  --masks "$REPO_ROOT/docs/ui-review/state-b/mask.json" \
  --out "$STATE_B/diff" > "$STATE_B/diff-run.txt" 2>&1
GATE_B=$?
set -e
cat "$STATE_B/diff-run.txt"
trace "visual diff state-b exit=$GATE_B (0=all pass / 1=zone FAIL known-gap / 2=invalid input)"
[[ "$GATE_B" != 2 ]] || { cat "$STATE_B/diff-run.txt" >&2; die "ui-visual-diff invalid input (state-b, exit 2)"; }

trace "State B step 2: open Header Activity popover (r6-state-b-open)"
ax_press inspector-toggle "$STATE_B/action-press-inspector-toggle.txt" "open Activity popover"
wait_tree_state 'identifier="activity-popover"' present "$PHASE_TIMEOUT_SECS" activity-popover-open \
  || { echo "ui-r6a-states: activity-popover did not open (evidence kept: $OUT)" >&2; exit 4; }
ASSERT_B_OPEN_OK=0
require_phase r6-state-b-open "$STATE_B/ax-tree-open.txt" "$STATE_B/geometry-open.txt" \
  "$STATE_B/assert-r6-state-b-open.json" || ASSERT_B_OPEN_OK=1
archive_shot "$STATE_B/shot-activity-popover.png" "activity popover evidence"

trace "State B step 3: summary click resumes Inspector on Changes (r6-state-b-resumed)"
ax_press activity-open-changes "$STATE_B/action-press-activity-open-changes.txt" "resume inspector via popover"
wait_tree_state 'identifier="inspector"' present "$PHASE_TIMEOUT_SECS" inspector-resumed \
  || { echo "ui-r6a-states: inspector did not resume (evidence kept: $OUT)" >&2; exit 4; }
ASSERT_B_RESUMED_OK=0
require_phase r6-state-b-resumed "$STATE_B/ax-tree-resumed.txt" "$STATE_B/geometry-resumed.txt" \
  "$STATE_B/assert-r6-state-b-resumed.json" || ASSERT_B_RESUMED_OK=1

copy_runtime_evidence
"$PY" "$WAVE_D_TOOLS" shell-manifest --dir "$STATE_A" --repo "$REPO_ROOT" \
  --scenario r6-wave-a-state-a --label "$LABEL"
"$PY" "$WAVE_D_TOOLS" shell-manifest --dir "$STATE_B" --repo "$REPO_ROOT" \
  --scenario r6-wave-a-state-b --label "$LABEL"

trace "teardown fixture root=$ROOT"
"$FIXTURE" down --root "$ROOT"
"$FIXTURE" clean --root "$ROOT"
DESKTOP_PID=""
rm -rf "$WORK"
trace "run done assert_a=$ASSERT_A_OK assert_b_open=$ASSERT_B_OPEN_OK assert_b_resumed=$ASSERT_B_RESUMED_OK gate_a=$GATE_A gate_b=$GATE_B"

if (( ASSERT_A_OK == 0 && ASSERT_B_OPEN_OK == 0 && ASSERT_B_RESUMED_OK == 0 )); then
  exit 0
fi
echo "ui-r6a-states: structural assertion failed (evidence kept: $OUT)" >&2
exit 4
