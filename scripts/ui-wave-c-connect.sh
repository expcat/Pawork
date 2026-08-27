#!/usr/bin/env bash
# Pawork R2 Wave C - U2 connection-failure retry driver (scripts-only).
#
# Loop 1 (Disconnected retry): seed -> serve -> desktop -> AXPress open
# session -> drop-socket (host drops connections, keeps listening) -> phase
# disconnected (reconnect present, shell/rail/session/timeline retained) ->
# AXPress reconnect -> new timeline_stable barrier (Connected again) ->
# AXPress session row (re-open; no-op if active session survived) -> phase
# reconnected (reconnect absent, session + timeline restored from host).
#
# Loop 2 (ConnectFailed retry): serve_stop.request barrier (frozen fixture
# contract, same one ui-fixture.sh stop_host uses) stops the host -> desktop
# Disconnected -> phase disconnected -> AXPress reconnect while host is down
# -> connection-status label "Connect failed" + reconnect affordance stays
# (phase connect-failed) -> restart-host -> AXPress reconnect -> new settle
# barrier -> AXPress session row -> phase reconnected.
#
# 同步只用 barrier/轮询（相位断言本身充当收敛轮询），禁固定 sleep。
# 清理只删自建 fixture root 与临时工具目录；失败保留证据并退出非零。
#
# Usage: scripts/ui-wave-c-connect.sh run --out <dir> [--label <name>]
#
# Exit codes: 0 all phase assertions pass; 2 usage error; 3 infrastructure
#   failure; 4 structural assertion failure (phase timeout).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
FIXTURE="$SCRIPT_DIR/ui-fixture.sh"
SESSION_ID="fx-ses-alpha-today"
SESSION_ROW="session-$SESSION_ID"
BARRIER_TIMEOUT_SECS="${PAWORK_UI_BARRIER_TIMEOUT_SECS:-180}"
WINDOW_TIMEOUT_SECS="${PAWORK_UI_WINDOW_TIMEOUT_SECS:-120}"
PHASE_TIMEOUT_SECS="${PAWORK_UI_PHASE_TIMEOUT_SECS:-30}"

die() { echo "ui-wave-c: $*" >&2; exit 3; }
usage() {
  echo "usage: scripts/ui-wave-c-connect.sh run --out <dir> [--label <name>]" >&2
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
LABEL="wave-c-connect"
while (( $# )); do
  case "$1" in
    run) MODE="$1"; shift ;;
    --out) OUT="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "ui-wave-c: unknown argument $1" >&2; usage ;;
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

WORK="$(mktemp -d /tmp/pawork-wave-c-tools.XXXXXX)"
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
HOST_PID="$(cat "$ROOT/host.pid")"
trace "host pid=$HOST_PID"

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
# 通过或超时；断言 JSON/树/几何文件落在 OUT 里兼作失败证据。tag 用于
# 同一相位的多轮证据（如 connect-failed 循环）不互相覆盖。
wait_phase() { # $1=phase $2=timeout_secs $3=tag(optional)
  local phase="$1" timeout_secs="$2" tag="${3:-}"
  local stem="$phase"
  [[ -n "$tag" ]] && stem="$phase-$tag"
  local tree="$OUT/ax-tree-$stem.txt"
  local frames="$OUT/geometry-$stem.txt"
  local json="$OUT/assert-$stem.json"
  local deadline=$(( SECONDS + timeout_secs ))
  local attempt=0 dump_rc frames_rc assert_rc
  trace "wait phase=$phase stem=$stem (phase-aware assertion polling)"
  while :; do
    attempt=$(( attempt + 1 ))
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$tree" \
      --wid-out "$WORK/wid-$stem.txt" >/dev/null 2>&1
    dump_rc=$?
    "$WORK/ax-frames" "$DESKTOP_PID" > "$frames" 2>/dev/null
    frames_rc=$?
    "$PY" "$PY_TOOLS" assert --frames "$frames" --tree "$tree" \
      --phase "$phase" --out "$json" >/dev/null 2>&1
    assert_rc=$?
    set -e
    if (( dump_rc == 0 && frames_rc == 0 && assert_rc == 0 )); then
      trace "phase=$phase stem=$stem ok attempt=$attempt"
      return 0
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null \
      || { tail -5 "$ROOT/logs/desktop.log" >&2 || true; die "desktop exited during phase=$phase"; }
    (( SECONDS < deadline )) || {
      trace "phase=$phase stem=$stem TIMEOUT attempt=$attempt dump=$dump_rc frames=$frames_rc assert=$assert_rc"
      return 1
    }
    sleep 0.2
  done
}

require_phase() { # $1=phase $2=tag(optional)
  wait_phase "$1" "$PHASE_TIMEOUT_SECS" "${2:-}" || {
    echo "ui-wave-c: structural assertion failed phase=$1 (evidence kept: $OUT)" >&2
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

# host 停机等待：serve_stop.request 是 fixture host 监听的冻结 barrier
# （ui-fixture.sh stop_host 同款）；这里只轮询进程退出，不发信号。
wait_host_exit() { # $1=pid
  local pid="$1"
  local deadline=$(( SECONDS + BARRIER_TIMEOUT_SECS ))
  trace "wait host exit after serve_stop.request (pid=$pid)"
  while kill -0 "$pid" 2>/dev/null; do
    (( SECONDS < deadline )) || {
      tail -20 "$ROOT/logs/serve.log" >&2 || true
      die "timeout waiting host exit (pid=$pid)"
    }
    sleep 0.2
  done
  trace "host exited (pid=$pid)"
}

# ConnectFailed 收敛轮询：connection-status 节点 value 必须是
# "Connect failed · ..."（Connecting/Disconnected 文案不算）。
wait_connect_failed_label() { # $1=evidence tree
  local tree="$1"
  local deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
  local attempt=0 dump_rc
  trace "wait connection-status label=Connect failed"
  while :; do
    attempt=$(( attempt + 1 ))
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$tree" >/dev/null 2>&1
    dump_rc=$?
    set -e
    if (( dump_rc == 0 )) \
        && grep 'identifier="connection-status"' "$tree" | grep -q 'value="Connect failed'; then
      trace "connection-status=Connect failed ok attempt=$attempt"
      return 0
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null \
      || { tail -5 "$ROOT/logs/desktop.log" >&2 || true; die "desktop exited waiting Connect failed"; }
    (( SECONDS < deadline )) || {
      trace "connection-status=Connect failed TIMEOUT attempt=$attempt dump=$dump_rc"
      return 1
    }
    sleep 0.2
  done
}

wait_desktop_ready

trace "place Desktop window deterministically on the main display"
"$WORK/ax-frames" "$DESKTOP_PID" --place-main > "$OUT/window-place.txt" \
  || die "failed to place Desktop window (see $OUT/window-place.txt)"
grep -q '# place-main result=0 ' "$OUT/window-place.txt" \
  || die "Desktop main-display placement did not converge (see $OUT/window-place.txt)"

INITIAL_SEQ="$(wait_timeline_stable 0 "" 0 "$BARRIER_TIMEOUT_SECS")"

trace "loop 1: open session before disconnect"
ax_press "$SESSION_ROW" "$OUT/action-press-session.txt" "open session"
OPEN_SEQ="$(wait_timeline_stable "$INITIAL_SEQ" "$SESSION_ID" 1 "$BARRIER_TIMEOUT_SECS")"

trace "loop 1: drop-socket (host drops connections, keeps listening)"
"$FIXTURE" drop-socket --root "$ROOT"
require_phase "disconnected"

PRE_RECONNECT_SEQ="$(barrier_seq)"
[[ "$PRE_RECONNECT_SEQ" =~ ^[0-9]+$ ]] || PRE_RECONNECT_SEQ="$OPEN_SEQ"
ax_press "reconnect" "$OUT/action-press-reconnect-drop.txt" "reconnect after drop"
RECONNECT_SEQ="$(wait_timeline_stable "$PRE_RECONNECT_SEQ" "" 0 "$BARRIER_TIMEOUT_SECS")"
# 重开会话：同进程断连不清 active_session_id（apply_fresh_snapshot 不恢复
# 但也不清除进程内状态），该 press 在会话存活时退化为聚焦 Composer，
# 在会话丢失时走 open_session 重载——两种语义都收敛到选中 + timeline 加载。
ax_press "$SESSION_ROW" "$OUT/action-press-session-reconnected.txt" "re-open session after reconnect"
wait_timeline_stable "$RECONNECT_SEQ" "$SESSION_ID" 1 "$BARRIER_TIMEOUT_SECS" >/dev/null
require_phase "reconnected"

trace "loop 2: stop host via frozen serve_stop.request barrier"
: > "$ROOT/barriers/serve_stop.request"
wait_host_exit "$HOST_PID"
require_phase "disconnected" "host-stopped"

PRE_RETRY_SEQ="$(barrier_seq)"
[[ "$PRE_RETRY_SEQ" =~ ^[0-9]+$ ]] || PRE_RETRY_SEQ="$PRE_RECONNECT_SEQ"
ax_press "reconnect" "$OUT/action-press-reconnect-failed.txt" "reconnect while host down (expect Connect failed)"
wait_connect_failed_label "$OUT/ax-tree-connect-failed.txt" \
  || { echo "ui-wave-c: Connect failed label not observed (evidence kept: $OUT)" >&2; exit 4; }
# ConnectFailed 相位：重试入口仍在场，且 connection-status 必须是 Connect failed。
require_phase "connect-failed"

trace "loop 2: restart-host (stop stale pid + serve new host + host_restarted barrier)"
"$FIXTURE" restart-host --root "$ROOT"
[[ -f "$ROOT/barriers/host_restarted" ]] \
  || die "restart-host completed but host_restarted barrier missing"

PRE_RECONNECT2_SEQ="$(barrier_seq)"
[[ "$PRE_RECONNECT2_SEQ" =~ ^[0-9]+$ ]] || PRE_RECONNECT2_SEQ="$PRE_RETRY_SEQ"
ax_press "reconnect" "$OUT/action-press-reconnect-restart.txt" "reconnect after host restart"
RECONNECT2_SEQ="$(wait_timeline_stable "$PRE_RECONNECT2_SEQ" "" 0 "$BARRIER_TIMEOUT_SECS")"
ax_press "$SESSION_ROW" "$OUT/action-press-session-after-restart.txt" "re-open session after host-restart reconnect"
wait_timeline_stable "$RECONNECT2_SEQ" "$SESSION_ID" 1 "$BARRIER_TIMEOUT_SECS" >/dev/null
require_phase "reconnected" "host-restart"

copy_runtime_evidence
"$PY" "$PY_TOOLS" shell-manifest --dir "$OUT" --repo "$REPO_ROOT" \
  --scenario wave-c-connect --label "$LABEL"

trace "teardown fixture root=$ROOT"
"$FIXTURE" down --root "$ROOT"
"$FIXTURE" clean --root "$ROOT"
DESKTOP_PID=""
rm -rf "$WORK"
trace "run done (all phase assertions passed)"
exit 0
