#!/usr/bin/env bash
# Pawork R7 Wave A - State A hover/active/focus 补充证据 driver。
#
# 场景：seed -> serve -> desktop（ui-fixture 隔离 root）-> place-main ->
# 以 AX frames 坐标驱动 hover / press-hold(active) / focus 三类补充图：
#   hover : task-rail-grouping / session 行 / model-picker /
#           inspector-tab-terminal / send（disabled 可见性）
#   active: task-rail-grouping 按住（截后 release，菜单开后 Escape 关）
#   focus : session 行 click 聚焦选中 / composer-input click 聚焦
# Tab 链焦点图沿用 u2 nav driver 归档（shot-tab-traverse.png），不重复采集。
#
# 同步只用 barrier/AX dump 轮询；清理只删自建 fixture root 与临时工具目录。
#
# Usage: scripts/ui-r7-wave-a-states.sh run --out <dir> [--label <name>]
# Exit: 0 证据成套；2 usage；3 infrastructure；4 采集失败。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
FIXTURE="$SCRIPT_DIR/ui-fixture.sh"
KEYEVENT_SRC="$SCRIPT_DIR/ui-key-event.swift"
AXDUMP_SRC="$SCRIPT_DIR/ui-ax-dump.swift"
FRAMES_SRC="$SCRIPT_DIR/ui-ax-frames.swift"
FOCUS_SWITCH="$SCRIPT_DIR/ui-focus-switch.sh"
WINDOW_TIMEOUT_SECS="${PAWORK_UI_WINDOW_TIMEOUT_SECS:-120}"
PHASE_TIMEOUT_SECS="${PAWORK_UI_PHASE_TIMEOUT_SECS:-30}"
RUN_TIMEOUT_SECS="${PAWORK_UI_RUN_TIMEOUT_SECS:-600}"
SESSION_ROW_ID="session-fx-ses-beta-pending"

die() { echo "ui-r7a-states: $*" >&2; exit 3; }
usage() {
  echo "usage: scripts/ui-r7-wave-a-states.sh run --out <new-or-empty-dir> [--label <name>]" >&2
  exit 2
}

MODE=""
OUT=""
LABEL="r7-wave-a-states"
while (( $# )); do
  case "$1" in
    run) MODE="$1"; shift ;;
    --out) OUT="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done
[[ "$MODE" == run && -n "$OUT" ]] || usage
[[ -x "$FIXTURE" ]] || die "missing executable $FIXTURE"
[[ -f "$KEYEVENT_SRC" ]] || die "missing $KEYEVENT_SRC"
[[ -f "$AXDUMP_SRC" ]] || die "missing $AXDUMP_SRC"
[[ -f "$FRAMES_SRC" ]] || die "missing $FRAMES_SRC"
[[ -x "$FOCUS_SWITCH" ]] || die "missing executable $FOCUS_SWITCH"
for tool in cargo swiftc screencapture python3 osascript codesign plutil; do
  command -v "$tool" >/dev/null 2>&1 || die "required tool not found: $tool"
done
if [[ -e "$OUT" && ! -d "$OUT" ]]; then die "--out must be a directory: $OUT"; fi
if [[ -d "$OUT" ]]; then
  shopt -s nullglob dotglob; entries=("$OUT"/*); shopt -u nullglob dotglob
  (( ${#entries[@]} == 0 )) || die "--out must be new or empty: $OUT"
fi

mkdir -p "$OUT/logs" "$OUT/barriers"
OUT="$(cd "$OUT" && pwd -P)"
TRACE="$OUT/action-trace.txt"
: > "$TRACE"
trace() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" | tee -a "$TRACE" >&2; }

WORK="$(mktemp -d /tmp/pawork-r7a-states.XXXXXX)"
ROOT="$(mktemp -d /tmp/pawork-ui-fixture.XXXXXX)"
ROOT="$(cd "$ROOT" && pwd -P)"
DESKTOP_PID=""
RUN_WATCHDOG=""
teardown() {
  local status=$?
  trap - EXIT
  kill "${RUN_WATCHDOG:-}" 2>/dev/null || true
  cp "$ROOT/logs/serve.log" "$OUT/logs/serve.log" 2>/dev/null || true
  cp "$ROOT/logs/desktop.log" "$OUT/logs/desktop.log" 2>/dev/null || true
  for barrier_file in "$ROOT"/barriers/*; do
    [[ -f "$barrier_file" ]] && cp "$barrier_file" "$OUT/barriers/" 2>/dev/null || true
  done
  unset PAWORK_DATA_DIR
  if [[ -n "${DESKTOP_PID:-}" ]] || [[ -f "$ROOT/desktop.pid" ]]; then
    "$FIXTURE" down --root "$ROOT" >/dev/null 2>&1 || true
  fi
  "$FIXTURE" clean --root "$ROOT" >/dev/null 2>&1 || true
  rm -rf "$WORK"
  exit $status
}
trap teardown EXIT
( sleep "$RUN_TIMEOUT_SECS"; kill -TERM "$$" 2>/dev/null || true ) >/dev/null 2>&1 &
RUN_WATCHDOG=$!

trace "compile helpers into $WORK"
swiftc -O -o "$WORK/ui-key-event" "$KEYEVENT_SRC" 2> "$WORK/key-event.err" \
  || { cp "$WORK/key-event.err" "$OUT/"; die "ui-key-event compile failed"; }
swiftc -O -o "$WORK/ui-ax-dump" "$AXDUMP_SRC" 2> "$WORK/axdump.err" \
  || { cp "$WORK/axdump.err" "$OUT/"; die "ui-ax-dump compile failed"; }
swiftc -O -o "$WORK/ui-ax-frames" "$FRAMES_SRC" 2> "$WORK/frames.err" \
  || { cp "$WORK/frames.err" "$OUT/"; die "ui-ax-frames compile failed"; }
KEYEVENT="$WORK/ui-key-event"; AXDUMP="$WORK/ui-ax-dump"; FRAMES="$WORK/ui-ax-frames"

trace "build Desktop once"
(cd "$REPO_ROOT" && cargo build -p pawork-desktop --offline --features gpui/runtime_shaders --bin pawork-desktop) >/dev/null

# 已知平台事实（ax-forms 对照 + macOS 26.6.2）：裸 debug 二进制的 AX 注册
# 间歇不可用。State 采集要求稳定 AX 窗口，因此与 ax-forms 相同：同一
# build payload 装入最小 .app 并 ad-hoc 签名后启动；形态与哈希进 manifest。
BUILD_BIN="$REPO_ROOT/target/debug/pawork-desktop"
APP="$WORK/Pawork State Supplement.app"
APP_EXEC="$APP/Contents/MacOS/pawork-desktop"
mkdir -p "$APP/Contents/MacOS"
cp -p "$BUILD_BIN" "$APP_EXEC"
BUILD_SHA="$(shasum -a 256 "$BUILD_BIN" | awk '{print $1}')"
APP_PRE_SIGN_SHA="$(shasum -a 256 "$APP_EXEC" | awk '{print $1}')"
[[ "$APP_PRE_SIGN_SHA" == "$BUILD_SHA" ]] || die "bundle executable differs from Desktop build output before signing"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>pawork-desktop</string>
  <key>CFBundleIdentifier</key><string>dev.pawork.desktop.state-supplement</string>
  <key>CFBundleName</key><string>Pawork</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleShortVersionString</key><string>0.0.0-r7</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" > "$OUT/plutil.txt" \
  || die "bundle Info.plist invalid (see plutil.txt)"
codesign --force --sign - "$APP" > "$OUT/codesign-sign.txt" 2>&1 \
  || die "ad-hoc codesign failed (see codesign-sign.txt)"
codesign --verify --deep --strict "$APP" >> "$OUT/codesign-sign.txt" 2>&1 \
  || die "signed bundle verification failed (see codesign-sign.txt)"
LAUNCH_FORM="bundled-adhoc"
LAUNCH_PATH="$APP_EXEC"
APP_SHA="$(shasum -a 256 "$APP_EXEC" | awk '{print $1}')"

trace "seed fixture root=$ROOT"
"$FIXTURE" seed --root "$ROOT"
trace "serve fixture (wait host_ready barrier)"
"$FIXTURE" serve --root "$ROOT" &
SERVE_PID=$!
deadline=$(( SECONDS + WINDOW_TIMEOUT_SECS ))
until [[ -f "$ROOT/barriers/host_ready" ]] || (( SECONDS >= deadline )); do
  kill -0 "$SERVE_PID" 2>/dev/null || die "fixture serve exited early"
  sleep 0.2
done
[[ -f "$ROOT/barriers/host_ready" ]] || die "host_ready barrier timeout"

trace "launch desktop via ui-fixture"
PAWORK_UI_DESKTOP_BIN="$LAUNCH_PATH" "$FIXTURE" desktop --root "$ROOT"
DESKTOP_PID="$(cat "$ROOT/desktop.pid")"

is_ax_recursion() { # $1=probe-file: AX server registration failure signature
  local probe="$1" ax_app_lines
  [[ -f "$probe" ]] || return 1
  ax_app_lines="$(grep -c 'role=AXApplication' "$probe" 2>/dev/null || true)"
  [[ "$ax_app_lines" =~ ^[0-9]+$ ]] || ax_app_lines=0
  if (( ax_app_lines >= 3 )) && grep -q '# identifiers (none)' "$probe"; then
    return 0
  fi
  # macOS 26.x degraded registration: deep AXApplication recursion chain whose
  # identifiers are only system menu items (e.g. _forceQuitRequested), no session-list.
  if (( ax_app_lines >= 5 )) && ! grep -q 'identifier="session-list"' "$probe"; then
    return 0
  fi
  grep -q '^# WARN ax-fallback=axwindows' "$probe" \
    && ! grep -q 'identifier="session-list"' "$probe"
}

desktop_alive_or_die() {
  kill -0 "$DESKTOP_PID" 2>/dev/null \
    || { tail -5 "$ROOT/logs/desktop.log" >&2 || true; die "desktop exited early (AX ready polling)"; }
}

wait_desktop_ready() { # AX session-list 可见；递归签名走 desktop-restart<=3（与 r6 driver 同口径）
  local deadline=$(( SECONDS + WINDOW_TIMEOUT_SECS ))
  local probe_attempts=0 probe_rc ax_restarts=0
  while :; do
    set +e
    "$AXDUMP" --pid "$DESKTOP_PID" --out "$WORK/probe.txt" >/dev/null 2>&1
    probe_rc=$?
    set -e
    if (( probe_rc == 0 )) && grep -q 'identifier="session-list"' "$WORK/probe.txt"; then
      trace "desktop window ready (session-list rendered; AX registered)"
      return 0
    fi
    probe_attempts=$(( probe_attempts + 1 ))
    if (( probe_attempts % 10 == 1 )); then
      trace "probe not-ready rc=$probe_rc attempt=$probe_attempts windows=$(grep -c 'wid=' "$WORK/probe.txt" 2>/dev/null)"
    fi
    if (( probe_attempts >= 30 )) && is_ax_recursion "$WORK/probe.txt"; then
      if (( ax_restarts >= 3 )); then
        cp "$WORK/probe.txt" "$OUT/ax-tree-probe-recursive-final.txt" 2>/dev/null || true
        die "AX recursion persisted after $ax_restarts desktop restarts (evidence in $OUT)"
      fi
      ax_restarts=$(( ax_restarts + 1 ))
      trace "AX recursion signature detected (not-ready=$probe_attempts); desktop-restart $ax_restarts/3"
      PAWORK_UI_DESKTOP_BIN="$LAUNCH_PATH" "$FIXTURE" desktop-restart --root "$ROOT"
      DESKTOP_PID="$(cat "$ROOT/desktop.pid")"
      trace "desktop restarted pid=$DESKTOP_PID (AX ready counter reset)"
      probe_attempts=0
    fi
    desktop_alive_or_die
    if (( SECONDS >= deadline )); then
      cp "$WORK/probe.txt" "$OUT/ax-tree-probe-timeout.txt" 2>/dev/null || true
      die "timeout waiting for desktop window/session-list (evidence in $OUT)"
    fi
    sleep 0.3
  done
}

wait_desktop_ready

place_main_or_die() { # AX 间歇劣化时单次调用可能阻塞：看门狗 + 有界重试
  local attempt rc frames_pid dog
  for attempt in 1 2 3; do
    "$FRAMES" "$DESKTOP_PID" --place-main > "$OUT/window-place.txt" &
    frames_pid=$!
    ( sleep 30; kill -9 "$frames_pid" 2>/dev/null ) &
    dog=$!
    set +e
    wait "$frames_pid"
    rc=$?
    kill "$dog" 2>/dev/null || true
    pkill -P "$dog" 2>/dev/null || true
    wait "$dog" 2>/dev/null || true
    set -e
    if (( rc == 0 )) && grep -q '# place-main result=0 ' "$OUT/window-place.txt"; then
      if (( attempt > 1 )); then trace "place-main converged on attempt $attempt"; fi
      return 0
    fi
    trace "place-main attempt $attempt failed (rc=$rc); archiving AX probe"
    "$AXDUMP" --pid "$DESKTOP_PID" --out "$WORK/place-main-fail-$attempt.txt" >/dev/null 2>&1 || true
    cp "$WORK/place-main-fail-$attempt.txt" "$OUT/" 2>/dev/null || true
    sleep 1
  done
  die "Desktop placement did not converge after 3 attempts (evidence in $OUT)"
}

deadline=$(( SECONDS + WINDOW_TIMEOUT_SECS ))
until [[ -f "$ROOT/barriers/timeline_stable" ]] \
  || ! kill -0 "$DESKTOP_PID" 2>/dev/null || (( SECONDS >= deadline )); do
  sleep 0.2
done
[[ -f "$ROOT/barriers/timeline_stable" ]] || die "timeline_stable barrier timeout"
kill -0 "$DESKTOP_PID" 2>/dev/null || die "desktop exited during startup"

trace "place Desktop window deterministically on the main display"
place_main_or_die

dump_tree() { # $1=label
  PAWORK_AXDUMP_TIMEOUT="$PHASE_TIMEOUT_SECS" python3 -c 'import os, signal, subprocess, sys
proc = subprocess.Popen(sys.argv[1:], start_new_session=True)
try:
    raise SystemExit(proc.wait(timeout=float(os.environ["PAWORK_AXDUMP_TIMEOUT"])))
except subprocess.TimeoutExpired:
    os.killpg(proc.pid, signal.SIGKILL)
    proc.wait()
    raise SystemExit(124)' "$AXDUMP" --pid "$DESKTOP_PID" --out "$WORK/tree-$1.txt" --wid-out "$WORK/wid-$1.id"
  local rc=$?
  (( rc == 0 )) || die "AX dump failed (phase $1 rc=$rc)"
  [[ -s "$WORK/tree-$1.txt" ]] || die "AX dump empty (phase $1)"
}
activate() {
  "$FOCUS_SWITCH" activate --pid "$DESKTOP_PID" >/dev/null 2>&1 || true
}
ax_focus() { # $1=identifier
  PAWORK_AXDUMP_TIMEOUT="$PHASE_TIMEOUT_SECS" python3 -c 'import os, signal, subprocess, sys
proc = subprocess.Popen(sys.argv[1:], start_new_session=True)
try:
    raise SystemExit(proc.wait(timeout=float(os.environ["PAWORK_AXDUMP_TIMEOUT"])))
except subprocess.TimeoutExpired:
    os.killpg(proc.pid, signal.SIGKILL)
    proc.wait()
    raise SystemExit(124)' "$AXDUMP" --pid "$DESKTOP_PID" --focus "$1" --out "$WORK/tree-current.txt" --wid-out "$WORK/wid-current.id"
}
shot() { # $1=label
  local wid
  wid="$(cat "$WORK/wid-current.id")"
  screencapture -x -o -l "$wid" "$OUT/shot-$1.png" || die "screencapture failed ($1)"
  [[ -s "$OUT/shot-$1.png" ]] || die "screenshot empty ($1)"
}
center_of() { # $1=identifier -> "x,y"（屏幕坐标；geometry.txt 单一来源）
  awk -v id="id=$1" '
    $1 == id {
      for (i = 2; i <= NF; i++) {
        if ($i ~ /^x=/) x = substr($i, 3)
        if ($i ~ /^y=/) y = substr($i, 3)
        if ($i ~ /^w=/) w = substr($i, 3)
        if ($i ~ /^h=/) h = substr($i, 3)
      }
      printf "%.1f,%.1f", x + w / 2, y + h / 2
      found = 1
    }
    END { exit found ? 0 : 1 }
  ' "$OUT/geometry.txt"
}
neutral_point() { # 悬停消隐点：workspace header 安全区内
  awk '$1 == "id=workspace-header" {
    for (i = 2; i <= NF; i++) {
      if ($i ~ /^x=/) x = substr($i, 3)
      if ($i ~ /^y=/) y = substr($i, 3)
    }
    printf "%.1f,%.1f", x + 24, y + 24
    found = 1
  } END { exit found ? 0 : 1 }' "$OUT/geometry.txt"
}
se_escape() {
  osascript -e 'tell application "System Events" to key code 53' >/dev/null 2>&1 || true
}

dump_tree current
cp "$WORK/tree-current.txt" "$OUT/ax-tree.txt"
cp "$WORK/wid-current.id" "$OUT/window-id.txt"
"$FRAMES" "$DESKTOP_PID" > "$OUT/geometry.txt" || die "AX frames dump failed"
for required_id in task-rail-grouping "$SESSION_ROW_ID" model-picker \
  inspector-tab-terminal send composer-input workspace-header; do
  grep -q "^id=$required_id " "$OUT/geometry.txt" \
    || die "required identifier missing from frames: $required_id"
done
"$FOCUS_SWITCH" activate --pid "$DESKTOP_PID" >/dev/null 2>&1 || true

NEUTRAL="$(neutral_point)"
PHASES=()
fail_phase() {
  echo "ui-r7a-states: $*" >&2
  cp "$WORK/tree-current.txt" "$OUT/ax-tree-fail.txt" 2>/dev/null || true
  exit 4
}

trace "hover set: rail trigger / session row / model picker / inspector tab / send"
for target in task-rail-grouping "$SESSION_ROW_ID" model-picker inspector-tab-terminal send; do
  point="$(center_of "$target")" || fail_phase "no frame for $target"
  activate
  "$KEYEVENT" --pid "$DESKTOP_PID" --hover-at "$point" >/dev/null \
    || fail_phase "hover post failed ($target)"
  sleep 0.3
  dump_tree current
  shot "hover-$target"
  PHASES+=("hover-$target:pass")
  trace "hover $target captured"
  "$KEYEVENT" --pid "$DESKTOP_PID" --hover-at "$NEUTRAL" >/dev/null || true
  sleep 0.2
done

trace "active set: grouping trigger press-hold (release opens menu, Escape closes)"
point="$(center_of task-rail-grouping)"
activate
"$KEYEVENT" --pid "$DESKTOP_PID" --press-at "$point" >/dev/null \
  || fail_phase "press post failed (task-rail-grouping)"
sleep 0.3
dump_tree current
shot "active-task-rail-grouping"
"$KEYEVENT" --pid "$DESKTOP_PID" --release-at "$point" >/dev/null \
  || fail_phase "release post failed (task-rail-grouping)"
deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
menu_open=0
while (( SECONDS < deadline )); do
  dump_tree current
  if grep -q 'identifier="grouping-menu"' "$WORK/tree-current.txt"; then menu_open=1; break; fi
  sleep 0.2
done
[[ "$menu_open" == 1 ]] || fail_phase "grouping menu did not open after release"
shot "active-opened-menu"
PHASES+=("active-task-rail-grouping:pass")
se_escape
deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
menu_closed=0
while (( SECONDS < deadline )); do
  dump_tree current
  if ! grep -q 'identifier="grouping-menu"' "$WORK/tree-current.txt"; then menu_closed=1; break; fi
  sleep 0.2
done
[[ "$menu_closed" == 1 ]] || fail_phase "grouping menu did not close via Escape"
trace "active task-rail-grouping captured (menu opened then closed)"

trace "focus set: AX focus session row / composer-input"
activate
ax_focus "$SESSION_ROW_ID"
point="$(center_of "$SESSION_ROW_ID")"
"$KEYEVENT" --pid "$DESKTOP_PID" --click-at "$point" >/dev/null \
  || fail_phase "click post failed (session row)"
deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
row_selected=0
while (( SECONDS < deadline )); do
  dump_tree current
  if grep -q "identifier=\"$SESSION_ROW_ID\"[^#]*selected=1" "$WORK/tree-current.txt"; then
    row_selected=1; break
  fi
  sleep 0.2
done
# 产品合同：点击 session 行会 open_session 后 focus_composer，行级 AX focused
# 不会留在 ListRow 上。行级键盘描边由 Tab 链覆盖（u2 nav）；本相位断言选中。
[[ "$row_selected" == 1 ]] || fail_phase "session row did not become selected"
shot "focus-session-row"
PHASES+=("focus-session-row:pass")
activate
ax_focus "composer-input"
point="$(center_of composer-input)"
"$KEYEVENT" --pid "$DESKTOP_PID" --click-at "$point" >/dev/null \
  || fail_phase "click post failed (composer-input)"
deadline=$(( SECONDS + PHASE_TIMEOUT_SECS ))
input_focused=0
while (( SECONDS < deadline )); do
  dump_tree current
  if grep -q 'identifier="composer-input"[^#]*focused=1' "$WORK/tree-current.txt"; then
    input_focused=1; break
  fi
  sleep 0.2
done
[[ "$input_focused" == 1 ]] || fail_phase "composer-input did not gain focus"
shot "focus-composer-input"
PHASES+=("focus-composer-input:pass")

trace "write run manifest"
REPO_ROOT="$REPO_ROOT" OUT="$OUT" LABEL="$LABEL" SESSION_ROW_ID="$SESSION_ROW_ID" \
LAUNCH_FORM="$LAUNCH_FORM" LAUNCH_PATH="$LAUNCH_PATH" BUILD_SHA="$BUILD_SHA" APP_SHA="$APP_SHA" APP_PRE_SIGN_SHA="$APP_PRE_SIGN_SHA" \
  python3 -c '
import datetime
import json
import os
import subprocess
import sys
from pathlib import Path

out = os.environ["OUT"]
label = os.environ["LABEL"]
session_row = os.environ["SESSION_ROW_ID"]
repo = Path(os.environ["REPO_ROOT"])
head = subprocess.run(
    ["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True
).stdout.strip()
status = subprocess.run(
    ["git", "status", "--short"], cwd=repo, capture_output=True, text=True
).stdout.splitlines()
shots = sorted(p.name for p in Path(out).glob("shot-*.png"))
phases = {name.replace(".png", "").replace("shot-", ""): True for name in shots}
Path(out, "run-manifest.json").write_text(
    json.dumps(
        {
            "scenario": "r7-wave-a-states",
            "label": label,
            "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "git_head": head,
            "git_status": status,
            "launch": {
                "form": os.environ.get("LAUNCH_FORM", "bundled-adhoc"),
                "path": os.environ.get("LAUNCH_PATH", ""),
                "build_sha256": os.environ.get("BUILD_SHA", ""),
                "app_sha256": os.environ.get("APP_SHA", ""),
                "app_sha256_before_sign": os.environ.get("APP_PRE_SIGN_SHA", ""),
                "same_payload_before_sign": (
                    os.environ.get("APP_PRE_SIGN_SHA", "") != ""
                    and os.environ.get("APP_PRE_SIGN_SHA", "") == os.environ.get("BUILD_SHA", "")
                ),
            },
            "session_row": session_row,
            "phases": phases,
        },
        ensure_ascii=False,
        indent=2,
    ) + "\n",
    encoding="utf-8",
)
'
trace "run done (supplemental evidence complete)"
