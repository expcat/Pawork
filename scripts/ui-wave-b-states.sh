#!/usr/bin/env bash
# Pawork R2 Wave B - U2 window/interaction states driver (scripts-only).
#
# Scenario: seed -> serve -> desktop -> wait ready -> empty-state capture
# (workspace-empty-hint assertion + screenshot) -> focus/blur screenshots ->
# resize 1440->1080 (narrow assertion) -> resize back (restored assertion) ->
# AXPress session row -> AXPress inspector-collapse -> AXPress
# inspector-toggle (ActivityPopover) -> State B capture (collapsed assertion +
# normalized screenshot) -> desktop-restart -> resume assertion (session +
# timeline restored, three-column shell back).
#
# State B 只产 shell 证据：不做 zones/current 映射（F-05/F-12 不在本波）。
# 同步只用 barrier/轮询（相位断言本身充当布局收敛轮询），禁固定 sleep。
# 清理只删自建 fixture root 与临时工具目录；失败保留证据并退出非零。
#
# Usage: scripts/ui-wave-b-states.sh run --out <dir> [--label <name>]
#
# Exit codes: 0 all phase assertions pass; 2 usage error; 3 infrastructure
#   failure; 4 structural assertion failure (phase timeout).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
FIXTURE="$SCRIPT_DIR/ui-fixture.sh"
FOCUS="$SCRIPT_DIR/ui-focus-switch.sh"
SESSION_ID="fx-ses-alpha-today"
SESSION_ROW="session-$SESSION_ID"
BARRIER_TIMEOUT_SECS="${PAWORK_UI_BARRIER_TIMEOUT_SECS:-180}"
WINDOW_TIMEOUT_SECS="${PAWORK_UI_WINDOW_TIMEOUT_SECS:-120}"
PHASE_TIMEOUT_SECS="${PAWORK_UI_PHASE_TIMEOUT_SECS:-30}"

die() { echo "ui-wave-b: $*" >&2; exit 3; }
usage() {
  echo "usage: scripts/ui-wave-b-states.sh run --out <dir> [--label <name>]" >&2
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
LABEL="wave-b-states"
while (( $# )); do
  case "$1" in
    run) MODE="$1"; shift ;;
    --out) OUT="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "ui-wave-b: unknown argument $1" >&2; usage ;;
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
[[ -x "$FOCUS" ]] || die "missing executable $FOCUS"

mkdir -p "$OUT/logs" "$OUT/barriers"
OUT="$(cd "$OUT" && pwd)"
TRACE="$OUT/action-trace.txt"
: > "$TRACE"
trace() {
  echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" | tee -a "$TRACE" >&2
}

WORK="$(mktemp -d /tmp/pawork-wave-b-tools.XXXXXX)"
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

barrier_seq() {
  "$PY" "$PY_TOOLS" barrier-read --file "$ROOT/barriers/timeline_stable" --field seq 2>/dev/null || true
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
    echo "ui-wave-b: structural assertion failed phase=$1 (evidence kept: $OUT)" >&2
    exit 4
  }
}

shot_with_wid() { # $1=wid-file $2=outfile $3=label
  local wid
  wid="$(cat "$1" 2>/dev/null || true)"
  [[ "$wid" =~ ^[0-9]+$ ]] || die "missing wid for capture $3"
  screencapture -x -o -l "$wid" "$2" || die "screencapture failed $3 wid=$wid"
  [[ -s "$2" ]] || die "screenshot empty $3 wid=$wid"
}

ax_press() { # $1=identifier $2=evidence-file $3=label
  trace "AXPress $1 ($3)"
  "$AXDUMP" --pid "$DESKTOP_PID" --press "$1" --action-only --out "$2" >/dev/null \
    || die "AXPress $1 failed (see $2)"
  grep -q 'result=0' "$2" || die "AXPress $1 result!=0 (see $2)"
  trace "AXPress $1 result=0"
}

resize_window() { # $1=WxH $2=evidence-file $3=label
  trace "resize -> $1 ($3)"
  "$WORK/ax-frames" "$DESKTOP_PID" --resize "$1" > "$2" \
    || { cat "$2" >&2; die "resize $1 failed ($3, see $2)"; }
  grep -q '# resize result=0 ' "$2" \
    || die "resize $1 did not converge ($3, see $2)"
}

wait_desktop_ready

trace "place Desktop window deterministically on the main display"
"$WORK/ax-frames" "$DESKTOP_PID" --place-main > "$OUT/window-place.txt" \
  || die "failed to place Desktop window (see $OUT/window-place.txt)"
grep -q '# place-main result=0 ' "$OUT/window-place.txt" \
  || die "Desktop main-display placement did not converge (see $OUT/window-place.txt)"

INITIAL_SEQ="$(wait_timeline_stable 0 "" 0 "$BARRIER_TIMEOUT_SECS")"

trace "phase empty: workspace-empty-hint + screenshot"
require_phase "empty"
shot_with_wid "$WORK/wid-empty.txt" "$WORK/empty-raw.png" "empty"
"$PY" "$PY_TOOLS" normalize --shot "$WORK/empty-raw.png" \
  --tree "$OUT/ax-tree-empty.txt" --wid "$WORK/wid-empty.txt" \
  --frames "$OUT/geometry-empty.txt" \
  --out "$OUT/empty-state.png" --json "$OUT/normalize-empty.json" >/dev/null
trace "empty-state normalized -> $OUT/empty-state.png"

trace "focus/blur evidence"
"$FOCUS" activate --pid "$DESKTOP_PID" > "$OUT/logs/focus-activate.txt" \
  || { cat "$OUT/logs/focus-activate.txt" >&2; die "focus activate failed"; }
shot_with_wid "$WORK/wid-empty.txt" "$OUT/focus-active.png" "focus"
"$FOCUS" deactivate --pid "$DESKTOP_PID" > "$OUT/logs/focus-deactivate.txt" \
  || { cat "$OUT/logs/focus-deactivate.txt" >&2; die "focus deactivate (Finder) failed"; }
shot_with_wid "$WORK/wid-empty.txt" "$OUT/focus-blurred.png" "blur"
"$FOCUS" activate --pid "$DESKTOP_PID" > "$OUT/logs/focus-reactivate.txt" \
  || { cat "$OUT/logs/focus-reactivate.txt" >&2; die "focus reactivate failed"; }
trace "focus/blur captured (focused + blurred + reactivated)"

trace "resize 1440 -> 1080 (narrow)"
resize_window "1080x1024" "$OUT/resize-narrow.txt" "narrow"
require_phase "narrow"
shot_with_wid "$WORK/wid-narrow.txt" "$OUT/narrow-1080.png" "narrow"

trace "resize 1080 -> 1440 (restored)"
resize_window "1440x1024" "$OUT/resize-restore.txt" "restore"
require_phase "restored"

PRE_PRESS_SEQ="$(barrier_seq)"
[[ "$PRE_PRESS_SEQ" =~ ^[0-9]+$ ]] || PRE_PRESS_SEQ="$INITIAL_SEQ"
ax_press "$SESSION_ROW" "$OUT/action-press-session.txt" "open session"
wait_timeline_stable "$PRE_PRESS_SEQ" "$SESSION_ID" 1 "$BARRIER_TIMEOUT_SECS" >/dev/null

ax_press "inspector-collapse" "$OUT/action-inspector-collapse.txt" "collapse inspector"
ax_press "inspector-toggle" "$OUT/action-activity-popover.txt" "open ActivityPopover"
trace "phase collapsed (State B): inspector absent + popover open + screenshot"
require_phase "collapsed"
shot_with_wid "$WORK/wid-collapsed.txt" "$WORK/stateb-raw.png" "state-b"
"$PY" "$PY_TOOLS" normalize --shot "$WORK/stateb-raw.png" \
  --tree "$OUT/ax-tree-collapsed.txt" --wid "$WORK/wid-collapsed.txt" \
  --frames "$OUT/geometry-collapsed.txt" \
  --out "$OUT/state-b.png" --json "$OUT/normalize-state-b.json" >/dev/null
trace "State B normalized -> $OUT/state-b.png (no zones/current mapping this wave)"

PRE_RESTART_SEQ="$(barrier_seq)"
[[ "$PRE_RESTART_SEQ" =~ ^[0-9]+$ ]] || PRE_RESTART_SEQ="$PRE_PRESS_SEQ"
trace "desktop-restart (host/data/barriers preserved); pre_restart_seq=$PRE_RESTART_SEQ"
"$FIXTURE" desktop-restart --root "$ROOT"
DESKTOP_PID="$(cat "$ROOT/desktop.pid")"
trace "desktop restarted pid=$DESKTOP_PID"

wait_desktop_ready
"$WORK/ax-frames" "$DESKTOP_PID" --place-main > "$OUT/restart-window-place.txt" \
  || die "failed to place restarted Desktop window (see $OUT/restart-window-place.txt)"
grep -q '# place-main result=0 ' "$OUT/restart-window-place.txt" \
  || die "restarted Desktop placement did not converge (see $OUT/restart-window-place.txt)"

# 新进程不保留 active session（apply_fresh_snapshot 只合并 snapshot，
# active_session_id 是进程内状态）；start_connect 已删除旧 barrier。
# 先等新一轮任意 barrier 取得新进程 settle_seq 基线，再 AXPress 重开会话，
# 验证 host 侧持久化（会话列表/时间线可恢复）——这是重开后的诚实 resume 语义。
RESTART_BASE_SEQ="$(wait_timeline_stable 0 "" 0 "$BARRIER_TIMEOUT_SECS")"
trace "restart baseline barrier seq=$RESTART_BASE_SEQ (no active session in fresh process)"
ax_press "$SESSION_ROW" "$OUT/action-press-session-resumed.txt" "re-open session after restart"
wait_timeline_stable "$RESTART_BASE_SEQ" "$SESSION_ID" 1 "$BARRIER_TIMEOUT_SECS" >/dev/null
trace "phase resumed: session re-opened from host after desktop-restart"
require_phase "resumed"

copy_runtime_evidence
"$PY" "$PY_TOOLS" shell-manifest --dir "$OUT" --repo "$REPO_ROOT" \
  --scenario wave-b-states --label "$LABEL"

trace "teardown fixture root=$ROOT"
"$FIXTURE" down --root "$ROOT"
"$FIXTURE" clean --root "$ROOT"
DESKTOP_PID=""
rm -rf "$WORK"
trace "run done (all phase assertions passed)"
exit 0
