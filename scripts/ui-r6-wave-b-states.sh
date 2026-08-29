#!/usr/bin/env bash
# Pawork R6 Wave B U1/U2 real-interface driver.
#
# The isolated fixture exposes explicit dev-only Host profiles for successful
# terminal I/O, deterministic MCP status rows, and read_only terminal denial.
# It never fabricates a production wire action: the frozen protocol still has
# no terminal stop/close operation, and the assertions require those controls
# to remain absent.
# Synchronization is barrier/AX assertion polling.  The short polling interval
# is not a readiness delay; there are no fixed sleeps.  AX recursion uses the
# R6A AXWindows signature and at most three desktop restarts.
#
# Usage: scripts/ui-r6-wave-b-states.sh run --out <new-or-empty-dir>
#        [--label r6-wave-b-states|c1|c2|c3|t1|t2|i1|s1|d1|r1]
# Exit: 0 requested scenes passed; 2 usage; 3 infrastructure; 4 contract failure.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
FIXTURE="$SCRIPT_DIR/ui-fixture.sh"
TOOLS="$SCRIPT_DIR/test_ui_r6_wave_b_states.py"
WAVE_D="$SCRIPT_DIR/ui-wave-d-tools.py"
WINDOW_TIMEOUT_SECS="${PAWORK_UI_WINDOW_TIMEOUT_SECS:-120}"
PHASE_TIMEOUT_SECS="${PAWORK_UI_PHASE_TIMEOUT_SECS:-30}"
BARRIER_TIMEOUT_SECS="${PAWORK_UI_BARRIER_TIMEOUT_SECS:-180}"
RUN_TIMEOUT_SECS="${PAWORK_UI_RUN_TIMEOUT_SECS:-900}"
SESSION_A="fx-ses-alpha-today"
SESSION_OLD="fx-ses-alpha-yesterday"

die() { echo "ui-r6b-states: $*" >&2; exit 3; }
usage() {
  echo "usage: scripts/ui-r6-wave-b-states.sh run --out <dir> [--label r6-wave-b-states|c1|c2|c3|t1|t2|i1|s1|d1|r1]" >&2
  exit 2
}

pick_python() {
  local candidate chosen
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

MODE=""; OUT=""; LABEL="r6-wave-b-states"
while (( $# )); do
  case "$1" in
    run) MODE=run; shift ;;
    --out) (( $# >= 2 )) || usage; OUT="$2"; shift 2 ;;
    --label) (( $# >= 2 )) || usage; LABEL="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done
[[ "$MODE" == run && -n "$OUT" ]] || usage
case "$LABEL" in r6-wave-b-states|c1|c2|c3|t1|t2|i1|s1|d1|r1) ;; *) usage ;; esac
pick_python
if [[ -e "$OUT" && ! -d "$OUT" ]]; then die "--out must name a new or empty directory: $OUT"; fi
if [[ -d "$OUT" ]]; then
  shopt -s nullglob dotglob; entries=("$OUT"/*); shopt -u nullglob dotglob
  (( ${#entries[@]} == 0 )) || die "--out must be new or empty to prevent stale evidence: $OUT"
fi
[[ -x "$FIXTURE" && -f "$TOOLS" && -f "$WAVE_D" ]] || die "missing fixture/assertion tools"

mkdir -p "$OUT/logs" "$OUT/barriers"
OUT="$(cd "$OUT" && pwd -P)"
TRACE="$OUT/action-trace.txt"; : > "$TRACE"
trace() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" | tee -a "$TRACE" >&2; }

WORK="$(mktemp -d /tmp/pawork-r6b-states-tools.XXXXXX)"
ROOT="$(mktemp -d /tmp/pawork-ui-fixture.XXXXXX)"; ROOT="$(cd "$ROOT" && pwd -P)"
DESKTOP_PID=""; RUN_WATCHDOG=""
copy_evidence() {
  cp "$ROOT/logs/serve.log" "$OUT/logs/serve.log" 2>/dev/null || true
  cp "$ROOT/logs/desktop.log" "$OUT/logs/desktop.log" 2>/dev/null || true
  for file in "$ROOT"/barriers/*; do [[ -f "$file" ]] && cp "$file" "$OUT/barriers/" 2>/dev/null || true; done
}
teardown() {
  local status=$?; trap - EXIT
  kill "${RUN_WATCHDOG:-}" 2>/dev/null || true
  copy_evidence
  [[ -f "$ROOT/.pawork-ui-fixture" ]] && "$FIXTURE" down --root "$ROOT" >/dev/null 2>&1 || true
  [[ -f "$ROOT/.pawork-ui-fixture" ]] && "$FIXTURE" clean --root "$ROOT" >/dev/null 2>&1 || true
  rm -rf "$WORK"
  exit "$status"
}
trap teardown EXIT
(
  sleep "$RUN_TIMEOUT_SECS"
  kill -TERM "$$" 2>/dev/null || true
) &
RUN_WATCHDOG=$!
trap 'kill "$RUN_WATCHDOG" 2>/dev/null || true; die "run watchdog timeout"' TERM

trace "compile AX/key helpers"
swiftc -O -o "$WORK/ui-ax-dump" "$SCRIPT_DIR/ui-ax-dump.swift" || die "ui-ax-dump compile failed"
swiftc -O -o "$WORK/ui-key-event" "$SCRIPT_DIR/ui-key-event.swift" || die "ui-key-event compile failed"
AXDUMP="$WORK/ui-ax-dump"; KEY="$WORK/ui-key-event"

trace "seed/serve/desktop isolated fixture"
"$FIXTURE" seed --root "$ROOT"
"$FIXTURE" serve --root "$ROOT" --profile r6-terminal
"$FIXTURE" desktop --root "$ROOT"
DESKTOP_PID="$(cat "$ROOT/desktop.pid")"
AX_RESTARTS=0
AX_RECOVERY_ALLOWED=1

# 单个外部动作必须有自身 deadline：AX/CGEvent 子进程挂死不得绕过 phase
# timeout 直到全局 watchdog。超时杀整个进程组，返回 124 交给相位重试/失败。
run_bounded() {
  local timeout="$1"; shift
  "$PY" - "$timeout" "$@" <<'PY'
import os
import signal
import subprocess
import sys

timeout = float(sys.argv[1])
argv = sys.argv[2:]
proc = subprocess.Popen(argv, start_new_session=True)
try:
    raise SystemExit(proc.wait(timeout=timeout))
except subprocess.TimeoutExpired:
    os.killpg(proc.pid, signal.SIGKILL)
    proc.wait()
    raise SystemExit(124)
PY
}

action_failed() { echo "ui-r6b-states: $*" >&2; exit 4; }

alive() { kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited ($1)"; }
is_ax_recursion() {
  local file="$1" count
  count="$(grep -c 'role=AXApplication' "$file" 2>/dev/null || true)"
  { [[ "$count" =~ ^[0-9]+$ ]] && (( count >= 3 )) && grep -q '# identifiers (none)' "$file"; } \
    || { grep -q '^# WARN ax-fallback=axwindows' "$file" && ! grep -q 'identifier="session-list"' "$file"; }
}
wait_ready() {
  local deadline=$((SECONDS + WINDOW_TIMEOUT_SECS)) attempts=0 rc
  while :; do
    set +e
    run_bounded "$PHASE_TIMEOUT_SECS" "$AXDUMP" --pid "$DESKTOP_PID" --out "$WORK/probe.txt" >/dev/null 2>&1
    rc=$?
    set -e
    if (( rc == 0 )) && grep -q 'identifier="session-list"' "$WORK/probe.txt"; then return 0; fi
    attempts=$((attempts + 1))
    if (( attempts >= 30 )) && is_ax_recursion "$WORK/probe.txt"; then
      (( AX_RESTARTS < 3 )) || { cp "$WORK/probe.txt" "$OUT/ax-recursion-final.txt"; die "AX recursion after 3 desktop restarts"; }
      AX_RESTARTS=$((AX_RESTARTS + 1)); trace "AX recursion: desktop-restart $AX_RESTARTS/3"
      "$FIXTURE" desktop-restart --root "$ROOT"; DESKTOP_PID="$(cat "$ROOT/desktop.pid")"; attempts=0
    fi
    alive "AX ready"; (( SECONDS < deadline )) || die "timeout waiting desktop AX tree"
    sleep 0.2
  done
}
recover_ax_for_keyboard_focus() {
  local file="$1"
  [[ "$AX_RECOVERY_ALLOWED" == 1 ]] || return 1
  is_ax_recursion "$file" || return 1
  (( AX_RESTARTS < 3 )) || { cp "$file" "$OUT/ax-recursion-final.txt"; die "AX recursion after 3 desktop restarts"; }
  AX_RESTARTS=$((AX_RESTARTS + 1)); trace "AX recursion during focus: desktop-restart $AX_RESTARTS/3"
  "$FIXTURE" desktop-restart --root "$ROOT"
  DESKTOP_PID="$(cat "$ROOT/desktop.pid")"
  wait_ready
  press "session-$SESSION_A" ax-recovery-select-alpha
  wait_timeline "$SESSION_A"
}
barrier_field() { "$PY" "$WAVE_D" barrier-read --file "$1" --field "$2" 2>/dev/null || true; }
wait_timeline() {
  local wanted="$1" deadline=$((SECONDS + BARRIER_TIMEOUT_SECS)) session
  while :; do
    session="$(barrier_field "$ROOT/barriers/timeline_stable" session)"
    [[ "$session" == "$wanted" ]] && return 0
    alive "timeline_stable"; (( SECONDS < deadline )) || die "timeline_stable timeout for $wanted"
    sleep 0.2
  done
}
wait_timeline_after() { # $1=min_seq(exclusive) $2=want_session(""=any) $3=min_entries
  local min_seq="$1" wanted="$2" min_entries="$3"
  local deadline=$((SECONDS + BARRIER_TIMEOUT_SECS)) seq session entries
  while :; do
    seq="$(barrier_field "$ROOT/barriers/timeline_stable" seq)"
    session="$(barrier_field "$ROOT/barriers/timeline_stable" session)"
    entries="$(barrier_field "$ROOT/barriers/timeline_stable" entries)"
    if [[ "$seq" =~ ^[0-9]+$ ]] && (( seq > min_seq )) \
        && [[ "$entries" =~ ^[0-9]+$ ]] && (( entries >= min_entries )) \
        && { [[ -z "$wanted" ]] || [[ "$session" == "$wanted" ]]; }; then
      trace "timeline_stable seq=$seq session=$session entries=$entries"
      printf '%s' "$seq"
      return 0
    fi
    alive "timeline_stable"; (( SECONDS < deadline )) || die "timeline_stable timeout after seq=$min_seq"
    sleep 0.2
  done
}
wait_phase() {
  local phase="$1" deadline=$((SECONDS + PHASE_TIMEOUT_SECS)) rc=0 tree json
  tree="$OUT/ax-tree-$phase.txt"
  json="$OUT/assert-$phase.json"
  trace "assert phase=$phase"
  while :; do
    set +e
    run_bounded "$PHASE_TIMEOUT_SECS" "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 18 --out "$tree" >/dev/null 2>&1
    rc=$?
    (( rc == 0 )) && run_bounded "$PHASE_TIMEOUT_SECS" python3 "$TOOLS" assert --tree "$tree" --phase "$phase" --out "$json" >/dev/null 2>&1
    rc=$?
    set -e
    (( rc == 0 )) && return 0
    alive "$phase"; (( SECONDS < deadline )) || { trace "phase=$phase timeout rc=$rc"; exit 4; }
    sleep 0.2
  done
}
press() { run_bounded "$PHASE_TIMEOUT_SECS" "$AXDUMP" --pid "$DESKTOP_PID" --press "$1" --action-only --out "$OUT/action-$2.txt" >/dev/null || action_failed "AXPress $1"; }
focus() { run_bounded "$PHASE_TIMEOUT_SECS" "$AXDUMP" --pid "$DESKTOP_PID" --focus "$1" --action-only --out "$OUT/action-focus-$2.txt" >/dev/null || action_failed "AXFocus $1"; }
key() {
  run_bounded "$PHASE_TIMEOUT_SECS" "$SCRIPT_DIR/ui-focus-switch.sh" activate --pid "$DESKTOP_PID" >/dev/null 2>&1 || true
  run_bounded "$PHASE_TIMEOUT_SECS" "$KEY" --pid "$DESKTOP_PID" --key "$1" >/dev/null || action_failed "key $1"
}
set_value() { run_bounded "$PHASE_TIMEOUT_SECS" "$AXDUMP" --pid "$DESKTOP_PID" --set-value "$1" "$2" --action-only --out "$OUT/action-set-$3.txt" >/dev/null || action_failed "AXSetValue $1"; }
screenshot() { # $1=phase
  local phase="$1" wid
  run_bounded "$PHASE_TIMEOUT_SECS" "$AXDUMP" --pid "$DESKTOP_PID" \
    --out "$WORK/wid-$phase.txt" --wid-out "$WORK/wid-$phase.id" >/dev/null 2>&1 \
    || die "AX dump for screenshot failed (phase $phase)"
  wid="$(cat "$WORK/wid-$phase.id")"
  [[ "$wid" =~ ^[0-9]+$ ]] || die "missing window id (phase $phase)"
  run_bounded "$PHASE_TIMEOUT_SECS" screencapture -x -o -l "$wid" "$OUT/shot-$phase.png" \
    || die "screencapture failed (phase $phase, wid=$wid)"
  [[ -s "$OUT/shot-$phase.png" ]] || die "screenshot empty (phase $phase, wid=$wid)"
  trace "screenshot $OUT/shot-$phase.png"
}
# 键盘路径验收必须让 GPUI FocusHandle 真正获焦；AXFocus 只设辅助技术焦点，
# 不一定驱动应用内 keymap（本波 attempt-3 已实证 focused 仍为 0）。
focus_by_keys() {
  local target="$1" label="$2" attempt=0 rc
  while (( attempt < 60 )); do
    set +e
    run_bounded "$PHASE_TIMEOUT_SECS" "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 18 --out "$WORK/focus-check.txt" >/dev/null 2>&1
    rc=$?
    set -e
    if (( rc == 0 )) && is_ax_recursion "$WORK/focus-check.txt" && recover_ax_for_keyboard_focus "$WORK/focus-check.txt"; then
      attempt=0
      continue
    fi
    if (( rc == 0 )) && grep -F "identifier=\"$target\"" "$WORK/focus-check.txt" | grep -q 'focused=1'; then
      cp "$WORK/focus-check.txt" "$OUT/action-key-focus-$label.txt"
      return 0
    fi
    key tab
    attempt=$((attempt + 1))
    sleep 0.2
  done
  cp "$WORK/focus-check.txt" "$OUT/action-key-focus-$label-timeout.txt" 2>/dev/null || true
  action_failed "keyboard focus $target"
}
want() { [[ "$LABEL" == r6-wave-b-states || "$LABEL" == "$1" ]]; }
switch_host_profile() { # $1=profile $2=evidence-label
  local profile="$1" label="$2"
  trace "restart fixture Host profile=$profile"
  AX_RECOVERY_ALLOWED=0
  "$FIXTURE" restart-host --root "$ROOT" --profile "$profile"
  wait_phase profile-disconnected
  screenshot "$label-disconnected"
  press reconnect "$label-reconnect"
  AX_RECOVERY_ALLOWED=1
}

wait_ready
press "session-$SESSION_A" select-alpha
wait_timeline "$SESSION_A"

if want c1; then
  trace "C1 canonical Changes files"
  wait_phase c1-files
  screenshot c1-files
fi
if want c3; then
  trace "C3 keyboard secondary tabs and file rows"
  focus_by_keys changes-tab-files c3-files; key right; wait_phase c3-summary
  key left
  focus_by_keys 'changes-file-src_2fmain.rs' c3-first-file; key down; key enter
  wait_phase c3-file-focus
  screenshot c3-file-focus
fi
if want t1 || want d1; then
  trace "T1 terminal create/write/output"
  focus_by_keys inspector-tab-changes terminal-from-changes; key right; wait_phase t1-idle
  focus_by_keys terminal-start terminal-start; key enter
  wait_phase t1-ready
  focus_by_keys terminal-resize terminal-resize; key enter
  wait_phase t1-resized
  set_value terminal-input 'printf pawork-r6b-t1' terminal-input
  focus_by_keys terminal-input terminal-input; key return
  wait_phase t1-output
  screenshot t1-output
fi
if want i1; then
  trace "I1 keyboard collapse and Activity restore"
  press inspector-tab-changes i1-changes
  focus_by_keys inspector-collapse i1-collapse; key enter; wait_phase i1-collapsed
  key enter
  press activity-open-changes i1-open-changes
  wait_phase i1-restored
  screenshot i1-restored
fi
if want s1; then
  trace "S1 latest-session scope remains distinct from active old session"
  press "session-$SESSION_OLD" s1-old-session
  wait_timeline "$SESSION_OLD"
  press inspector-tab-changes s1-changes
  wait_phase s1-latest-scope
  screenshot s1-latest-scope
fi
if want d1; then
  trace "D1 disconnect/reconnect terminal state"
  AX_RECOVERY_ALLOWED=0
  press inspector-tab-terminal d1-terminal
  "$FIXTURE" drop-socket --root "$ROOT" >/dev/null
  set_value terminal-input 'pawork-r6b-d1' terminal-disconnected-input
  focus_by_keys terminal-input terminal-disconnected-input; key return
  wait_phase d1-disconnected
  screenshot d1-disconnected
  press reconnect d1-reconnect
  wait_phase d1-reconnected
  screenshot d1-reconnected
  AX_RECOVERY_ALLOWED=1
fi

if want c2; then
  trace "C2 new clean beta task returns ready empty Changes"
  C2_BASE_SEQ="$(barrier_field "$ROOT/barriers/timeline_stable" seq)"
  # D1 reconnect may truthfully clear the optional UI-only stability barrier;
  # the subsequent SessionCreate must publish a fresh positive sequence.
  [[ "$C2_BASE_SEQ" =~ ^[0-9]+$ ]] || C2_BASE_SEQ=0
  press project-add-Earlier_3afx-beta-lib c2-new-beta-task
  wait_timeline_after "$C2_BASE_SEQ" "" 0 >/dev/null
  press inspector-tab-changes c2-changes
  wait_phase c2-empty
  screenshot c2-empty
fi

if want r1; then
  trace "R1 empty Resources then deterministic connected/failed matrix"
  press inspector-tab-resources r1-resources-empty
  wait_phase r1-empty
  focus_by_keys resources-refresh r1-refresh-empty; key enter
  wait_phase r1-empty
  screenshot r1-empty
  switch_host_profile r6-resources r1-profile
  wait_phase r1-matrix
  focus_by_keys resources-refresh r1-refresh-matrix; key enter
  wait_phase r1-matrix
  screenshot r1-matrix
fi

if want t2; then
  trace "T2 read_only profile rejects terminal creation fail-closed"
  switch_host_profile r6-read-only t2-profile
  press inspector-tab-terminal t2-terminal
  wait_phase t1-idle
  focus_by_keys terminal-start t2-start; key enter
  wait_phase t2-denied
  screenshot t2-denied
fi

python3 "$TOOLS" matrix > "$OUT/scenario-matrix.json"
copy_evidence
trace "run done; all requested real-interface scenes passed"
"$FIXTURE" down --root "$ROOT"; "$FIXTURE" clean --root "$ROOT"; DESKTOP_PID=""
rm -rf "$WORK"
exit 0
