#!/usr/bin/env bash
# Pawork R7 Wave C — 1080×720 / 1k timeline / resize / preference evidence.
#
# 只派生临时 fixture DB；不改 fixtures/ui/seed.json、GUI wire、Host 或 Policy。
# 同步使用 barrier/AX 轮询，不用固定长等待。失败保留 --out 证据，清理自建
# fixture root 与临时编译目录。
#
# Usage: scripts/ui-r7-wave-c-resilience.sh run --out <new-or-empty-dir>
#        [--label <name>]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
FIXTURE="$SCRIPT_DIR/ui-fixture.sh"
PY_TOOLS="$SCRIPT_DIR/ui-wave-d-tools.py"
R7_TOOLS="$SCRIPT_DIR/ui-r7-wave-c-tools.py"
SESSION_ID="fx-ses-beta-long"
SESSION_ROW="session-$SESSION_ID"
LOGICAL_ROWS=1024
SENTINEL="R7C 千级列表末尾 🐾🧪"
BARRIER_TIMEOUT_SECS="${PAWORK_UI_BARRIER_TIMEOUT_SECS:-180}"
WINDOW_TIMEOUT_SECS="${PAWORK_UI_WINDOW_TIMEOUT_SECS:-120}"
PHASE_TIMEOUT_SECS="${PAWORK_UI_PHASE_TIMEOUT_SECS:-30}"
RESIZE_CYCLES="${PAWORK_UI_RESIZE_CYCLES:-3}"

die() { echo "ui-r7-wave-c: $*" >&2; exit 3; }
usage() {
  echo "usage: scripts/ui-r7-wave-c-resilience.sh run --out <dir> [--label <name>]" >&2
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

MODE=""
OUT=""
LABEL="r7-wave-c-resilience"
while (( $# )); do
  case "$1" in
    run) MODE="$1"; shift ;;
    --out) OUT="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "ui-r7-wave-c: unknown argument $1" >&2; usage ;;
  esac
done
[[ "$MODE" == "run" && -n "$OUT" ]] || usage
[[ "$RESIZE_CYCLES" =~ ^[1-9][0-9]*$ ]] || die "PAWORK_UI_RESIZE_CYCLES must be positive"
if [[ -e "$OUT" && ! -d "$OUT" ]]; then
  die "--out must name a new or empty directory: $OUT"
fi
if [[ -d "$OUT" ]]; then
  shopt -s nullglob dotglob
  out_entries=("$OUT"/*)
  shopt -u nullglob dotglob
  (( ${#out_entries[@]} == 0 )) || die "--out must be new or empty"
fi

pick_python
[[ -x "$FIXTURE" ]] || die "missing executable $FIXTURE"
[[ -f "$PY_TOOLS" && -f "$R7_TOOLS" ]] || die "missing Python helper"
mkdir -p "$OUT/logs" "$OUT/barriers"
OUT="$(cd "$OUT" && pwd -P)"
TRACE="$OUT/action-trace.txt"
SAMPLES="$OUT/performance-samples.tsv"
: > "$TRACE"
: > "$SAMPLES"
trace() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" | tee -a "$TRACE" >&2; }
now_ms() { "$PY" -c 'import time; print(time.time_ns() // 1000000)'; }
record_duration() { # $1=metric $2=start_ms $3=detail
  local end_ms
  end_ms="$(now_ms)"
  printf '%s\t%s\t%s\n' "$1" "$(( end_ms - $2 ))" "$3" >> "$SAMPLES"
}

WORK="$(mktemp -d /tmp/pawork-r7-wave-c-tools.XXXXXX)"
ROOT="$(mktemp -d /tmp/pawork-r7-wave-c-fixture.XXXXXX)"
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
  local status="$1"
  trap - EXIT
  copy_runtime_evidence
  if [[ -f "$ROOT/desktop.pid" || -f "$ROOT/host.pid" ]]; then
    "$FIXTURE" down --root "$ROOT" >/dev/null 2>&1 || true
  fi
  "$FIXTURE" clean --root "$ROOT" >/dev/null 2>&1 || true
  rm -rf "$WORK"
  exit "$status"
}
trap 'fixture_teardown "$?"' EXIT

trace "run start label=$LABEL out=$OUT"
trace "compile AX/frame/input/platform helpers"
swiftc -O -o "$WORK/ui-ax-dump" "$SCRIPT_DIR/ui-ax-dump.swift" \
  2>"$WORK/swiftc-axdump.err" \
  || { cat "$WORK/swiftc-axdump.err" >&2; die "ui-ax-dump compile failed"; }
swiftc -O -o "$WORK/ax-frames" "$SCRIPT_DIR/ui-ax-frames.swift" \
  2>"$WORK/swiftc-frames.err" \
  || { cat "$WORK/swiftc-frames.err" >&2; die "ui-ax-frames compile failed"; }
swiftc -O -o "$WORK/ui-key-event" "$SCRIPT_DIR/ui-key-event.swift" \
  2>"$WORK/swiftc-key.err" \
  || { cat "$WORK/swiftc-key.err" >&2; die "ui-key-event compile failed"; }
swiftc -O -o "$WORK/ui-platform-prefs" "$SCRIPT_DIR/ui-platform-prefs.swift" \
  2>"$WORK/swiftc-prefs.err" \
  || { cat "$WORK/swiftc-prefs.err" >&2; die "ui-platform-prefs compile failed"; }
AXDUMP="$WORK/ui-ax-dump"

"$WORK/ui-platform-prefs" > "$OUT/platform-preferences.json"
trace "platform preferences captured read-only"
trace "seed canonical fixture then derive $LOGICAL_ROWS logical rows in temp DB"
"$FIXTURE" seed --root "$ROOT"
"$PY" "$R7_TOOLS" inflate \
  --db "$ROOT/data/session.db" --session-id "$SESSION_ID" \
  --base-rows 64 --target-rows "$LOGICAL_ROWS" --out "$OUT/dataset.json" \
  || die "temporary 1k dataset derivation failed"
"$FIXTURE" serve --root "$ROOT"

wait_desktop_ready() {
  local deadline=$(( SECONDS + WINDOW_TIMEOUT_SECS )) attempt=0 rc
  while :; do
    attempt=$(( attempt + 1 ))
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --out "$WORK/probe.txt" >/dev/null 2>&1
    rc=$?
    set -e
    if (( rc == 0 )) && grep -q 'identifier="session-list"' "$WORK/probe.txt"; then
      trace "desktop ready attempt=$attempt"
      return 0
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited before ready"
    (( SECONDS < deadline )) || {
      cp "$WORK/probe.txt" "$OUT/ax-tree-probe-timeout.txt" 2>/dev/null || true
      die "timeout waiting Desktop AX tree"
    }
    sleep 0.3
  done
}

wait_timeline_stable() { # min_seq want_session min_entries timeout
  local min_seq="$1" want_session="$2" min_entries="$3" timeout="$4"
  local deadline=$(( SECONDS + timeout )) seq session entries
  while :; do
    seq="$("$PY" "$PY_TOOLS" barrier-read --file "$ROOT/barriers/timeline_stable" --field seq 2>/dev/null || true)"
    session="$("$PY" "$PY_TOOLS" barrier-read --file "$ROOT/barriers/timeline_stable" --field session 2>/dev/null || true)"
    entries="$("$PY" "$PY_TOOLS" barrier-read --file "$ROOT/barriers/timeline_stable" --field entries 2>/dev/null || true)"
    if [[ "$seq" =~ ^[0-9]+$ && "$entries" =~ ^[0-9]+$ ]] \
        && (( seq > min_seq && entries >= min_entries )) \
        && [[ -z "$want_session" || "$session" == "$want_session" ]]; then
      trace "timeline_stable seq=$seq session=$session entries=$entries"
      printf '%s' "$seq"
      return 0
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited waiting timeline"
    (( SECONDS < deadline )) || die "timeout waiting timeline_stable session=$want_session entries=$min_entries"
    sleep 0.2
  done
}
barrier_seq() {
  "$PY" "$PY_TOOLS" barrier-read --file "$ROOT/barriers/timeline_stable" --field seq 2>/dev/null || true
}

wait_phase() { # phase tag
  local phase="$1" tag="$2" stem
  stem="$phase-$tag"
  local deadline=$(( SECONDS + PHASE_TIMEOUT_SECS )) rc=1
  trace "wait phase=$phase tag=$tag"
  while :; do
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 \
      --out "$OUT/ax-tree-$stem.txt" --wid-out "$WORK/wid-$stem.txt" >/dev/null 2>&1
    "$WORK/ax-frames" "$DESKTOP_PID" > "$OUT/geometry-$stem.txt" 2>/dev/null
    "$PY" "$PY_TOOLS" assert --frames "$OUT/geometry-$stem.txt" \
      --tree "$OUT/ax-tree-$stem.txt" --phase "$phase" \
      --out "$OUT/assert-$stem.json" >/dev/null 2>&1
    rc=$?
    set -e
    (( rc == 0 )) && { trace "phase=$phase tag=$tag passed"; return 0; }
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited phase=$phase"
    (( SECONDS < deadline )) || return 1
    sleep 0.2
  done
}
require_phase() {
  wait_phase "$1" "$2" || {
    echo "ui-r7-wave-c: structural phase failed: $1/$2 (evidence kept: $OUT)" >&2
    exit 4
  }
}

wait_thousand() { # tag
  local tag="$1" deadline=$(( SECONDS + PHASE_TIMEOUT_SECS )) rc=1
  trace "wait virtualized-thousand tag=$tag"
  while :; do
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 \
      --out "$OUT/ax-tree-virtualized-thousand-$tag.txt" >/dev/null 2>&1
    "$PY" "$PY_TOOLS" states-assert \
      --tree "$OUT/ax-tree-virtualized-thousand-$tag.txt" \
      --phase virtualized-thousand --logical-entries "$LOGICAL_ROWS" \
      --barrier "$ROOT/barriers/timeline_stable" \
      --out "$OUT/assert-virtualized-thousand-$tag.json" >/dev/null 2>&1
    rc=$?
    set -e
    (( rc == 0 )) && { trace "virtualized-thousand tag=$tag passed"; return 0; }
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited waiting 1k AX slice"
    (( SECONDS < deadline )) || {
      echo "ui-r7-wave-c: 1k virtualization assertion failed (evidence kept: $OUT)" >&2
      exit 4
    }
    sleep 0.2
  done
}

ax_action() { # action identifier [value] evidence
  local action="$1" identifier="$2" value="$3" evidence="$4"
  local args=(--pid "$DESKTOP_PID")
  case "$action" in
    press) args+=(--press "$identifier") ;;
    focus) args+=(--focus "$identifier") ;;
    set) args+=(--set-value "$identifier" "$value") ;;
    *) die "unknown AX action: $action" ;;
  esac
  "$AXDUMP" "${args[@]}" --action-only --out "$evidence" >/dev/null \
    || die "AX $action failed: $identifier"
  grep -q 'result=0' "$evidence" || die "AX $action result!=0: $identifier"
}

resize_window() { # size tag phase metric
  local size="$1" tag="$2" phase="$3" metric="$4" start
  start="$(now_ms)"
  "$WORK/ax-frames" "$DESKTOP_PID" --resize "$size" > "$OUT/resize-$tag.txt" \
    || die "resize failed: $size"
  grep -q '# resize result=0 ' "$OUT/resize-$tag.txt" || die "resize did not converge: $size"
  require_phase "$phase" "$tag"
  record_duration "$metric" "$start" "$tag"
}

shot_phase() { # phase-tag output detail
  local stem="$1" output="$2" detail="$3" wid start
  wid="$(cat "$WORK/wid-$stem.txt" 2>/dev/null || true)"
  [[ "$wid" =~ ^[0-9]+$ ]] || die "missing window id for $stem"
  start="$(now_ms)"
  screencapture -x -o -l "$wid" "$output" || die "screenshot failed: $detail"
  [[ -s "$output" ]] || die "empty screenshot: $detail"
  record_duration screenshot "$start" "$detail"
}

scroll_until() { # present|absent delta output
  local expectation="$1" delta="$2" output="$3"
  local deadline=$(( SECONDS + PHASE_TIMEOUT_SECS )) point has_sentinel
  point="$(awk '
    $1 == "id=timeline" {
      for (i=1; i<=NF; i++) { split($i,a,"="); v[a[1]]=a[2] }
      printf "%.0f,%.0f", v["x"] + v["w"]/2, v["y"] + v["h"]/2
    }' "$OUT/geometry-r7c-wide-initial.txt")"
  [[ "$point" == *,* ]] || die "timeline frame center unavailable"
  while :; do
    "$WORK/ui-key-event" --pid "$DESKTOP_PID" --scroll-at "$point" --scroll-y "$delta" \
      >/dev/null 2>>"$OUT/logs/input-events.log" || die "timeline scroll injection failed"
    "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$output" >/dev/null 2>&1 \
      || die "AX dump failed after scroll"
    has_sentinel=0
    grep -Fq "$SENTINEL" "$output" && has_sentinel=1
    if [[ "$expectation" == "present" && "$has_sentinel" == 1 ]] \
        || [[ "$expectation" == "absent" && "$has_sentinel" == 0 ]]; then
      return 0
    fi
    (( SECONDS < deadline )) || die "timeline scroll did not reach $expectation sentinel state"
  done
}

wait_composer_value() { # value output
  local value="$1" output="$2" deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
  while :; do
    "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$output" >/dev/null 2>&1 \
      || die "AX dump failed waiting composer value"
    grep 'identifier="composer-input"' "$output" | grep -Fq "value=\"$value\"" && return 0
    (( SECONDS < deadline )) || die "composer value did not settle"
    sleep 0.1
  done
}

start="$(now_ms)"
"$FIXTURE" desktop --root "$ROOT"
DESKTOP_PID="$(cat "$ROOT/desktop.pid")"
wait_desktop_ready
record_duration desktop_ready "$start" "launch-to-session-list"
"$WORK/ax-frames" "$DESKTOP_PID" --place-main > "$OUT/window-place.txt" \
  || die "main-display placement failed"
grep -q '# place-main result=0 ' "$OUT/window-place.txt" || die "main-display placement did not converge"

INITIAL_SEQ="$(wait_timeline_stable 0 "" 0 "$BARRIER_TIMEOUT_SECS")"
start="$(now_ms)"
ax_action press "$SESSION_ROW" "" "$OUT/action-open-beta-long.txt"
OPEN_SEQ="$(wait_timeline_stable "$INITIAL_SEQ" "$SESSION_ID" "$LOGICAL_ROWS" "$BARRIER_TIMEOUT_SECS")"
record_duration timeline_1024_load "$start" "AXPress-to-timeline_stable"
require_phase r7c-wide initial
wait_thousand initial
shot_phase r7c-wide-initial "$OUT/wide-1440x1024.png" "wide-1k"

start="$(now_ms)"
scroll_until absent 720 "$OUT/ax-tree-scroll-up.txt"
record_duration timeline_scroll_up "$start" "sentinel-visible-to-absent"
start="$(now_ms)"
scroll_until present -1600 "$OUT/ax-tree-scroll-bottom.txt"
record_duration timeline_scroll_bottom "$start" "sentinel-absent-to-visible"
wait_thousand after-scroll

INPUT_VALUE="Wave C 输入响应 🐾"
start="$(now_ms)"
ax_action set composer-input "$INPUT_VALUE" "$OUT/action-set-composer.txt"
wait_composer_value "$INPUT_VALUE" "$OUT/ax-tree-composer-value.txt"
record_duration composer_input "$start" "AXSetValue-to-AXValue"
ax_action set composer-input "" "$OUT/action-clear-composer.txt"
ax_action focus composer-input "" "$OUT/action-focus-composer.txt"

resize_window 1080x720 narrow-initial r7c-narrow resize_narrow
shot_phase r7c-narrow-narrow-initial "$OUT/connected-1080x720.png" "connected-1080x720"
ax_action press inspector-toggle "" "$OUT/action-open-activity-popover.txt"
require_phase r7c-narrow-popover popover
shot_phase r7c-narrow-popover-popover "$OUT/activity-popover-1080x720.png" "popover-1080x720"
ax_action press inspector-toggle "" "$OUT/action-close-activity-popover.txt"
ax_action focus composer-input "" "$OUT/action-refocus-after-popover.txt"
require_phase r7c-narrow popover-closed

for (( cycle=1; cycle<=RESIZE_CYCLES; cycle++ )); do
  resize_window 1440x1024 "wide-$cycle" r7c-wide resize_wide
  resize_window 1080x720 "narrow-$cycle" r7c-narrow resize_narrow
done

trace "drop socket at 1080x720 and assert retained session/timeline/reconnect"
"$FIXTURE" drop-socket --root "$ROOT"
require_phase r7c-disconnected dropped
shot_phase r7c-disconnected-dropped "$OUT/disconnected-1080x720.png" "disconnected-1080x720"
"$PY" "$R7_TOOLS" paint-assert \
  --screenshot "$OUT/disconnected-1080x720.png" \
  --geometry "$OUT/geometry-r7c-disconnected-dropped.txt" \
  --out "$OUT/assert-r7c-disconnected-rail-paint.json" \
  || die "disconnected rail paint gap assertion failed"
PRE_RECONNECT_SEQ="$(barrier_seq)"
[[ "$PRE_RECONNECT_SEQ" =~ ^[0-9]+$ ]] || PRE_RECONNECT_SEQ="$OPEN_SEQ"
ax_action press reconnect "" "$OUT/action-reconnect.txt"
RECONNECT_SEQ="$(wait_timeline_stable "$PRE_RECONNECT_SEQ" "" 0 "$BARRIER_TIMEOUT_SECS")"
ax_action press "$SESSION_ROW" "" "$OUT/action-reopen-beta-long.txt"
wait_timeline_stable "$RECONNECT_SEQ" "$SESSION_ID" "$LOGICAL_ROWS" "$BARRIER_TIMEOUT_SECS" >/dev/null
ax_action focus composer-input "" "$OUT/action-refocus-reconnected.txt"
require_phase r7c-narrow-reconnected reconnected

copy_runtime_evidence
"$PY" "$R7_TOOLS" report --samples "$SAMPLES" \
  --platform "$OUT/platform-preferences.json" --dataset "$OUT/dataset.json" \
  --out "$OUT/performance-baseline.json" || die "performance report failed"
"$PY" "$PY_TOOLS" shell-manifest --dir "$OUT" --repo "$REPO_ROOT" \
  --scenario r7-wave-c-resilience --label "$LABEL"

trace "run done; teardown fixture root=$ROOT"
"$FIXTURE" down --root "$ROOT"
"$FIXTURE" clean --root "$ROOT"
DESKTOP_PID=""
rm -rf "$WORK"
trap - EXIT
exit 0
