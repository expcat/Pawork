#!/usr/bin/env bash
# Pawork R5 Wave B W-B2 — U2 Composer 九场景驱动（scripts-only）。
#
# 编排：seed -> serve -> desktop（隔离 PAWORK_DATA_DIR + barrier 目录）->
# 相位断言轮询（test_ui_r5_wave_b_states.py assert）-> 每场景 AX dump + 截图归档。
#
# 场景（--label r5-N 可只跑一场景；默认九连）：
#   r5-1 空输入：open alpha-today -> send enabled=0、cancel 缺席、composer-input
#        value 为空（placeholder 不进 AXValue）。
#   r5-2 发送→running→取消：set-value 多行草稿 -> send enabled=1 -> AXPress send
#        -> cancel 出现且 enabled=1、send 缺席 -> AXPress cancel -> send 复原；
#        timeline 回显多行草稿。
#   r5-3 断线草稿：set-value -> drop-socket -> send disabled 且文本在 ->
#        restart-host + reconnect -> 文本仍在。
#   r5-4 task 切换草稿隔离：A 写 draft-A -> 切 B（composer 空）-> 写 draft-B ->
#        切回 A -> 恢复 draft-A。
#   r5-5 大文本粘贴：pbcopy 多行大文本（≥8KB 含 CJK）-> HID cmd-v -> composer
#        value 完整 -> 发送 -> timeline 回显一致。
#   r5-6 running 视觉：fixture:hang 进 running -> shot-hang-cancelable.png。
#   r5-7 model 菜单：AXPress model-picker -> 菜单开 -> 若 seed 有第二 model 则
#        选中并断言触发器文案变更；单 model 降级为菜单开合断言并注明。
#   r5-8 窄窗 1080×720：resize 后 composer/send 可达、无溢出（assert frames）。
#   r5-9 键盘路径：HID 键入短文本 -> Return 发送 -> 再键入 -> cmd-. 取消；
#        shift-return 插入换行不发送。
#
# 同步只用 barrier/轮询（相位断言本身充当收敛轮询），禁固定 sleep。
# 清理只删自建 fixture root 与临时工具目录；失败保留证据并退出非零。
#
# Usage: scripts/ui-r5-wave-b-states.sh run --out <dir> [--label <name>]
#   --label r5-N 只跑该场景；其它值（默认 r5-wave-b-states）跑 r5-1..r5-9。
#
# Exit codes: 0 all phase assertions pass; 2 usage error; 3 infrastructure
#   failure; 4 structural assertion failure (phase timeout).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
FIXTURE="$SCRIPT_DIR/ui-fixture.sh"
WAVE_D_TOOLS="$SCRIPT_DIR/ui-wave-d-tools.py"
R5_TOOLS="$SCRIPT_DIR/test_ui_r5_wave_b_states.py"
BARRIER_TIMEOUT_SECS="${PAWORK_UI_BARRIER_TIMEOUT_SECS:-180}"
WINDOW_TIMEOUT_SECS="${PAWORK_UI_WINDOW_TIMEOUT_SECS:-120}"
PHASE_TIMEOUT_SECS="${PAWORK_UI_PHASE_TIMEOUT_SECS:-30}"
RUN_TIMEOUT_SECS="${PAWORK_UI_RUN_TIMEOUT_SECS:-900}"
KEY_RETRY_PROBE_SECS="${PAWORK_UI_KEY_RETRY_PROBE_SECS:-3}"
SESSION_A="fx-ses-alpha-today"
SESSION_B="fx-ses-alpha-yesterday"
SESSION_A_ROW="session-$SESSION_A"
SESSION_B_ROW="session-$SESSION_B"
PLACEHOLDER="Message Pawork… (Enter to send, Shift+Enter for newline)"
DRAFT_A="draft-A"
DRAFT_B="draft-B"
MULTILINE=$'fixture:hang\nR5 multiline draft\nsecond line 中文'
KEYBOARD_TEXT="ab"
HANG_TEXT="fixture:hang"

die() { echo "ui-r5b-states: $*" >&2; exit 3; }
usage() {
  echo "usage: scripts/ui-r5-wave-b-states.sh run --out <dir> [--label <name>]" >&2
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
LABEL="r5-wave-b-states"
while (( $# )); do
  case "$1" in
    run) MODE="$1"; shift ;;
    --out) OUT="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "ui-r5b-states: unknown argument $1" >&2; usage ;;
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
[[ -f "$R5_TOOLS" ]] || die "missing $R5_TOOLS"
[[ -x "$FIXTURE" ]] || die "missing executable $FIXTURE"

mkdir -p "$OUT/logs" "$OUT/barriers"
OUT="$(cd "$OUT" && pwd)"
TRACE="$OUT/action-trace.txt"
: > "$TRACE"
trace() {
  echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" | tee -a "$TRACE" >&2
}

WORK="$(mktemp -d /tmp/pawork-r5b-states-tools.XXXXXX)"
ROOT="$(mktemp -d /tmp/pawork-ui-fixture.XXXXXX)"
ROOT="$(cd "$ROOT" && pwd -P)"
DESKTOP_PID=""
HOST_PID=""
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
  # 恢复运行前锁定的输入源（见下方 pin ascii input source）。desktop 退出
  # 后再恢复：TISSelectInputSource 作用于当时的前台上下文，desktop 存活
  # 期间恢复会落到即将消亡的 desktop 上下文上。
  if [[ -n "${INPUT_SOURCE_BEFORE:-}" && -x "${KEY:-}" ]]; then
    "$KEY" --pid 1 --restore-input-source "$INPUT_SOURCE_BEFORE" \
      2> "$OUT/input-source-restore.txt" >/dev/null || true
  fi
  rm -rf "$WORK"
  exit $status
}
trap fixture_teardown EXIT

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
swiftc -O -o "$WORK/ui-key-event" "$SCRIPT_DIR/ui-key-event.swift" 2>"$WORK/swiftc-key.err" \
  || { cat "$WORK/swiftc-key.err" >&2; die "ui-key-event compile failed"; }
AXDUMP="$WORK/ui-ax-dump"
FRAMES="$WORK/ax-frames"
KEY="$WORK/ui-key-event"

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

soft_activate() {
  "$SCRIPT_DIR/ui-focus-switch.sh" activate --pid "$DESKTOP_PID" >/dev/null 2>&1 || true
}

dump_tree() { # $1=evidence-file
  "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$1" >/dev/null 2>&1 \
    || die "AX dump failed (see $1)"
}

key() { # $1=key $2=modifiers(optional)
  soft_activate
  if (( $# > 1 )); then
    "$KEY" --pid "$DESKTOP_PID" --key "$1" --modifiers "$2" >/dev/null
  else
    "$KEY" --pid "$DESKTOP_PID" --key "$1" >/dev/null
  fi
}

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

wait_phase() { # $1=phase [extra assert args...]
  local phase="$1"; shift
  local tree="$OUT/ax-tree-$phase.txt"
  local json="$OUT/assert-$phase.json"
  local deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
  local attempt=0 dump_rc assert_rc
  trace "wait phase=$phase (composer-assert polling)"
  while :; do
    attempt=$(( attempt + 1 ))
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$tree" >/dev/null 2>&1
    dump_rc=$?
    "$PY" "$R5_TOOLS" assert --tree "$tree" --phase "$phase" --out "$json" "$@" >/dev/null 2>&1
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

require_phase() { # $1=phase [extra assert args...]
  local phase="$1"; shift
  wait_phase "$phase" "$@" || {
    echo "ui-r5b-states: structural assertion failed phase=$phase (evidence kept: $OUT)" >&2
    exit 4
  }
}

wait_tree_contains() { # $1=pattern $2=timeout_secs $3=label $4=invert(1=invert)
  local deadline=$(( SECONDS + $2 ))
  local attempt=0 invert="${4:-0}"
  trace "wait tree contains: $3"
  while :; do
    attempt=$(( attempt + 1 ))
    dump_tree "$WORK/wait-tree.txt"
    set +e
    grep -q "$1" "$WORK/wait-tree.txt"
    local found=$?
    set -e
    if (( invert == 0 && found == 0 )) || (( invert == 1 && found != 0 )); then
      trace "tree contains ok: $3 attempt=$attempt"
      return 0
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited waiting $3"
    (( SECONDS < deadline )) || {
      cp "$WORK/wait-tree.txt" "$OUT/ax-tree-wait-timeout-$3.txt" 2>/dev/null || true
      trace "tree contains TIMEOUT: $3"
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

resize_window() { # $1=WxH $2=evidence-file $3=label
  trace "resize -> $1 ($3)"
  "$FRAMES" "$DESKTOP_PID" --resize "$1" > "$2" \
    || { cat "$2" >&2; die "resize $1 failed ($3, see $2)"; }
  grep -q '# resize result=0 ' "$2" \
    || die "resize $1 did not converge ($3, see $2)"
}

dump_frames() { # $1=evidence-file
  "$FRAMES" "$DESKTOP_PID" > "$1" \
    || die "ax-frames dump failed (see $1)"
}

focus_composer() {
  "$AXDUMP" --pid "$DESKTOP_PID" --focus composer-input --action-only \
    --out "$OUT/action-focus-composer-input.txt" >/dev/null 2>&1 || true
  wait_tree_contains 'identifier="composer-input"[^#]*focused=1' "$PHASE_TIMEOUT_SECS" composer-focused \
    || { echo "ui-r5b-states: composer-input not focused (evidence kept: $OUT)" >&2; exit 4; }
}

open_session() { # $1=session-id $2=evidence-stem
  local sid="$1" stem="$2" seq
  seq="$(barrier_field "$ROOT/barriers/timeline_stable" seq)"
  [[ "$seq" =~ ^[0-9]+$ ]] || seq=0
  ax_press "session-$sid" "$OUT/action-press-$stem.txt" "open $sid"
  wait_timeline_stable "$seq" "$sid" 1 "$BARRIER_TIMEOUT_SECS" >/dev/null
}

write_value_file() { # $1=path $2=value
  printf '%s' "$2" > "$1"
}

hid_type_chars() { # $1=text
  local ch
  soft_activate
  local IFS=
  for (( i=0; i<${#1}; i++ )); do
    ch="${1:i:1}"
    case "$ch" in
      [a-zA-Z0-9]) key "$ch" ;;
      *) die "hid_type_chars: unsupported char '$ch'" ;;
    esac
  done
}


want_scene() { # $1=id
  [[ "$LABEL" == "r5-wave-b-states" || "$LABEL" == "$1" ]]
}

scene_r5_1() {
  want_scene r5-1 || return 0
  trace "r5-1 empty input on alpha-today"
  open_session "$SESSION_A" "alpha-today"
  require_phase empty-input --expect-value ""
  screenshot empty-input
}

scene_r5_2() {
  want_scene r5-2 || return 0
  trace "r5-2 send multiline -> running cancel -> restored + echo"
  open_session "$SESSION_A" "r5-2-session"
  write_value_file "$OUT/r5-2-multiline.txt" "$MULTILINE"
  composer_set_value "$MULTILINE" "$OUT/action-set-value-multiline.txt"
  require_phase send-enabled --expect-value-file "$OUT/r5-2-multiline.txt"
  screenshot send-enabled
  local seq entries
  seq="$(barrier_field "$ROOT/barriers/timeline_stable" seq)"
  [[ "$seq" =~ ^[0-9]+$ ]] || seq=0
  entries="$(timeline_entries)"
  [[ "$entries" =~ ^[0-9]+$ ]] || entries=0
  ax_press send "$OUT/action-press-send-multiline.txt" "send multiline draft"
  require_phase running-cancelable
  screenshot running-cancelable
  ax_press cancel "$OUT/action-press-cancel-multiline.txt" "cancel multiline run"
  wait_timeline_stable "$seq" "$SESSION_A" $(( entries + 2 )) "$BARRIER_TIMEOUT_SECS" >/dev/null
  require_phase send-restored
  screenshot send-restored
  require_phase multiline-echoed --expect-contains "R5 multiline draft"
}

scene_r5_3() {
  want_scene r5-3 || return 0
  trace "r5-3 disconnected draft retained across drop-socket + restart-host"
  open_session "$SESSION_A" "r5-3-session"
  composer_set_value "keep-draft-r5-3" "$OUT/action-set-value-disconnect-draft.txt"
  require_phase draft-set --expect-value "keep-draft-r5-3"
  screenshot draft-set
  "$FIXTURE" drop-socket --root "$ROOT" >/dev/null
  require_phase disconnected-draft --expect-value "keep-draft-r5-3"
  screenshot disconnected-draft
  "$FIXTURE" restart-host --root "$ROOT"
  [[ -f "$ROOT/barriers/host_restarted" ]] \
    || die "restart-host completed but host_restarted barrier missing"
  HOST_PID="$(cat "$ROOT/host.pid")"
  ax_press reconnect "$OUT/action-press-reconnect.txt" "reconnect after host restart"
  wait_tree_contains 'value="[^"]*Connected[^"]*" identifier="connection-status"' "$BARRIER_TIMEOUT_SECS" reconnected-status \
    || { echo "ui-r5b-states: reconnect did not restore Connected (evidence kept: $OUT)" >&2; exit 4; }
  require_phase reconnected-draft --expect-value "keep-draft-r5-3"
  screenshot reconnected-draft
}

scene_r5_4() {
  want_scene r5-4 || return 0
  trace "r5-4 task switch draft isolation"
  open_session "$SESSION_A" "r5-4-a"
  composer_set_value "$DRAFT_A" "$OUT/action-set-value-draft-a.txt"
  require_phase draft-a --expect-value "$DRAFT_A"
  screenshot draft-a
  open_session "$SESSION_B" "r5-4-b"
  require_phase draft-b-empty
  screenshot draft-b-empty
  composer_set_value "$DRAFT_B" "$OUT/action-set-value-draft-b.txt"
  require_phase draft-b --expect-value "$DRAFT_B"
  screenshot draft-b
  open_session "$SESSION_A" "r5-4-a-back"
  require_phase draft-a-restored --expect-value "$DRAFT_A"
  screenshot draft-a-restored
}

build_paste_payload() {
  "$PY" - "$1" <<'PY'
import sys
from pathlib import Path
path = Path(sys.argv[1])
chunks = ["R5 paste CJK 中文日本語"]
while True:
    text = "\n".join(chunks)
    if len(text.encode("utf-8")) >= 8192:
        path.write_text(text, encoding="utf-8")
        print(len(text.encode("utf-8")))
        break
    chunks.append("中文行" + str(len(chunks)) + " 日本語 filler " + ("漢" * 40))
PY
}

scene_r5_5() {
  want_scene r5-5 || return 0
  trace "r5-5 pbcopy + HID cmd-v (modifiers cmd,v) large CJK paste"
  open_session "$SESSION_A" "r5-5-session"
  local bytes
  bytes="$(build_paste_payload "$OUT/r5-5-paste.txt")"
  trace "paste payload bytes=$bytes"
  pbcopy < "$OUT/r5-5-paste.txt"
  # Session A restores its per-session draft; clear it so the HID paste
  # (which inserts at the caret) starts from an empty composer.
  composer_set_value "" "$OUT/action-set-value-clear-paste.txt"
  focus_composer
  key v cmd
  require_phase paste-complete --expect-value-file "$OUT/r5-5-paste.txt"
  screenshot paste-complete
  local seq entries needle
  seq="$(barrier_field "$ROOT/barriers/timeline_stable" seq)"
  [[ "$seq" =~ ^[0-9]+$ ]] || seq=0
  entries="$(timeline_entries)"
  [[ "$entries" =~ ^[0-9]+$ ]] || entries=0
  needle="$(head -n 1 "$OUT/r5-5-paste.txt")"
  ax_press send "$OUT/action-press-send-paste.txt" "send pasted payload"
  wait_timeline_stable "$seq" "$SESSION_A" $(( entries + 2 )) "$BARRIER_TIMEOUT_SECS" >/dev/null
  require_phase paste-echoed --expect-contains "$needle"
  screenshot paste-echoed
}

scene_r5_6() {
  want_scene r5-6 || return 0
  trace "r5-6 fixture:hang running Cancel slot"
  open_session "$SESSION_A" "r5-6-session"
  composer_set_value "$HANG_TEXT" "$OUT/action-set-value-hang.txt"
  ax_press send "$OUT/action-press-send-hang.txt" "send fixture:hang"
  require_phase hang-cancelable
  screenshot hang-cancelable
  ax_press cancel "$OUT/action-press-cancel-hang.txt" "cancel hang"
  require_phase send-restored
}

count_model_options() {
  dump_tree "$WORK/model-count.txt"
  "$PY" - "$WORK/model-count.txt" <<'PY'
import re, sys
from pathlib import Path
text = Path(sys.argv[1]).read_text("utf-8")
ids = set(re.findall(r'identifier="([^"]*)"', text))
options = [
    identifier for identifier in ids
    if identifier.startswith("model-") and identifier not in {"model-menu", "model-picker"}
]
print(len(options))
print("\n".join(sorted(options)))
PY
}

scene_r5_7() {
  want_scene r5-7 || return 0
  trace "r5-7 model menu open/select-or-degrade"
  open_session "$SESSION_A" "r5-7-session"
  dump_tree "$WORK/model-before.txt"
  local before
  before="$("$PY" - "$WORK/model-before.txt" <<'PY'
import re, sys
from pathlib import Path
text = Path(sys.argv[1]).read_text("utf-8")
for line in text.splitlines():
    if 'identifier="model-picker"' in line:
        match = re.search(r'value="([^"]*)"', line)
        print(match.group(1) if match else "")
        break
PY
)"
  printf '%s\n' "$before" > "$OUT/r5-7-model-before.txt"
  ax_press model-picker "$OUT/action-press-model-picker.txt" "open model menu"
  require_phase model-menu-open
  screenshot model-menu-open
  local count first_other
  MODEL_INFO=()
  while IFS= read -r model_line; do
    MODEL_INFO+=("$model_line")
  done < <(count_model_options)
  count="${MODEL_INFO[0]:-0}"
  first_other=""
  local option
  for option in "${MODEL_INFO[@]:1}"; do
    [[ -n "$option" ]] || continue
    first_other="$option"
    break
  done
  if [[ "$count" =~ ^[0-9]+$ ]] && (( count >= 2 )) && [[ -n "$first_other" ]]; then
    ax_press "$first_other" "$OUT/action-press-model-option.txt" "select $first_other"
    wait_tree_contains 'identifier="model-menu"' "$PHASE_TIMEOUT_SECS" model-menu-closed-after-select 1 \
      || true
    dump_tree "$WORK/model-after.txt"
    local after
    after="$("$PY" - "$WORK/model-after.txt" <<'PY'
import re, sys
from pathlib import Path
text = Path(sys.argv[1]).read_text("utf-8")
for line in text.splitlines():
    if 'identifier="model-picker"' in line:
        match = re.search(r'value="([^"]*)"', line)
        print(match.group(1) if match else "")
        break
PY
)"
    printf '%s\n' "$after" > "$OUT/r5-7-model-after.txt"
    require_phase model-trigger-changed --expect-value "$after"
    screenshot model-trigger-changed
  else
    echo "r5-7 degraded: seed exposed $count model(s); asserting open/close only" \
      | tee "$OUT/r5-7-degraded.txt" >&2
    ax_press model-picker "$OUT/action-press-model-picker-close.txt" "close model menu"
    require_phase model-menu-closed
    screenshot model-menu-closed
  fi
}

scene_r5_8() {
  want_scene r5-8 || return 0
  trace "r5-8 resize 1080x720 composer/send reachable"
  open_session "$SESSION_A" "r5-8-session"
  resize_window "1080x720" "$OUT/resize-narrow.txt" "narrow"
  dump_frames "$OUT/geometry-narrow.txt"
  require_phase narrow-reachable --frames "$OUT/geometry-narrow.txt"
  screenshot narrow-reachable
  resize_window "1440x1024" "$OUT/resize-restore.txt" "restore"
}

scene_r5_9() {
  want_scene r5-9 || return 0
  trace "r5-9 HID type / shift-return / Return send / cmd-. (modifiers cmd,.) cancel"
  open_session "$SESSION_A" "r5-9-session"
  focus_composer
  composer_set_value "" "$OUT/action-set-value-clear-keyboard.txt"
  hid_type_chars "$KEYBOARD_TEXT"
  require_phase keyboard-typed --expect-value "$KEYBOARD_TEXT"
  screenshot keyboard-typed
  key return shift
  require_phase keyboard-newline --expect-value $'ab\n'
  screenshot keyboard-newline
  composer_set_value "" "$OUT/action-set-value-clear-before-send.txt"
  hid_type_chars "$KEYBOARD_TEXT"
  local seq entries
  seq="$(barrier_field "$ROOT/barriers/timeline_stable" seq)"
  [[ "$seq" =~ ^[0-9]+$ ]] || seq=0
  entries="$(timeline_entries)"
  [[ "$entries" =~ ^[0-9]+$ ]] || entries=0
  key return
  wait_timeline_stable "$seq" "$SESSION_A" $(( entries + 2 )) "$BARRIER_TIMEOUT_SECS" >/dev/null
  require_phase keyboard-sent --expect-contains "$KEYBOARD_TEXT"
  screenshot keyboard-sent
  composer_set_value "$HANG_TEXT" "$OUT/action-set-value-keyboard-hang.txt"
  ax_press send "$OUT/action-press-send-keyboard-hang.txt" "send hang for cmd-."
  require_phase running-cancelable
  # kVK_ANSI_Period = 47; ui-key-event accepts numeric codes.
  key 47 cmd
  require_phase keyboard-cancelled
  screenshot keyboard-cancelled
}


wait_desktop_ready

# 键盘场景对输入法敏感（W-B2 r5-9 实跑取证：第三方拼音 IME 的组合会话
# 在首字符后接管 keyDown，shift+Return / Return 全被 NSTextInputContext
# 吃掉，永远到不了 keymap）。把当前输入源钉到 ASCII（ABC 优先），
# fixture_teardown 负责恢复。
INPUT_SOURCE_BEFORE=""
soft_activate
"$KEY" --pid "$DESKTOP_PID" --pin-ascii-input-source 2> "$WORK/input-source-pin.txt" \
  || { cat "$WORK/input-source-pin.txt" >&2; die "pin ascii input source failed"; }
cp "$WORK/input-source-pin.txt" "$OUT/input-source-pin.txt"
INPUT_SOURCE_BEFORE="$(sed -n 's/^ui-key-event input-source-before=\([^ ]*\).*/\1/p' "$WORK/input-source-pin.txt" | head -1)"
trace "ascii input source pinned (before=${INPUT_SOURCE_BEFORE:-unknown})"

trace "place Desktop window deterministically on the main display"
"$FRAMES" "$DESKTOP_PID" --place-main > "$OUT/window-place.txt" \
  || die "failed to place Desktop window (see $OUT/window-place.txt)"
grep -q '# place-main result=0 ' "$OUT/window-place.txt" \
  || die "Desktop main-display placement did not converge (see $OUT/window-place.txt)"

INITIAL_SEQ="$(wait_timeline_stable 0 "" 0 "$BARRIER_TIMEOUT_SECS")"
trace "initial timeline_stable seq=$INITIAL_SEQ"

scene_r5_1
scene_r5_2
scene_r5_3
scene_r5_4
scene_r5_5
scene_r5_6
scene_r5_8
scene_r5_9
# r5-7 会把“下一轮”模型切到另一 provider；放在所有 fixture 发送场景之后，
# 避免该有意的 UI 状态变更污染 r5-9 的 mock provider 键盘路径。
scene_r5_7

copy_runtime_evidence
# shell-manifest writes run-manifest.json (r4 evidence layout).
"$PY" "$WAVE_D_TOOLS" shell-manifest --dir "$OUT" --repo "$REPO_ROOT" \
  --scenario r5-wave-b-states --label "$LABEL"

trace "teardown fixture root=$ROOT"
"$FIXTURE" down --root "$ROOT"
"$FIXTURE" clean --root "$ROOT"
DESKTOP_PID=""
# 与 fixture_teardown 同理：desktop 退出后恢复输入源；成功路径在 rm $WORK
# 前必须显式恢复一次（trap 路径只兜失败分支）。
if [[ -n "${INPUT_SOURCE_BEFORE:-}" && -x "${KEY:-}" ]]; then
  "$KEY" --pid 1 --restore-input-source "$INPUT_SOURCE_BEFORE" \
    2> "$OUT/input-source-restore.txt" >/dev/null || true
fi
rm -rf "$WORK"
trace "run done (requested composer scenes passed)"
exit 0
