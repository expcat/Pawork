#!/usr/bin/env bash
# Pawork R4 Wave B WS-2 - U2 agent 状态场景九连驱动（scripts-only）。
#
# 编排：seed -> serve -> desktop（隔离 PAWORK_DATA_DIR + barrier 目录）->
# 相位断言轮询（states-assert）-> 每场景 AX dump + 截图归档。
#
# 场景（断言在 ui-wave-d-tools.py states-assert / entry-compare）：
#   S1 approval：选 fx-ses-beta-pending -> 等 approval_visible barrier ->
#      approval-visible（卡 + 三按钮 enabled + rail Needs input + approval
#      requested 行）-> AXPress approve-once -> approval-resolved（卡消失 +
#      Needs input 消失 + tool-row value=failed；决策行 live 不推，不在
#      此断言）-> 切走 alpha-today 再切回（强制快照重拉）->
#      approval-replayed（卡仍消失 + 决策行 approval approve_once 出现）。
#   S2 failed：选 fx-ses-alpha-yesterday -> Run failed 摘要卡 + 种子原因
#      原文（fixture scripted provider failure）+ Run failed 页脚。
#   S3 cancelled：选 fx-ses-beta-cancelled -> Run cancelled 摘要 + 页脚。
#   S4 tool_failed：选 fx-ses-beta-toolfailed -> tool-row value=failed +
#      fixture tool failure 详情。
#   S5 virtualized：选 fx-ses-beta-long -> barrier entry_count=64 且 AX
#      timeline-entry-*/run-summary-* 节点数 < 64（窗口切片卸载证据）。
#   S6 streamed：回 fx-ses-alpha-today -> composer set-value 普通消息 +
#      send -> entry_count 增长 + Ready for review 摘要 + composer 清空。
#   S7 live-failed：fixture:fail -> Run failed 摘要 + 诚实兜底（live wire
#      不带原因）-> 切走再切回强制快照重拉 -> failed-replayed（真实
#      provider 原因经重放出现）。
#   S8 hang-cancel：fixture:hang -> cancel 可用 -> AXPress cancel ->
#      Run cancelled 摘要 + composer 回到空闲可输入态（草稿已清空，
#      所以空输入 Send disabled）。
#   S9 replay：host 停（serve_stop.request）-> disconnected 保留 -> host 起
#      -> AXPress reconnect -> reconnected + 断线前后 entry_count 与
#      timeline-entry identifier 集合一致（entry-compare）。
#
# 已知限制：滚轮无法经 U2 注入（swift helper 无 wheel），BackToBottom
# 抢夺场景只登记不驱动（留 U1）。
#
# 同步只用 barrier/轮询（相位断言本身充当收敛轮询），禁固定 sleep。
# 清理只删自建 fixture root 与临时工具目录；失败保留证据并退出非零。
#
# Usage: scripts/ui-r4-wave-b-states.sh run --out <dir> [--label <name>]
#   （建议 --out docs/ui-review/r4-wave-b/u2）
#
# Exit codes: 0 all phase assertions pass; 2 usage error; 3 infrastructure
#   failure; 4 structural assertion failure (phase/compare timeout).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
FIXTURE="$SCRIPT_DIR/ui-fixture.sh"
WAVE_D_TOOLS="$SCRIPT_DIR/ui-wave-d-tools.py"
BARRIER_TIMEOUT_SECS="${PAWORK_UI_BARRIER_TIMEOUT_SECS:-180}"
WINDOW_TIMEOUT_SECS="${PAWORK_UI_WINDOW_TIMEOUT_SECS:-120}"
PHASE_TIMEOUT_SECS="${PAWORK_UI_PHASE_TIMEOUT_SECS:-30}"
RUN_TIMEOUT_SECS="${PAWORK_UI_RUN_TIMEOUT_SECS:-900}"

die() { echo "ui-r4b-states: $*" >&2; exit 3; }
usage() {
  echo "usage: scripts/ui-r4-wave-b-states.sh run --out <dir> [--label <name>]" >&2
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
LABEL="r4-wave-b-states"
while (( $# )); do
  case "$1" in
    run) MODE="$1"; shift ;;
    --out) OUT="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "ui-r4b-states: unknown argument $1" >&2; usage ;;
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
[[ -x "$FIXTURE" ]] || die "missing executable $FIXTURE"

mkdir -p "$OUT/logs" "$OUT/barriers"
OUT="$(cd "$OUT" && pwd)"
TRACE="$OUT/action-trace.txt"
: > "$TRACE"
trace() {
  echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" | tee -a "$TRACE" >&2
}

WORK="$(mktemp -d /tmp/pawork-r4b-states-tools.XXXXXX)"
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

# 全局 watchdog：超上限 TERM 主进程，经 die 记录失败并触发 teardown 清理。
(
  sleep "$RUN_TIMEOUT_SECS"
  kill -TERM "$$"
) &
RUN_WATCHDOG=$!
trap 'kill "$RUN_WATCHDOG" 2>/dev/null; die "run watchdog timeout ($RUN_TIMEOUT_SECS)"' TERM

trace "run start label=$LABEL out=$OUT"
trace "compile helpers into $WORK"
swiftc -O -o "$WORK/ui-ax-dump" "$SCRIPT_DIR/ui-ax-dump.swift" 2>"$WORK/swiftc-axdump.err" \
  || { cat "$WORK/swiftc-axdump.err" >&2; die "ui-ax-dump compile failed"; }
swiftc -O -o "$WORK/ax-frames" "$SCRIPT_DIR/ui-ax-frames.swift" 2>"$WORK/swiftc-frames.err" \
  || { cat "$WORK/swiftc-frames.err" >&2; die "ax-frames compile failed"; }
AXDUMP="$WORK/ui-ax-dump"
FRAMES="$WORK/ax-frames"

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
    kill -0 "$DESKTOP_PID" 2>/dev/null \
      || { tail -5 "$ROOT/logs/desktop.log" >&2 || true; die "desktop exited early"; }
    if (( SECONDS >= deadline )); then
      cp "$WORK/probe.txt" "$OUT/ax-tree-probe-timeout.txt" 2>/dev/null || true
      copy_runtime_evidence
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
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited while waiting timeline_stable"
    (( SECONDS < deadline )) || die "timeout waiting timeline_stable"
    sleep 0.2
  done
}

timeline_entries() {
  barrier_field "$ROOT/barriers/timeline_stable" entries
}

# 相位断言本身即收敛轮询：AX dump + states-assert 反复重试直到相位通过
# 或超时；断言 JSON/树文件落在 OUT 里兼作失败证据。
wait_phase() { # $1=phase $2=timeout_secs(optional) $3=logical_entries(optional)
  local phase="$1" timeout_secs="${2:-$PHASE_TIMEOUT_SECS}" logical="${3:-}"
  local tree="$OUT/ax-tree-$phase.txt"
  local json="$OUT/assert-$phase.json"
  local deadline=$(( SECONDS + timeout_secs ))
  local attempt=0 dump_rc assert_rc
  trace "wait phase=$phase (states-assert polling)"
  while :; do
    attempt=$(( attempt + 1 ))
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$tree" >/dev/null 2>&1
    dump_rc=$?
    if [[ -n "$logical" ]]; then
      "$PY" "$WAVE_D_TOOLS" states-assert --tree "$tree" --phase "$phase" \
        --logical-entries "$logical" --out "$json" >/dev/null 2>&1
    else
      "$PY" "$WAVE_D_TOOLS" states-assert --tree "$tree" --phase "$phase" \
        --out "$json" >/dev/null 2>&1
    fi
    assert_rc=$?
    set -e
    if (( dump_rc == 0 && assert_rc == 0 )); then
      trace "phase=$phase ok attempt=$attempt"
      return 0
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null \
      || { tail -5 "$ROOT/logs/desktop.log" >&2 || true; die "desktop exited during phase=$phase"; }
    (( SECONDS < deadline )) || {
      trace "phase=$phase TIMEOUT attempt=$attempt dump=$dump_rc assert=$assert_rc"
      return 1
    }
    sleep 0.2
  done
}

require_phase() { # $1=phase $2=logical_entries(optional)
  wait_phase "$1" "$PHASE_TIMEOUT_SECS" "${2:-}" || {
    echo "ui-r4b-states: structural assertion failed phase=$1 (evidence kept: $OUT)" >&2
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

composer_set_value() { # $1=value $2=evidence-file
  trace "AX set-value composer-input"
  "$AXDUMP" --pid "$DESKTOP_PID" --set-value composer-input "$1" \
    --action-only --out "$2" >/dev/null \
    || die "AX set-value composer-input failed (see $2)"
  grep -q 'result=0' "$2" || die "AX set-value composer-input result!=0 (see $2)"
}

screenshot() { # $1=phase
  local wid
  "$AXDUMP" --pid "$DESKTOP_PID" --out "$WORK/wid-$1.txt" --wid-out "$WORK/wid-$1.id" >/dev/null 2>&1 \
    || die "AX dump for wid failed (phase $1)"
  wid="$(cat "$WORK/wid-$1.id")"
  screencapture -x -o -l "$wid" "$OUT/shot-$1.png" || die "screencapture failed (phase $1, wid=$wid)"
  [[ -s "$OUT/shot-$1.png" ]] || die "screenshot empty (phase $1, wid=$wid)"
  trace "screenshot $OUT/shot-$1.png"
}

# 审批 barrier：approval_visible 存在且 JSON 含 tool/run_id；消失即文件删除。
wait_approval_barrier() { # $1=want(present|absent)
  local want="$1"
  local deadline=$(( SECONDS + BARRIER_TIMEOUT_SECS ))
  local approval_line approval_rc
  trace "wait approval_visible $want"
  while :; do
    set +e
    approval_line="$("$PY" "$WAVE_D_TOOLS" approval-read --file "$ROOT/barriers/approval_visible" 2>/dev/null)"
    approval_rc=$?
    set -e
    if [[ "$want" == "present" && "$approval_rc" == 0 ]]; then
      echo "approval_visible $approval_line" | tee -a "$OUT/approval-barrier.txt" >&2
      trace "approval_visible present ($approval_line)"
      return 0
    fi
    if [[ "$want" == "absent" && "$approval_rc" != 0 ]]; then
      echo "approval_visible removed" | tee -a "$OUT/approval-barrier.txt" >&2
      trace "approval_visible absent"
      return 0
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited waiting approval_visible $want"
    (( SECONDS < deadline )) || die "timeout waiting approval_visible $want"
    sleep 0.2
  done
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

wait_desktop_ready

trace "place Desktop window deterministically on the main display"
"$FRAMES" "$DESKTOP_PID" --place-main > "$OUT/window-place.txt" \
  || die "failed to place Desktop window (see $OUT/window-place.txt)"
grep -q '# place-main result=0 ' "$OUT/window-place.txt" \
  || die "Desktop main-display placement did not converge (see $OUT/window-place.txt)"

INITIAL_SEQ="$(wait_timeline_stable 0 "" 0 "$BARRIER_TIMEOUT_SECS")"

# ---------------------------------------------------------------- S1 approval
trace "S1: open pending-approval session"
ax_press session-fx-ses-beta-pending "$OUT/action-press-pending.txt" "open beta-pending"
S1_SEQ="$(wait_timeline_stable "$INITIAL_SEQ" fx-ses-beta-pending 1 "$BARRIER_TIMEOUT_SECS")"
wait_approval_barrier present
require_phase approval-visible
screenshot approval-visible

trace "S1: approve once -> card disappears + tool row shows failed"
ax_press approve-once "$OUT/action-press-approve-once.txt" "approve once"
S1_RESOLVED_SEQ="$(wait_timeline_stable "$S1_SEQ" fx-ses-beta-pending 1 "$BARRIER_TIMEOUT_SECS")"
wait_approval_barrier absent
require_phase approval-resolved
screenshot approval-resolved

trace "S1: reselect away+back -> replayed snapshot surfaces decision row"
ax_press session-fx-ses-alpha-today "$OUT/action-press-reselect-away.txt" "reselect away to alpha-today"
S1_AWAY_SEQ="$(wait_timeline_stable "$S1_RESOLVED_SEQ" fx-ses-alpha-today 1 "$BARRIER_TIMEOUT_SECS")"
ax_press session-fx-ses-beta-pending "$OUT/action-press-reselect-back.txt" "reselect back to beta-pending"
wait_timeline_stable "$S1_AWAY_SEQ" fx-ses-beta-pending 1 "$BARRIER_TIMEOUT_SECS" >/dev/null
require_phase approval-replayed
screenshot approval-replayed

# ---------------------------------------------------------------- S2 failed
trace "S2: seeded failed session renders Run failed summary"
ax_press session-fx-ses-alpha-yesterday "$OUT/action-press-failed.txt" "open alpha-yesterday"
S2_SEQ="$(wait_timeline_stable "$S1_SEQ" fx-ses-alpha-yesterday 1 "$BARRIER_TIMEOUT_SECS")"
require_phase failed-summary
screenshot failed-summary

# ---------------------------------------------------------------- S3 cancelled
trace "S3: seeded cancelled session renders Run cancelled summary"
ax_press session-fx-ses-beta-cancelled "$OUT/action-press-cancelled.txt" "open beta-cancelled"
S3_SEQ="$(wait_timeline_stable "$S2_SEQ" fx-ses-beta-cancelled 1 "$BARRIER_TIMEOUT_SECS")"
require_phase cancelled-summary
screenshot cancelled-summary

# ---------------------------------------------------------------- S4 tool_failed
trace "S4: seeded tool failure renders raw wire status"
ax_press session-fx-ses-beta-toolfailed "$OUT/action-press-toolfailed.txt" "open beta-toolfailed"
S4_SEQ="$(wait_timeline_stable "$S3_SEQ" fx-ses-beta-toolfailed 1 "$BARRIER_TIMEOUT_SECS")"
require_phase tool-failed
screenshot tool-failed

# ---------------------------------------------------------------- S5 virtualized
trace "S5: 64 logical rows render as AX window slice"
ax_press session-fx-ses-beta-long "$OUT/action-press-long.txt" "open beta-long"
S5_SEQ="$(wait_timeline_stable "$S4_SEQ" fx-ses-beta-long 64 "$BARRIER_TIMEOUT_SECS")"
S5_ENTRIES="$(timeline_entries)"
[[ "$S5_ENTRIES" =~ ^[0-9]+$ ]] || die "no timeline_stable entry_count for beta-long"
printf '{"session": "fx-ses-beta-long", "logical_entries": %s}\n' "$S5_ENTRIES" > "$OUT/s5-entry-count.json"
require_phase virtualized "$S5_ENTRIES"
screenshot virtualized

# ---------------------------------------------------------------- S6 streamed
trace "S6: back to alpha-today, send normal message (3-chunk stream)"
ax_press session-fx-ses-alpha-today "$OUT/action-press-alpha-today.txt" "back to alpha-today"
S6_BASE_SEQ="$(wait_timeline_stable "$S5_SEQ" fx-ses-alpha-today 1 "$BARRIER_TIMEOUT_SECS")"
S6_BEFORE="$(timeline_entries)"
[[ "$S6_BEFORE" =~ ^[0-9]+$ ]] || die "no timeline_stable entry_count before stream"
composer_set_value "U2 状态场景回归：普通消息走三段流式" "$OUT/action-set-value-stream.txt"
ax_press send "$OUT/action-press-send-stream.txt" "send streamed message"
wait_timeline_stable "$S6_BASE_SEQ" fx-ses-alpha-today $(( S6_BEFORE + 3 )) "$BARRIER_TIMEOUT_SECS" >/dev/null
S6_AFTER="$(timeline_entries)"
printf '{"session": "fx-ses-alpha-today", "before": %s, "after": %s}\n' "$S6_BEFORE" "$S6_AFTER" > "$OUT/s6-entry-growth.json"
require_phase streamed-summary
screenshot streamed-summary

# ---------------------------------------------------------------- S7 live fail
trace "S7: fixture:fail -> live provider failure summary"
S7_SEQ="$(barrier_field "$ROOT/barriers/timeline_stable" seq)"
[[ "$S7_SEQ" =~ ^[0-9]+$ ]] || S7_SEQ="$S6_BASE_SEQ"
S7_BEFORE="$(timeline_entries)"
composer_set_value "fixture:fail" "$OUT/action-set-value-fail.txt"
ax_press send "$OUT/action-press-send-fail.txt" "send fixture:fail"
wait_timeline_stable "$S7_SEQ" fx-ses-alpha-today $(( S7_BEFORE + 2 )) "$BARRIER_TIMEOUT_SECS" >/dev/null
require_phase live-failed
screenshot live-failed

trace "S7: reselect alpha-today (away+back) -> replay shows real failure reason"
ax_press session-fx-ses-alpha-yesterday "$OUT/action-press-s7-away.txt" "S7 reselect away"
S7_AWAY_SEQ="$(wait_timeline_stable "$S7_SEQ" fx-ses-alpha-yesterday 1 "$BARRIER_TIMEOUT_SECS")"
ax_press session-fx-ses-alpha-today "$OUT/action-press-s7-back.txt" "S7 reselect back"
wait_timeline_stable "$S7_AWAY_SEQ" fx-ses-alpha-today 1 "$BARRIER_TIMEOUT_SECS" >/dev/null
require_phase failed-replayed
screenshot failed-replayed

# ---------------------------------------------------------------- S8 hang + cancel
trace "S8: fixture:hang -> cancel -> Run cancelled + composer idle/input-ready"
S8_SEQ="$(barrier_field "$ROOT/barriers/timeline_stable" seq)"
[[ "$S8_SEQ" =~ ^[0-9]+$ ]] || S8_SEQ="$S7_SEQ"
S8_BEFORE="$(timeline_entries)"
composer_set_value "fixture:hang" "$OUT/action-set-value-hang.txt"
ax_press send "$OUT/action-press-send-hang.txt" "send fixture:hang"
require_phase hang-cancelable
screenshot hang-cancelable
ax_press cancel "$OUT/action-press-cancel.txt" "cancel hanging run"
wait_timeline_stable "$S8_SEQ" fx-ses-alpha-today $(( S8_BEFORE + 2 )) "$BARRIER_TIMEOUT_SECS" >/dev/null
require_phase hang-cancelled
screenshot hang-cancelled

# ---------------------------------------------------------------- S9 disconnect replay
trace "S9: stop host -> disconnected keeps timeline -> restart -> replay identical"
S9_PRE_SEQ="$(barrier_field "$ROOT/barriers/timeline_stable" seq)"
[[ "$S9_PRE_SEQ" =~ ^[0-9]+$ ]] || S9_PRE_SEQ="$S8_SEQ"
S9_ENTRIES_BEFORE="$(timeline_entries)"
"$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$OUT/ax-tree-replay-before.txt" >/dev/null \
  || die "AX dump failed (replay-before)"
: > "$ROOT/barriers/serve_stop.request"
wait_host_exit "$HOST_PID"
require_phase disconnected-retained
screenshot disconnected-retained

trace "S9: restart host (host_restarted barrier) and reconnect"
"$FIXTURE" restart-host --root "$ROOT"
[[ -f "$ROOT/barriers/host_restarted" ]] \
  || die "restart-host completed but host_restarted barrier missing"
ax_press reconnect "$OUT/action-press-reconnect.txt" "reconnect after host restart"
wait_timeline_stable "$S9_PRE_SEQ" fx-ses-alpha-today 1 "$BARRIER_TIMEOUT_SECS" >/dev/null
require_phase reconnected-replay
S9_ENTRIES_AFTER="$(timeline_entries)"
"$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$OUT/ax-tree-replay-after.txt" >/dev/null \
  || die "AX dump failed (replay-after)"
screenshot reconnected-replay
set +e
"$PY" "$WAVE_D_TOOLS" entry-compare \
  --tree-a "$OUT/ax-tree-replay-before.txt" --tree-b "$OUT/ax-tree-replay-after.txt" \
  --entries-a "$S9_ENTRIES_BEFORE" --entries-b "$S9_ENTRIES_AFTER" \
  --out "$OUT/entry-compare.json" | tee -a "$OUT/entry-compare-run.txt" >&2
COMPARE_RC=${PIPESTATUS[0]}
set -e
if (( COMPARE_RC != 0 )); then
  echo "ui-r4b-states: replay entry-compare failed rc=$COMPARE_RC (evidence kept: $OUT)" >&2
  exit 4
fi

copy_runtime_evidence
"$PY" "$WAVE_D_TOOLS" shell-manifest --dir "$OUT" --repo "$REPO_ROOT" \
  --scenario r4-wave-b-states --label "$LABEL"

trace "teardown fixture root=$ROOT"
"$FIXTURE" down --root "$ROOT"
"$FIXTURE" clean --root "$ROOT"
DESKTOP_PID=""
rm -rf "$WORK"
trace "run done (all S1-S9 phase assertions passed)"
exit 0
