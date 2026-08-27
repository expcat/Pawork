#!/usr/bin/env bash
# Pawork R3 Wave B Slice 3 - U2 real-window keyboard navigation driver.
#
# 场景：seed -> serve -> desktop（隔离 PAWORK_DATA_DIR + barrier 目录）->
# 真窗口键盘/鼠标注入（ui-key-event：全局 HID tap，方向键带 SecondaryFn；
# --click-at 兜底把焦点放进 rail 链）-> 相位断言轮询 -> 截图/AX dump 归档。
#
# 相位（断言在 ui-r3-wave-b-tools.py；证据 ax-tree-<phase>.txt /
# assert-<phase>.json / shot-<phase>.png）：
#   f. next-needs-attention：原初态 cmd-alt-n 跳 fx-ses-beta-pending
#      （help 含 "Needs input"，active 切换，timeline 加载）。
#   a. rail 焦点链走查：Tab/Shift-Tab 真实遍历（composer → scope →
#      grouping → add-task，Shift-Tab 反向回 grouping）；真实点击
#      scope/grouping/add-task/项目头进链（GPUI mouse down 自动转移焦点），
#      ↓ 沿 header -> project-add -> task 行，→ 展开项目。
#   b. key-open-task：行聚焦 + Enter 行级 key_down 激活，断言 active 切换
#      与 timeline 加载（正向，无 cmd-alt-down 兜底）。
#   c. grouping-menu-keyboard：真鼠标点开菜单（聚焦触发器），↓ 移高亮，
#      Enter 选 Projects -> 再开 -> ↓ 环绕 -> Enter 回 Timeline；selection
#      与 timeline 保留。
#   d. cmd-alt cycling：cmd-alt-down/down/up 各步 selected 变化 + 加载。
#   e. disconnected-kept/reconnected-kept：drop-socket 断线保留 selection，
#      AXPress reconnect 恢复。
#   g. blocked-live：project-add 建稿 -> composer AX set-value "fixture:fail"
#      -> send -> run failed 派生 Blocked（行 help 含词）。
#      unread-live：self-check 造后台会话事件 -> 再次断线/重连快照重建后
#      新会话行 help 含 Unread。
#
# 同步只用 barrier/轮询（相位断言即收敛轮询），禁固定 sleep 猜测。
# 清理只删自建 fixture root 与临时工具目录；失败保留证据退出非零。
#
# Usage: scripts/ui-r3-wave-b-nav.sh run --out <dir> [--label <name>]
#
# Exit codes: 0 all phases pass; 2 usage; 3 infrastructure; 4 assertion failure.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
FIXTURE="$SCRIPT_DIR/ui-fixture.sh"
WAVE_D_TOOLS="$SCRIPT_DIR/ui-wave-d-tools.py"
R3B_TOOLS="$SCRIPT_DIR/ui-r3-wave-b-tools.py"
BARRIER_TIMEOUT_SECS="${PAWORK_UI_BARRIER_TIMEOUT_SECS:-180}"
WINDOW_TIMEOUT_SECS="${PAWORK_UI_WINDOW_TIMEOUT_SECS:-120}"
PHASE_TIMEOUT_SECS="${PAWORK_UI_PHASE_TIMEOUT_SECS:-30}"
# 全局兜底：任何未预期的卡死（如 desktop 半死状态导致轮询空转）超过
# 上限时记录失败并走 teardown 清理退出，禁止无限等待（R3 Wave B Slice 4）。
RUN_TIMEOUT_SECS="${PAWORK_UI_RUN_TIMEOUT_SECS:-900}"
PENDING_SESSION="fx-ses-beta-pending"
OPEN_SESSION="fx-ses-beta-toolfailed"
CYCLE_SESSION="fx-ses-beta-long"

die() { echo "ui-r3b-nav: $*" >&2; exit 3; }
usage() {
  echo "usage: scripts/ui-r3-wave-b-nav.sh run --out <dir> [--label <name>]" >&2
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
LABEL="r3-wave-b-nav"
while (( $# )); do
  case "$1" in
    run) MODE="$1"; shift ;;
    --out) OUT="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "ui-r3b-nav: unknown argument $1" >&2; usage ;;
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
[[ -f "$R3B_TOOLS" ]] || die "missing $R3B_TOOLS"
[[ -x "$FIXTURE" ]] || die "missing executable $FIXTURE"

mkdir -p "$OUT/logs" "$OUT/barriers"
OUT="$(cd "$OUT" && pwd)"
TRACE="$OUT/action-trace.txt"
: > "$TRACE"
trace() {
  echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" | tee -a "$TRACE" >&2
}

WORK="$(mktemp -d /tmp/pawork-r3b-nav-tools.XXXXXX)"
ROOT="$(mktemp -d /tmp/pawork-ui-fixture.XXXXXX)"
ROOT="$(cd "$ROOT" && pwd -P)"
DESKTOP_PID=""
copy_runtime_evidence() {
  cp "$ROOT/logs/serve.log" "$OUT/logs/serve.log" 2>/dev/null || true
  cp "$ROOT/logs/desktop.log" "$OUT/logs/desktop.log" 2>/dev/null || true
  cp "$ROOT/logs/self-check.log" "$OUT/logs/self-check.log" 2>/dev/null || true
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
swiftc -O -o "$WORK/ui-key-event" "$SCRIPT_DIR/ui-key-event.swift" 2>"$WORK/swiftc-key.err" \
  || { cat "$WORK/swiftc-key.err" >&2; die "ui-key-event compile failed"; }
AXDUMP="$WORK/ui-ax-dump"
FRAMES="$WORK/ax-frames"
KEY="$WORK/ui-key-event"

trace "seed fixture root=$ROOT"
"$FIXTURE" seed --root "$ROOT"
trace "serve fixture (wait host_ready barrier)"
"$FIXTURE" serve --root "$ROOT"
trace "launch desktop via ui-fixture"
"$FIXTURE" desktop --root "$ROOT"
DESKTOP_PID="$(cat "$ROOT/desktop.pid")"
trace "desktop pid=$DESKTOP_PID"

activate() {
  "$SCRIPT_DIR/ui-focus-switch.sh" activate --pid "$DESKTOP_PID" >/dev/null 2>&1 \
    || die "failed to activate desktop window (pid $DESKTOP_PID)"
}

# 重试路径软激活：真机前台可能被用户/其它应用抢走，CGEventPost 走全局
# HID tap 只会送达前台应用（ui-key-event.swift 契约），发送前不收敛前台
# 会让按键全部漏给别的应用（Slice 4 run2 实证：连发 10 次 Tab 无效）。
# 软失败不 die，交给相位断言/重试或最终超时暴露。
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

# 状态切换瞬间合成键可能被丢（探针实测：菜单挂载同帧发送的 escape/down
# 会丢，稍后重发即生效）。以下 helper 均为收敛驱动重试：发键 -> 短轮询
# 可观测断言 -> 未收敛才重发；不做固定 sleep 猜测。
KEY_RETRY_PROBE_SECS="${PAWORK_UI_KEY_RETRY_PROBE_SECS:-3}"

key_until_phase() { # $1=key $2=phase $3=modifiers(optional)
  local k="$1" phase="$2" mods="${3:-}"
  local deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
  while :; do
    # 先查后发：上一轮按键可能已送达但断言轮询慢了一拍，直接重发会过冲。
    wait_phase "$phase" 1 && return 0
    if [[ -n "$mods" ]]; then key "$k" "$mods"; else key "$k"; fi
    if wait_phase "$phase" "$KEY_RETRY_PROBE_SECS"; then return 0; fi
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited in key_until_phase phase=$phase"
    (( SECONDS < deadline )) || { trace "key_until_phase TIMEOUT phase=$phase key=$k"; return 1; }
  done
}

key_until_tree() { # $1=pattern $2=key $3=label $4=modifiers(optional)
  local pattern="$1" k="$2" label="$3" mods="${4:-}"
  local deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
  while :; do
    wait_tree_contains "$pattern" 1 "$label" && return 0
    if [[ -n "$mods" ]]; then key "$k" "$mods"; else key "$k"; fi
    if wait_tree_contains "$pattern" "$KEY_RETRY_PROBE_SECS" "$label"; then return 0; fi
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited in key_until_tree $label"
    (( SECONDS < deadline )) || { trace "key_until_tree TIMEOUT label=$label"; return 1; }
  done
}

se_escape() { # System Events keystroke escape：合成 Escape 经 CGEvent 任何源都会被
  # AppKit 路由到 cancelOperation: 而非 keyDown:，探针实测只有 SE 注入可达
  # GPUI 根节点的 escape 处理（仍为 OS 级真实键盘注入，作用于前台窗口）。
  "$SCRIPT_DIR/ui-focus-switch.sh" activate --pid "$DESKTOP_PID" >/dev/null 2>&1 || true
  osascript -e 'tell application "System Events" to key code 53' >/dev/null 2>&1 || true
}

escape_close() { # $1=menu identifier $2=label — 菜单已关后 escape 在根节点为 no-op，可安全重发
  local id="$1" label="$2"
  local deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
  while :; do
    se_escape
    if wait_tree_contains "identifier=\"$id\"" "$KEY_RETRY_PROBE_SECS" "$label" 1; then return 0; fi
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited in escape_close $label"
    (( SECONDS < deadline )) || { trace "escape_close TIMEOUT label=$label"; return 1; }
  done
}

enter_select() { # $1=menu identifier $2=phase — 菜单已关后重发 enter 会在聚焦的
  # 触发器上合成 click 重开菜单，故仅当菜单仍开（上次 enter 被丢）才重发。
  local id="$1" phase="$2"
  local deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
  while :; do
    wait_phase "$phase" 1 && return 0
    key return
    if wait_phase "$phase" "$KEY_RETRY_PROBE_SECS"; then return 0; fi
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited in enter_select phase=$phase"
    (( SECONDS < deadline )) || { trace "enter_select TIMEOUT phase=$phase"; return 1; }
    dump_tree "$WORK/enter-select.txt"
    if ! grep -q "identifier=\"$id\"" "$WORK/enter-select.txt"; then
      trace "enter_select: menu $id closed but phase=$phase unasserted; no resend (would reopen)"
      wait_phase "$phase" 5 && return 0
      return 1
    fi
  done
}

frame_center() { # $1=identifier -> stdout "x y"
  local line x y w h
  line="$("$FRAMES" "$DESKTOP_PID" 2>/dev/null | grep -m1 "^id=$1 ")"
  [[ -n "$line" ]] || return 1
  x="$(printf '%s' "$line" | sed -E 's/.*x=([0-9.]+).*/\1/')"
  y="$(printf '%s' "$line" | sed -E 's/.*y=([0-9.]+).*/\1/')"
  w="$(printf '%s' "$line" | sed -E 's/.*w=([0-9.]+).*/\1/')"
  h="$(printf '%s' "$line" | sed -E 's/.*h=([0-9.]+).*/\1/')"
  awk -v x="$x" -v y="$y" -v w="$w" -v h="$h" 'BEGIN { printf "%.1f,%.1f", x + w / 2, y + h / 2 }'
}

click_id() { # $1=identifier
  local center deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
  while :; do
    if center="$(frame_center "$1")"; then
      break
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited waiting for frame: $1"
    (( SECONDS < deadline )) || die "no AX frame for click target: $1"
    sleep 0.2
  done
  trace "click $1 at $center"
  soft_activate
  "$KEY" --pid "$DESKTOP_PID" --click-at "$center" >/dev/null
}

click_until_tree() { # $1=identifier $2=pattern $3=label — 效果驱动重试点击
  local id="$1" pattern="$2" label="$3"
  local deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
  while :; do
    wait_tree_contains "$pattern" 1 "$label" && return 0
    click_id "$id"
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited in click_until_tree $label"
    (( SECONDS < deadline )) || { trace "click_until_tree TIMEOUT label=$label"; return 1; }
  done
}

ax_press() { # $1=identifier $2=evidence-file $3=label
  trace "AXPress $1 ($3)"
  "$AXDUMP" --pid "$DESKTOP_PID" --press "$1" --action-only --out "$2" >/dev/null \
    || die "AXPress $1 failed (see $2)"
  grep -q 'result=0' "$2" || die "AXPress $1 result!=0 (see $2)"
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

wait_timeline_stable() { # $1=min_seq(exclusive) $2=want_session $3=timeout_secs -> stdout seq
  local min_seq="$1" want_session="$2" timeout_secs="$3"
  local deadline=$(( SECONDS + timeout_secs ))
  local seq session
  trace "wait timeline_stable (seq>$min_seq session=$want_session)"
  while :; do
    seq="$(barrier_field "$ROOT/barriers/timeline_stable" seq)"
    session="$(barrier_field "$ROOT/barriers/timeline_stable" session)"
    if [[ "$seq" =~ ^[0-9]+$ ]] && (( seq > min_seq )) && [[ "$session" == "$want_session" ]]; then
      trace "timeline_stable ok seq=$seq session=$session"
      printf '%s' "$seq"
      return 0
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited while waiting timeline_stable"
    (( SECONDS < deadline )) || die "timeout waiting timeline_stable session=$want_session"
    sleep 0.2
  done
}

wait_timeline_stable_soft() { # 同上，但超时/进程退出返回 1 而非 die（供重试包装）
  local min_seq="$1" want_session="$2" timeout_secs="$3"
  local deadline=$(( SECONDS + timeout_secs ))
  local seq session
  while :; do
    seq="$(barrier_field "$ROOT/barriers/timeline_stable" seq)"
    session="$(barrier_field "$ROOT/barriers/timeline_stable" session)"
    if [[ "$seq" =~ ^[0-9]+$ ]] && (( seq > min_seq )) && [[ "$session" == "$want_session" ]]; then
      printf '%s' "$seq"
      return 0
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null || return 1
    (( SECONDS < deadline )) || return 1
    sleep 0.2
  done
}

cycle_key() { # $1=plain key $2=min_seq(exclusive) $3=want_session -> stdout seq
  # cmd-alt 键以 barrier seq 推进为收敛信号；未推进则重发（重发只会在
  # 同一 cycling 语义下前进，若过冲由后续 phase 断言以证据暴露）。
  local k="$1" min_seq="$2" want="$3"
  local deadline=$(( SECONDS + BARRIER_TIMEOUT_SECS ))
  local seq=""
  while :; do
    key "$k" cmd,alt
    if seq="$(wait_timeline_stable_soft "$min_seq" "$want" "$KEY_RETRY_PROBE_SECS")"; then
      printf '%s' "$seq"
      return 0
    fi
    kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited in cycle_key key=$k"
    (( SECONDS < deadline )) || die "cycle_key TIMEOUT key=$k session=$want"
  done
}

wait_phase() { # $1=phase $2=timeout_secs(optional)
  local phase="$1" timeout_secs="${2:-$PHASE_TIMEOUT_SECS}"
  local tree="$OUT/ax-tree-$phase.txt"
  local json="$OUT/assert-$phase.json"
  local deadline=$(( SECONDS + timeout_secs ))
  local attempt=0 dump_rc assert_rc
  trace "wait phase=$phase (assertion polling)"
  while :; do
    attempt=$(( attempt + 1 ))
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --max-depth 16 --out "$tree" >/dev/null 2>&1
    dump_rc=$?
    "$PY" "$R3B_TOOLS" assert --tree "$tree" --phase "$phase" --out "$json" >/dev/null 2>&1
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

require_phase() { # $1=phase
  wait_phase "$1" || {
    echo "ui-r3b-nav: structural assertion failed phase=$1 (evidence kept: $OUT)" >&2
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

screenshot() { # $1=phase
  local wid
  "$AXDUMP" --pid "$DESKTOP_PID" --out "$WORK/wid-$1.txt" --wid-out "$WORK/wid-$1.id" >/dev/null 2>&1 \
    || die "AX dump for wid failed (phase $1)"
  wid="$(cat "$WORK/wid-$1.id")"
  screencapture -x -o -l "$wid" "$OUT/shot-$1.png" || die "screencapture failed (phase $1, wid=$wid)"
  [[ -s "$OUT/shot-$1.png" ]] || die "screenshot empty (phase $1, wid=$wid)"
  trace "screenshot $OUT/shot-$1.png"
}

wait_desktop_ready

trace "place Desktop window deterministically on the main display"
"$FRAMES" "$DESKTOP_PID" --place-main > "$OUT/window-place.txt" \
  || die "failed to place Desktop window (see $OUT/window-place.txt)"
grep -q '# place-main result=0 ' "$OUT/window-place.txt" \
  || die "Desktop main-display placement did not converge (see $OUT/window-place.txt)"

INITIAL_SEQ="$(wait_timeline_stable 0 "" "$BARRIER_TIMEOUT_SECS")"
activate

# ---------------------------------------------------------------- f. next-needs-attention
trace "phase f: cmd-alt-n jumps to Needs input session (pristine state)"
PENDING_SEQ="$(cycle_key n "$INITIAL_SEQ" "$PENDING_SESSION")"
require_phase next-needs-attention
screenshot next-needs-attention

# ---------------------------------------------------------------- a. rail focus walk
trace "phase a: Tab traverses design 3.6 focus chain (composer -> scope -> grouping -> add-task)"
dump_tree "$OUT/ax-tree-before-tab.txt"
key_until_phase tab tab-traverse-scope \
  || { echo "ui-r3b-nav: Tab did not focus scope trigger (evidence kept: $OUT)" >&2; exit 4; }
key_until_phase tab tab-traverse-grouping \
  || { echo "ui-r3b-nav: Tab did not advance to grouping trigger (evidence kept: $OUT)" >&2; exit 4; }
key_until_phase tab tab-traverse-add-task \
  || { echo "ui-r3b-nav: Tab did not advance to add-task (evidence kept: $OUT)" >&2; exit 4; }
trace "phase a: Shift-Tab reverses focus to grouping"
key_until_phase tab tab-reverse-grouping shift \
  || { echo "ui-r3b-nav: Shift-Tab did not reverse to grouping (evidence kept: $OUT)" >&2; exit 4; }
screenshot tab-traverse

# ---------------------------------------------------------------- a2. button-enter (Slice 5 P2b)
trace "phase a2: Tab 聚焦 rail 触发器后裸 Enter 打开菜单 / 浮层（不用 click_id）"
key_until_phase return button-enter-grouping-menu || { echo "ui-r3b-nav: bare Enter on grouping trigger did not open menu (evidence kept: $OUT)" >&2; exit 4; }
escape_close grouping-menu grouping-menu-closed-a2 || die "grouping menu did not close via Escape after button-enter"
key_until_phase tab tab-traverse-add-task || { echo "ui-r3b-nav: Tab did not advance to add-task (evidence kept: $OUT)" >&2; exit 4; }
key_until_phase return button-enter-add-task-popover || { echo "ui-r3b-nav: bare Enter on add-task did not open workspace confirm (evidence kept: $OUT)" >&2; exit 4; }
escape_close workspace-confirm add-task-popover-closed-a2 || die "workspace confirm popover did not close via Escape after button-enter"
key_until_phase tab tab-reverse-grouping shift || { echo "ui-r3b-nav: Shift-Tab did not reverse to grouping (evidence kept: $OUT)" >&2; exit 4; }
key_until_phase tab tab-reverse-scope shift || { echo "ui-r3b-nav: Shift-Tab did not reverse to scope (evidence kept: $OUT)" >&2; exit 4; }
key_until_phase return button-enter-scope-menu || { echo "ui-r3b-nav: bare Enter on scope trigger did not open menu (evidence kept: $OUT)" >&2; exit 4; }
escape_close scope-menu scope-menu-closed-a2 || die "scope menu did not close via Escape after button-enter"
screenshot button-enter

trace "phase a: click scope (opens scope menu; GPUI mouse-down moves focus)"
activate
click_id project-scope
wait_tree_contains 'identifier="scope-menu"' "$PHASE_TIMEOUT_SECS" scope-menu-open \
  || { echo "ui-r3b-nav: scope menu did not open (evidence kept: $OUT)" >&2; exit 4; }
cp "$WORK/wait-tree.txt" "$OUT/ax-tree-scope-menu.txt"
escape_close scope-menu scope-menu-closed \
  || die "scope menu did not close via Escape"

trace "phase a: click grouping trigger (menu opens; trigger keeps focus)"
click_id task-rail-grouping
wait_tree_contains 'identifier="grouping-menu"' "$PHASE_TIMEOUT_SECS" grouping-menu-open \
  || { echo "ui-r3b-nav: grouping menu did not open (evidence kept: $OUT)" >&2; exit 4; }
cp "$WORK/wait-tree.txt" "$OUT/ax-tree-grouping-menu-a.txt"
escape_close grouping-menu grouping-menu-closed-a \
  || die "grouping menu did not close via Escape"

trace "phase a: click add-task (workspace confirm popover + published focus)"
click_id add-task
require_phase rail-focus-add-task
cp "$OUT/ax-tree-rail-focus-add-task.txt" "$OUT/ax-tree-add-task-focus.txt"
escape_close workspace-confirm add-task-popover-closed \
  || die "workspace confirm popover did not close via Escape"

trace "phase a: click alpha header (focus enters rail list chain; click collapses)"
click_id project-Earlier_3afx-alpha-app
require_phase rail-focus-alpha-header
key_until_phase down rail-focus-alpha-add \
  || { echo "ui-r3b-nav: rail focus chain broke at alpha-add (evidence kept: $OUT)" >&2; exit 4; }
key_until_phase down rail-focus-beta-header \
  || { echo "ui-r3b-nav: rail focus chain broke at beta-header (evidence kept: $OUT)" >&2; exit 4; }
key_until_phase right rail-expand-beta \
  || { echo "ui-r3b-nav: rail focus chain broke at expand-beta (evidence kept: $OUT)" >&2; exit 4; }
key_until_phase down rail-focus-beta-add \
  || { echo "ui-r3b-nav: rail focus chain broke at beta-add (evidence kept: $OUT)" >&2; exit 4; }
key_until_phase down rail-focus-task \
  || { echo "ui-r3b-nav: rail focus chain broke at task row (evidence kept: $OUT)" >&2; exit 4; }
screenshot rail-focus-walk

# ---------------------------------------------------------------- b. key-open-task
trace "phase b: focus task row, Enter activates it via row-level key_down"
key_until_tree "identifier=\"session-$OPEN_SESSION\"[^#]*focused=1" down open-row-focused \
  || { echo "ui-r3b-nav: open target row did not gain focus (evidence kept: $OUT)" >&2; exit 4; }
cp "$WORK/wait-tree.txt" "$OUT/ax-tree-open-row-focused.txt"
key_until_phase return key-open-task \
  || { echo "ui-r3b-nav: Enter did not open focused row (evidence kept: $OUT)" >&2; exit 4; }
OPEN_SEQ="$(wait_timeline_stable "$PENDING_SEQ" "$OPEN_SESSION" "$BARRIER_TIMEOUT_SECS")"
require_phase key-open-task
screenshot key-open-task
printf '{"enter_gap": 0, "activation": "row-key-down", "target": "%s"}\n' \
  "$OPEN_SESSION" > "$OUT/enter-gap.json"

# ---------------------------------------------------------------- c. grouping menu keyboard
trace "phase c: grouping menu keyboard Timeline -> Projects"
activate
click_id task-rail-grouping
wait_tree_contains 'identifier="grouping-menu"' "$PHASE_TIMEOUT_SECS" grouping-menu-open-c \
  || { echo "ui-r3b-nav: grouping menu did not open (evidence kept: $OUT)" >&2; exit 4; }
cp "$WORK/wait-tree.txt" "$OUT/ax-tree-grouping-menu-c.txt"
key_until_tree 'identifier="group-projects"[^#]*focused=1' down projects-highlighted \
  || { echo "ui-r3b-nav: projects menu item not highlighted (evidence kept: $OUT)" >&2; exit 4; }
cp "$WORK/wait-tree.txt" "$OUT/ax-tree-grouping-highlight-projects.txt"
enter_select grouping-menu grouping-projects-keyboard \
  || { echo "ui-r3b-nav: keyboard Projects selection failed (evidence kept: $OUT)" >&2; exit 4; }
screenshot grouping-projects-keyboard

trace "phase c: grouping menu keyboard Projects -> Timeline (wrap via down)"
click_id task-rail-grouping
wait_tree_contains 'identifier="grouping-menu"' "$PHASE_TIMEOUT_SECS" grouping-menu-open-c2 \
  || { echo "ui-r3b-nav: grouping menu did not reopen (evidence kept: $OUT)" >&2; exit 4; }
cp "$WORK/wait-tree.txt" "$OUT/ax-tree-grouping-menu-c2.txt"
key_until_tree 'identifier="group-timeline"[^#]*focused=1' down timeline-highlighted \
  || { echo "ui-r3b-nav: timeline menu item not highlighted (evidence kept: $OUT)" >&2; exit 4; }
cp "$WORK/wait-tree.txt" "$OUT/ax-tree-grouping-highlight-timeline.txt"
enter_select grouping-menu grouping-timeline-keyboard \
  || { echo "ui-r3b-nav: keyboard Timeline selection failed (evidence kept: $OUT)" >&2; exit 4; }
screenshot grouping-timeline-keyboard

# ---------------------------------------------------------------- d. cmd-alt cycling
trace "phase d: cmd-alt-down / cmd-alt-down / cmd-alt-up cycling"
CYCLE1_SEQ="$(cycle_key down "$OPEN_SEQ" "$CYCLE_SESSION")"
require_phase cycle-down-1
CYCLE2_SEQ="$(cycle_key down "$CYCLE1_SEQ" fx-ses-beta-cancelled)"
require_phase cycle-down-2
cycle_key up "$CYCLE2_SEQ" "$CYCLE_SESSION" >/dev/null
require_phase cycle-up
screenshot cmd-alt-cycling

# ---------------------------------------------------------------- e. disconnect / reconnect
trace "phase e: drop socket, selection kept, reconnect"
"$FIXTURE" drop-socket --root "$ROOT" >/dev/null
require_phase disconnected-kept
screenshot disconnected-kept
ax_press reconnect "$OUT/action-press-reconnect.txt" "reconnect after drop"
require_phase reconnected-kept
screenshot reconnected-kept

# ---------------------------------------------------------------- g1. blocked (live derivation)
trace "phase g1: create task, send fixture:fail, assert Blocked derivation"
ax_press project-add-Earlier_3afx-alpha-app "$OUT/action-press-project-add.txt" "create task in alpha-app"
wait_tree_contains 'identifier="composer-input"[^#]*focused=1' "$PHASE_TIMEOUT_SECS" draft-composer-focused \
  || { echo "ui-r3b-nav: draft composer not focused (evidence kept: $OUT)" >&2; exit 4; }
cp "$WORK/wait-tree.txt" "$OUT/ax-tree-blocked-draft.txt"
"$AXDUMP" --pid "$DESKTOP_PID" --set-value composer-input "fixture:fail" \
  --action-only --out "$OUT/action-set-value-fixture-fail.txt" >/dev/null \
  || die "AX set-value composer-input failed (see $OUT/action-set-value-fixture-fail.txt)"
ax_press send "$OUT/action-press-send.txt" "send fixture:fail"
# Blocked 行落在新会话的 Today 桶下；phase a 折叠过 alpha 工作区（折叠键
# 按 workspace 粒度，新 Today/alpha 组继承 Collapsed），点开 Today 头让
# Blocked 行进入 AX 树（选择/激活状态不受影响；效果=Today 组 Expanded）。
click_until_tree project-Today_3afx-alpha-app \
  'identifier="project-Today_3afx-alpha-app"[^#]*help="Expanded"' today-alpha-expanded \
  || die "failed to expand Today alpha group for Blocked row visibility"
require_phase blocked-live
screenshot blocked-live

# ---------------------------------------------------------------- g2. unread (background session)
trace "phase g2: disconnect first, self-check in background, reconnect publishes Unread row"
# 顺序关键：桌面在线时会实时收到 self-check 事件（client_seq 追平
# server），重连走 UpToDate->Unchanged，新会话行永远进不了 rail。先断线
# 让 self-check 事件仅在服务端存在，重连才走 Replay + snapshot merge，
# Unread 由重放派生。
"$FIXTURE" drop-socket --root "$ROOT" >/dev/null
wait_tree_contains 'value="[^"]*Disconnected[^"]*" identifier="connection-status"' "$PHASE_TIMEOUT_SECS" disconnect2-observed \
  || { echo "ui-r3b-nav: second disconnect not observed (evidence kept: $OUT)" >&2; exit 4; }
cp "$WORK/wait-tree.txt" "$OUT/ax-tree-disconnect2.txt"
"$FIXTURE" self-check --root "$ROOT" >/dev/null
ax_press reconnect "$OUT/action-press-reconnect2.txt" "reconnect for unread snapshot"
require_phase unread-live
screenshot unread-live

copy_runtime_evidence
"$PY" "$WAVE_D_TOOLS" shell-manifest --dir "$OUT" --repo "$REPO_ROOT" \
  --scenario r3-wave-b-nav --label "$LABEL"

trace "teardown fixture root=$ROOT"
"$FIXTURE" down --root "$ROOT"
"$FIXTURE" clean --root "$ROOT"
DESKTOP_PID=""
rm -rf "$WORK"
trace "run done (all phases pass)"
exit 0
