#!/usr/bin/env bash
# Pawork R1 Wave D - State A U0-U3 closed loop driver.
#
# Scenario: seed -> serve -> desktop (isolated PAWORK_DATA_DIR +
# PAWORK_UI_BARRIER_DIR via ui-fixture) -> wait timeline_stable ->
# AX three-column skeleton assertions -> AXPress session row ->
# wait new timeline_stable round -> assert selected + timeline loaded ->
# window screenshot -> normalize current.png -> ui-visual-diff -> evidence.
#
# Sync uses barriers/polling only (no fixed sleeps for readiness).
# Cleans only fixture roots and tool dirs it created itself. On failure the
# evidence directory is preserved and the script exits non-zero.
#
# Usage:
#   scripts/ui-wave-d-state-a.sh run --out <dir> [--label <name>] [--write-zones <path>]
#   scripts/ui-wave-d-state-a.sh compare --a <dir> --b <dir> --report <file>
#
# Exit codes: 0 structural pass (visual zone FAIL is expected in R1);
#   2 usage error; 3 infrastructure failure; 4 structural assertion failure;
#   6 repeatability compare mismatch.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
FIXTURE="$SCRIPT_DIR/ui-fixture.sh"
SESSION_ID="fx-ses-alpha-today"
SESSION_ROW="session-$SESSION_ID"
BARRIER_TIMEOUT_SECS="$(printenv PAWORK_UI_BARRIER_TIMEOUT_SECS || true)"
[[ -n "$BARRIER_TIMEOUT_SECS" ]] || BARRIER_TIMEOUT_SECS=180
WINDOW_TIMEOUT_SECS="$(printenv PAWORK_UI_WINDOW_TIMEOUT_SECS || true)"
[[ -n "$WINDOW_TIMEOUT_SECS" ]] || WINDOW_TIMEOUT_SECS=120

die() { echo "ui-wave-d: $*" >&2; exit 3; }
usage() {
  echo "usage: scripts/ui-wave-d-state-a.sh run --out <dir> [--label <name>] [--write-zones <path>]" >&2
  echo "       scripts/ui-wave-d-state-a.sh compare --a <dir> --b <dir> --report <file>" >&2
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
LABEL="wave-d-run"
RUN_A=""
RUN_B=""
REPORT=""
WRITE_ZONES_OUT=""
while (( $# )); do
  case "$1" in
    run|compare) MODE="$1"; shift ;;
    --out) OUT="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    --a) RUN_A="$2"; shift 2 ;;
    --b) RUN_B="$2"; shift 2 ;;
    --report) REPORT="$2"; shift 2 ;;
    --write-zones) WRITE_ZONES_OUT="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "ui-wave-d: unknown argument $1" >&2; usage ;;
  esac
done
[[ -n "$MODE" ]] || usage

pick_python
PY_TOOLS="$SCRIPT_DIR/ui-wave-d-tools.py"
[[ -f "$PY_TOOLS" ]] || die "missing $PY_TOOLS"

if [[ "$MODE" == "compare" ]]; then
  [[ -n "$RUN_A" && -n "$RUN_B" && -n "$REPORT" ]] || usage
  "$PY" "$PY_TOOLS" compare --a "$RUN_A" --b "$RUN_B" --report "$REPORT"
  exit $?
fi

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
mkdir -p "$OUT/logs" "$OUT/barriers"
OUT="$(cd "$OUT" && pwd)"
TRACE="$OUT/action-trace.txt"
: > "$TRACE"
trace() {
  echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" | tee -a "$TRACE" >&2
}

WORK="$(mktemp -d /tmp/pawork-wave-d-tools.XXXXXX)"
ROOT="$(mktemp -d /tmp/pawork-ui-fixture.XXXXXX)"
ROOT="$(cd "$ROOT" && pwd -P)"
DESKTOP_PID=""
fixture_teardown() {
  local status=$?
  trap - EXIT
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

trace "seed fixture root=$ROOT"
"$FIXTURE" seed --root "$ROOT"
trace "serve fixture (wait host_ready barrier)"
"$FIXTURE" serve --root "$ROOT"

mkdir -p "$ROOT/logs" "$ROOT/barriers"
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

AXDUMP="$WORK/ui-ax-dump"
deadline=$(( SECONDS + WINDOW_TIMEOUT_SECS ))
probe_attempts=0
ax_restarts=0
while :; do
  set +e
  "$AXDUMP" --pid "$DESKTOP_PID" --out "$WORK/probe.txt" >/dev/null 2>&1
  probe_rc=$?
  set -e
  if (( probe_rc == 0 )) && grep -q 'identifier="session-list"' "$WORK/probe.txt"; then
    trace "desktop window ready (session-list rendered)"
    break
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
  kill -0 "$DESKTOP_PID" 2>/dev/null \
    || { tail -5 "$ROOT/logs/desktop.log" >&2 || true; die "desktop exited early"; }
  if (( SECONDS >= deadline )); then
    cp "$WORK/probe.txt" "$OUT/ax-tree-probe-timeout.txt" 2>/dev/null || true
    cp "$ROOT/logs/desktop.log" "$OUT/logs/desktop.log" 2>/dev/null || true
    cp "$ROOT/logs/serve.log" "$OUT/logs/serve.log" 2>/dev/null || true
    tail -30 "$OUT/ax-tree-probe-timeout.txt" >&2 || true
    tail -10 "$OUT/logs/serve.log" >&2 || true
    die "timeout waiting for desktop window/session-list (evidence in $OUT)"
  fi
  sleep 0.3
done

trace "place Desktop window deterministically on the main display"
"$WORK/ax-frames" "$DESKTOP_PID" --place-main > "$OUT/window-place.txt" \
  || die "failed to place Desktop window on main display (see $OUT/window-place.txt)"
grep -q '# place-main result=0 ' "$OUT/window-place.txt" \
  || die "Desktop main-display placement did not converge (see $OUT/window-place.txt)"

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

INITIAL_SEQ="$(wait_timeline_stable 0 "" 0 "$BARRIER_TIMEOUT_SECS")"

trace "initial AX dump + frame probe"
"$AXDUMP" --pid "$DESKTOP_PID" --out "$OUT/ax-tree-initial.txt" >/dev/null
"$WORK/ax-frames" "$DESKTOP_PID" > "$OUT/geometry-initial.txt"
INITIAL_ASSERT_OK=0
"$PY" "$PY_TOOLS" assert --frames "$OUT/geometry-initial.txt" \
  --tree "$OUT/ax-tree-initial.txt" --phase initial --out "$OUT/assert-initial.json" \
  || INITIAL_ASSERT_OK=1

trace "AXPress $SESSION_ROW"
"$AXDUMP" --pid "$DESKTOP_PID" --press "$SESSION_ROW" --action-only \
  --out "$OUT/action-press-session.txt" >/dev/null \
  || die "AXPress $SESSION_ROW failed (see $OUT/action-press-session.txt)"
grep -q 'result=0' "$OUT/action-press-session.txt" \
  || die "AXPress $SESSION_ROW result!=0 (see $OUT/action-press-session.txt)"
trace "AXPress result=0"

wait_timeline_stable "$INITIAL_SEQ" "$SESSION_ID" 1 "$BARRIER_TIMEOUT_SECS" >/dev/null

trace "final AX dump + frame probe"
"$AXDUMP" --pid "$DESKTOP_PID" --out "$OUT/ax-tree.txt" --wid-out "$WORK/wid.txt" >/dev/null
"$WORK/ax-frames" "$DESKTOP_PID" > "$OUT/geometry-final.txt"
FINAL_ASSERT_OK=0
"$PY" "$PY_TOOLS" assert --frames "$OUT/geometry-final.txt" \
  --tree "$OUT/ax-tree.txt" --phase final --out "$OUT/assert-final.json" \
  || FINAL_ASSERT_OK=1

ZONES_FILE="$REPO_ROOT/docs/ui-review/state-a/zones.json"
if [[ -n "${WRITE_ZONES_OUT:-}" ]]; then
  ZONES_FILE="$WORK/zones.json"
  trace "derive current zone rectangles from final AX frames"
  "$PY" "$PY_TOOLS" write-current-zones \
    --zones "$REPO_ROOT/docs/ui-review/state-a/zones.json" \
    --frames "$OUT/geometry-final.txt" --out "$ZONES_FILE"
fi

WID="$(cat "$WORK/wid.txt")"
trace "screencapture wid=$WID"
screencapture -x -o -l "$WID" "$WORK/shot-raw.png" || die "screencapture failed wid=$WID"
[[ -s "$WORK/shot-raw.png" ]] || die "screenshot empty wid=$WID"
"$PY" "$PY_TOOLS" normalize --shot "$WORK/shot-raw.png" --tree "$OUT/ax-tree.txt" \
  --wid "$WORK/wid.txt" --frames "$OUT/geometry-final.txt" \
  --out "$OUT/current.png" --json "$OUT/normalize.json"

trace "visual diff gate"
set +e
"$PY" "$SCRIPT_DIR/ui-visual-diff.py" \
  --reference "$REPO_ROOT/docs/ui-review/state-a/reference.png" \
  --current "$OUT/current.png" \
  --zones "$ZONES_FILE" \
  --masks "$REPO_ROOT/docs/ui-review/state-a/mask.json" \
  --out "$OUT/diff" > "$OUT/diff-run.txt" 2>&1
GATE_EXIT=$?
set -e
cat "$OUT/diff-run.txt"
trace "visual diff exit=$GATE_EXIT (0=all pass / 1=zone FAIL / 2=invalid input)"
[[ "$GATE_EXIT" != 2 ]] || { cat "$OUT/diff-run.txt" >&2; die "ui-visual-diff invalid input (exit 2)"; }

if [[ -n "${WRITE_ZONES_OUT:-}" ]] \
    && (( INITIAL_ASSERT_OK == 0 && FINAL_ASSERT_OK == 0 )); then
  trace "write validated current zone rectangles -> $WRITE_ZONES_OUT"
  mkdir -p "$(dirname "$WRITE_ZONES_OUT")"
  cp "$ZONES_FILE" "$WRITE_ZONES_OUT"
elif [[ -n "${WRITE_ZONES_OUT:-}" ]]; then
  trace "skip current zone write because structural assertions failed"
fi

cp "$ROOT/logs/serve.log" "$OUT/logs/serve.log" 2>/dev/null || true
cp "$ROOT/logs/desktop.log" "$OUT/logs/desktop.log" 2>/dev/null || true
for barrier_file in "$ROOT"/barriers/*; do
  [[ -f "$barrier_file" ]] && cp "$barrier_file" "$OUT/barriers/"
done

"$PY" "$PY_TOOLS" manifest --dir "$OUT" --repo "$REPO_ROOT" \
  --seed "$REPO_ROOT/fixtures/ui/seed.json" --scenario state-a \
  --label "$LABEL" --gate-exit "$GATE_EXIT"
"$PY" "$PY_TOOLS" checklist --dir "$OUT"

trace "teardown fixture root=$ROOT"
"$FIXTURE" down --root "$ROOT"
"$FIXTURE" clean --root "$ROOT"
DESKTOP_PID=""
rm -rf "$WORK"
trace "run done initial_assert_ok=$INITIAL_ASSERT_OK final_assert_ok=$FINAL_ASSERT_OK gate_exit=$GATE_EXIT"

if (( INITIAL_ASSERT_OK == 0 && FINAL_ASSERT_OK == 0 )); then
  exit 0
fi
echo "ui-wave-d: structural assertion failed (evidence kept: $OUT)" >&2
exit 4
