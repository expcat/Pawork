#!/usr/bin/env bash
# ui-focus-switch.sh — U2 focus/blur helper（测试工具，不进生产构建）。
#
# 通过 AppleScript System Events 切换前台应用：
#   activate    把指定 pid 的窗口应用置前（AX frontmost=true）
#   deactivate  激活 Finder 使目标应用失焦（blur）
#   state       打印 frontmost=true/false（front 时 exit 0）
#
# 同步语义：轮询确认激活状态收敛后才返回；禁固定 sleep 猜测。
# 环境变量：PAWORK_UI_FOCUS_TIMEOUT_SECS（默认 15）。

set -euo pipefail

POLL_INTERVAL=0.15
FOCUS_TIMEOUT_SECS="${PAWORK_UI_FOCUS_TIMEOUT_SECS:-15}"

usage() {
  echo "usage: scripts/ui-focus-switch.sh <activate|deactivate|state> --pid <pid>" >&2
  exit 2
}

MODE=""
PID=""
while (( $# )); do
  case "$1" in
    activate|deactivate|state) MODE="$1"; shift ;;
    --pid)
      [[ -n "${2:-}" ]] || usage
      PID="$2"
      shift 2
      ;;
    -h|--help) usage ;;
    *) echo "ui-focus-switch: unknown argument $1" >&2; usage ;;
  esac
done
[[ -n "$MODE" && "$PID" =~ ^[0-9]+$ ]] || usage
kill -0 "$PID" 2>/dev/null || { echo "ui-focus-switch: pid not running: $PID" >&2; exit 3; }

frontmost_pid() {
  osascript \
    -e 'tell application "System Events" to get unix id of first application process whose frontmost is true' \
    2>/dev/null || true
}

if [[ "$MODE" == "state" ]]; then
  front="$(frontmost_pid)"
  if [[ "$front" == "$PID" ]]; then
    echo "# focus state pid=$PID frontmost=true"
    exit 0
  fi
  echo "# focus state pid=$PID frontmost=false front=${front:-none}"
  exit 1
fi

if [[ "$MODE" == "activate" ]]; then
  osascript \
    -e "tell application \"System Events\" to set frontmost of first application process whose unix id is $PID to true" \
    >/dev/null
else
  osascript -e 'tell application "Finder" to activate' >/dev/null
fi

deadline=$(( SECONDS + FOCUS_TIMEOUT_SECS ))
attempts=0
while :; do
  attempts=$(( attempts + 1 ))
  front="$(frontmost_pid)"
  if [[ "$MODE" == "activate" && "$front" == "$PID" ]]; then
    echo "# focus $MODE pid=$PID frontmost=true attempts=$attempts"
    exit 0
  fi
  if [[ "$MODE" == "deactivate" && "$front" != "$PID" ]]; then
    echo "# focus $MODE pid=$PID frontmost=false front=${front:-none} attempts=$attempts"
    exit 0
  fi
  kill -0 "$PID" 2>/dev/null \
    || { echo "ui-focus-switch: target exited: $PID" >&2; exit 3; }
  (( SECONDS < deadline )) || {
    echo "ui-focus-switch: $MODE not converged (front=${front:-none} attempts=$attempts)" >&2
    exit 3
  }
  sleep "$POLL_INTERVAL"
done
