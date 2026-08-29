#!/usr/bin/env bash
# Pawork R1 Wave B UI fixture 驱动脚本：seed/serve/desktop/drop-socket/
# restart-host/self-check/down/clean/scan。
#
# 冻结 CLI 契约（W1 example，调用形状不得改动；脚本先 build 再直接运行产物）：
#   cargo run -p pawork-app --offline --features ui-fixture --example ui_fixture --
#     <subcommand> --root <dir> ...
#
# 同步语义一律使用 barrier 文件（存在性/内容轮询），禁止固定 sleep 猜测。
# clean 只删除带 .pawork-ui-fixture marker 的目录（fail-closed，防误删）。
#
# 环境变量：
#   PAWORK_UI_SERVE_TIMEOUT_SECS   serve 等待 host_ready 的超时（默认 300；
#                                  首次 cargo build 编译较慢时可调大）
#   PAWORK_UI_BARRIER_TIMEOUT_SECS drop-socket/self-check 等 barrier 超时（默认 120）
#   PAWORK_UI_DESKTOP_BIN          仅覆盖 desktop 启动的已构建可执行文件；必须是
#                                  默认 build 产物或仓库外、名为 pawork-desktop 的
#                                  绝对路径。未设置时仍
#                                  build/启动 target/debug/pawork-desktop。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
DEFAULT_SCAN_TARGET="$REPO_ROOT/fixtures/ui"
SERVE_TIMEOUT_SECS="${PAWORK_UI_SERVE_TIMEOUT_SECS:-300}"
BARRIER_TIMEOUT_SECS="${PAWORK_UI_BARRIER_TIMEOUT_SECS:-120}"
POLL_INTERVAL=0.1

if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  if [[ "$CARGO_TARGET_DIR" == /* ]]; then
    TARGET_DIR="$CARGO_TARGET_DIR"
  else
    TARGET_DIR="$REPO_ROOT/$CARGO_TARGET_DIR"
  fi
else
  TARGET_DIR="$REPO_ROOT/target"
fi
UI_FIXTURE_BIN="$TARGET_DIR/debug/examples/ui_fixture"
DEFAULT_DESKTOP_BIN="$TARGET_DIR/debug/pawork-desktop"
DESKTOP_BIN="$DEFAULT_DESKTOP_BIN"
if [[ -n "${PAWORK_UI_DESKTOP_BIN:-}" ]]; then
  [[ "$PAWORK_UI_DESKTOP_BIN" == /* ]] \
    || { printf 'ui-fixture: PAWORK_UI_DESKTOP_BIN 必须是绝对路径\n' >&2; exit 1; }
  [[ "$PAWORK_UI_DESKTOP_BIN" != *$'\n'* && "$PAWORK_UI_DESKTOP_BIN" != *$'\r'* ]] \
    || { printf 'ui-fixture: PAWORK_UI_DESKTOP_BIN 不得含换行\n' >&2; exit 1; }
  [[ "${PAWORK_UI_DESKTOP_BIN##*/}" == "pawork-desktop" ]] \
    || { printf 'ui-fixture: PAWORK_UI_DESKTOP_BIN 文件名必须是 pawork-desktop\n' >&2; exit 1; }
  DESKTOP_BIN=$(python3 - "$PAWORK_UI_DESKTOP_BIN" "$REPO_ROOT" "$DEFAULT_DESKTOP_BIN" <<'PY'
import os
import sys

raw, repo_raw, default_raw = sys.argv[1:]
path = os.path.realpath(raw)
repo = os.path.realpath(repo_raw)
default = os.path.realpath(default_raw)
try:
    inside = os.path.commonpath((path, repo)) == repo
except ValueError:
    inside = False
if inside and path != default:
    raise SystemExit("PAWORK_UI_DESKTOP_BIN 在仓库内时只能指向默认 Desktop build 产物")
print(path)
PY
  ) || { printf 'ui-fixture: PAWORK_UI_DESKTOP_BIN 路径校验失败\n' >&2; exit 1; }
fi

die() { printf 'ui-fixture: %s\n' "$*" >&2; exit 1; }
info() { printf 'ui-fixture: %s\n' "$*"; }

usage() {
  cat <<'EOF'
用法：scripts/ui-fixture.sh <command> [options]

命令（root 型命令必须显式 --root <dir>，禁止指向默认数据目录）：
  seed --root <dir> [--now-ms <i64>]   生成/重建 fixture（幂等，example 冻结 CLI）
  serve --root <dir> [--profile <name>] 后台启动 fixture host，等待 host_ready
  desktop --root <dir>                 后台启动 Desktop，连接 root 内 socket
  desktop-restart --root <dir>         只停/起 desktop（host/数据/barrier 保留）
  drop-socket --root <dir>             请求 host drop 连接并等 drop_socket.done
  restart-host --root <dir> [--profile <name>]
                                      停旧起新 host，host_ready 后写 host_restarted
  self-check --root <dir>              内置 client 验证 Resume Replay
  down --root <dir>                    停止 host/desktop（数据与 barrier 保留）
  clean --root <dir>                   只删除带 .pawork-ui-fixture marker 的 root
  scan [<path>...] [--root <dir>]      敏感信息扫描（默认扫 fixtures/ui；命中 exit 2）

示例：
  ROOT="$(mktemp -d /tmp/pawork-ui-fixture.XXXXXX)"
  scripts/ui-fixture.sh seed --root "$ROOT"
  scripts/ui-fixture.sh serve --root "$ROOT" --profile r6-terminal
  scripts/ui-fixture.sh desktop --root "$ROOT"
  scripts/ui-fixture.sh desktop-restart --root "$ROOT"
  scripts/ui-fixture.sh self-check --root "$ROOT"
  scripts/ui-fixture.sh down --root "$ROOT"
  scripts/ui-fixture.sh clean --root "$ROOT"
EOF
}

# 从参数中提取 --root <dir> / --root=<dir> 到 ROOT（原参数保持透传给 example）。
extract_root() {
  ROOT=""
  local want_value=0 arg
  for arg in "$@"; do
    if [[ "$want_value" == 1 ]]; then ROOT="$arg"; want_value=0; fi
    if [[ "$arg" == "--root" ]]; then want_value=1; fi
    if [[ "$arg" == --root=* ]]; then ROOT="${arg#--root=}"; fi
  done
}

require_root() {
  [[ -n "$ROOT" ]] || die "缺少 --root <dir>（fixture root 必须显式指定，避免误用默认数据目录）"
  [[ "$ROOT" != *$'\n'* && "$ROOT" != *$'\r'* ]] || die "fixture root 不得含换行"
  local resolved
  resolved=$(python3 - "$ROOT" "$REPO_ROOT" <<'PY'
import os
import sys
from pathlib import Path

raw, repo_raw = sys.argv[1:]
root = Path(os.path.realpath(os.path.abspath(raw)))
repo = Path(os.path.realpath(repo_raw))
home = Path(os.path.realpath(str(Path.home())))
data_raw = os.environ.get("PAWORK_DATA_DIR", "").strip()
data = Path(os.path.realpath(os.path.abspath(data_raw))) if data_raw else home / ".pawork"

def within(child: Path, parent: Path) -> bool:
    try:
        return os.path.commonpath((str(child), str(parent))) == str(parent)
    except ValueError:
        return False

if root == Path("/"):
    raise SystemExit("拒绝把 / 作为 fixture root")
if root == repo or within(root, repo) or within(repo, root):
    raise SystemExit("fixture root 不得位于仓库内或包含仓库")
if root == data or within(root, data) or within(data, root):
    raise SystemExit("fixture root 不得位于默认数据目录内或包含默认数据目录")
if root == home or within(home, root):
    raise SystemExit("fixture root 不得是或包含用户 home")
print(root)
PY
  ) || die "fixture root 安全校验失败：$ROOT"
  ROOT="$resolved"
}

require_socket_path() {
  python3 - "$ROOT" <<'PY' || \
    die "fixture Unix socket 路径校验失败：$ROOT（请改用 /tmp 下的短 root）"
import os
import sys
from pathlib import Path

socket = Path(sys.argv[1]) / "data/pawork-gui.sock"
if len(os.fsencode(socket)) > 103:
    raise SystemExit(f"fixture Unix socket 路径过长：{socket}")
PY
}

require_marker() {
  [[ -f "$ROOT/.pawork-ui-fixture" ]] || \
    die "root 缺少 .pawork-ui-fixture marker：${ROOT}（先 seed；本脚本只操作带 marker 的目录）"
}

require_ready_marker() {
  require_marker
  python3 - "$ROOT/.pawork-ui-fixture" <<'PY' || \
    die "fixture marker 不是 ready（可能 seed 未完成）；请重新运行 seed --root '$ROOT'"
import json
import sys

try:
    marker = json.loads(open(sys.argv[1], encoding="utf-8").read())
except (OSError, ValueError):
    raise SystemExit(1)
raise SystemExit(0 if marker.get("state") == "ready" else 1)
PY
}

now_ms() {
  if command -v python3 >/dev/null 2>&1; then
    python3 -c 'import time; print(int(time.time() * 1000))'
  else
    printf '%s000\n' "$(date +%s)"
  fi
}

# 原子写 barrier：内容 JSON {at_ms, detail}（brief §6 冻结语义）。
write_barrier() { # $1=file $2=detail
  local file="$1" tmp="$1.tmp"
  mkdir -p "$(dirname "$file")"
  printf '{"at_ms": %s, "detail": "%s"}\n' "$(now_ms)" "$2" > "$tmp"
  mv "$tmp" "$file"
}

# 轮询等待文件出现（§6 允许的读侧轮询）；超时返回 1。
wait_for_file() { # $1=file $2=timeout_secs
  local file="$1" ticks=$(( $2 * 10 )) n=0
  while [[ ! -e "$file" ]]; do
    if (( n >= ticks )); then return 1; fi
    sleep "$POLL_INTERVAL"; n=$((n + 1))
  done
  return 0
}

tail_log() { # $1=log 文件
  if [[ -f "$1" ]]; then
    printf 'ui-fixture: 最近日志（%s）：\n' "$1" >&2
    tail -n 20 "$1" >&2 || true
  fi
}

seeded_root_dirs() {
  mkdir -p "$ROOT/logs" "$ROOT/barriers"
}

build_ui_fixture() {
  (cd "$REPO_ROOT" && cargo build -p pawork-app --offline --features ui-fixture \
    --example ui_fixture)
  [[ -x "$UI_FIXTURE_BIN" ]] || die "找不到已构建的 ui_fixture：$UI_FIXTURE_BIN"
}

build_desktop() {
  if [[ -z "${PAWORK_UI_DESKTOP_BIN:-}" ]]; then
    (cd "$REPO_ROOT" && cargo build -p pawork-desktop --offline \
      --features gpui/runtime_shaders --bin pawork-desktop)
  fi
  [[ -x "$DESKTOP_BIN" ]] || die "找不到已构建的 pawork-desktop：$DESKTOP_BIN"
}

pid_matches_kind() { # $1=pid $2=host|desktop
  local pid="$1" kind="$2" command
  [[ "$pid" =~ ^[0-9]+$ ]] || return 1
  command=$(ps -p "$pid" -o command= 2>/dev/null || true)
  [[ -n "$command" ]] || return 1
  case "$kind" in
    host)    [[ "$command " == *"/ui_fixture serve --root $ROOT "* ]] ;;
    desktop)
      local launch_path
      launch_path=$(cat "$ROOT/desktop.launch-path" 2>/dev/null || true)
      [[ -n "$launch_path" ]] || launch_path="$DESKTOP_BIN"
      [[ "$command" == "$launch_path --socket $ROOT/data/pawork-gui.sock"* ]]
      ;;
    *)       return 1 ;;
  esac
}

refuse_if_running() { # $1=pidfile $2=label $3=host|desktop
  local pidfile="$1" label="$2" kind="$3" pid
  pid=$(cat "$pidfile" 2>/dev/null || true)
  if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
    if pid_matches_kind "$pid" "$kind"; then
      die "$label 已在运行（pid ${pid}）；先执行 down --root '$ROOT'"
    fi
    info "忽略不属于本 fixture 的陈旧 $label pidfile（pid ${pid}）"
  fi
  rm -f "$pidfile"
}

cmd_seed() {
  local -a forwarded=()
  while (( $# )); do
    case "$1" in
      --root) shift 2 ;;
      --root=*) shift ;;
      *) forwarded+=("$1"); shift ;;
    esac
  done
  if [[ -f "$ROOT/.pawork-ui-fixture" ]]; then
    refuse_if_running "$ROOT/host.pid" "host" host
    refuse_if_running "$ROOT/desktop.pid" "desktop" desktop
  fi
  build_ui_fixture
  if [[ -n "${forwarded[0]+present}" ]]; then
    "$UI_FIXTURE_BIN" seed --root "$ROOT" "${forwarded[@]}"
  else
    "$UI_FIXTURE_BIN" seed --root "$ROOT"
  fi
  info "seed 完成：$ROOT"
}

cmd_serve() {
  local -a forwarded=()
  while (( $# )); do
    case "$1" in
      --root) shift 2 ;;
      --root=*) shift ;;
      *) forwarded+=("$1"); shift ;;
    esac
  done
  require_ready_marker
  refuse_if_running "$ROOT/host.pid" "host" host
  build_ui_fixture
  seeded_root_dirs
  # 清除上一次 serve 的 barrier，确保本次等待观察到的是新 host 写入的文件。
  rm -f "$ROOT/barriers/host_ready" \
        "$ROOT/barriers/host_restarted" \
        "$ROOT/barriers/drop_socket.request" \
        "$ROOT/barriers/drop_socket.done"
  info "启动 ui_fixture serve（日志：$ROOT/logs/serve.log）"
  (
    if [[ -n "${forwarded[0]+present}" ]]; then
      nohup "$UI_FIXTURE_BIN" serve --root "$ROOT" "${forwarded[@]}" \
        </dev/null >>"$ROOT/logs/serve.log" 2>&1 &
    else
      nohup "$UI_FIXTURE_BIN" serve --root "$ROOT" \
        </dev/null >>"$ROOT/logs/serve.log" 2>&1 &
    fi
    printf '%s' "$!" > "$ROOT/host.pid"
  )
  local pid ticks=$(( SERVE_TIMEOUT_SECS * 10 )) n=0
  pid=$(cat "$ROOT/host.pid")
  while [[ ! -e "$ROOT/barriers/host_ready" ]]; do
    if ! kill -0 "$pid" 2>/dev/null; then
      tail_log "$ROOT/logs/serve.log"
      die "host 进程提前退出（pid ${pid}）"
    fi
    if (( n >= ticks )); then
      tail_log "$ROOT/logs/serve.log"
      die "等待 host_ready 超时（${SERVE_TIMEOUT_SECS}s）"
    fi
    sleep "$POLL_INTERVAL"; n=$((n + 1))
  done
  info "host ready（pid ${pid}；socket $ROOT/data/pawork-gui.sock；token 文件 $ROOT/data/gui.token）"
}

cmd_desktop() {
  require_ready_marker
  local socket="$ROOT/data/pawork-gui.sock"
  refuse_if_running "$ROOT/desktop.pid" "desktop" desktop
  build_desktop
  seeded_root_dirs
  [[ -S "$socket" ]] || die "socket 不存在：${socket}（先 serve --root '$ROOT'）"
  info "启动 desktop（日志：$ROOT/logs/desktop.log）"
  printf '%s\n' "$DESKTOP_BIN" > "$ROOT/desktop.launch-path"
  (
    PAWORK_UI_BARRIER_DIR="$ROOT/barriers" \
      nohup "$DESKTOP_BIN" --socket "$socket" \
      </dev/null >>"$ROOT/logs/desktop.log" 2>&1 &
    printf '%s' "$!" > "$ROOT/desktop.pid"
  )
  info "desktop 已启动（pid $(cat "$ROOT/desktop.pid")；连接 ${socket}）"
}

cmd_desktop_restart() {
  require_ready_marker
  require_socket_path
  stop_desktop
  cmd_desktop
  write_barrier "$ROOT/barriers/desktop_restarted" \
    "desktop-restart 完成：旧 desktop 停止后新 desktop 启动（host/数据/barrier 保留）"
  info "desktop restarted（pid $(cat "$ROOT/desktop.pid")；host 与数据目录未动）"
}

cmd_drop_socket() {
  require_ready_marker
  seeded_root_dirs
  rm -f "$ROOT/barriers/drop_socket.done"
  write_barrier "$ROOT/barriers/drop_socket.request" "driver 请求 host drop 全部连接（保持监听）"
  if ! wait_for_file "$ROOT/barriers/drop_socket.done" "$BARRIER_TIMEOUT_SECS"; then
    die "等待 drop_socket.done 超时（host 是否在运行？）"
  fi
  info "host 已 drop 全部连接并保持监听"
}

cmd_restart_host() {
  require_ready_marker
  stop_host
  cmd_serve "$@"
  write_barrier "$ROOT/barriers/host_restarted" "restart-host 完成：旧 host 停止后新 host_ready"
  info "host restarted（desktop 无需重启，走断连重连恢复路径）"
}

cmd_self_check() {
  require_ready_marker
  seeded_root_dirs
  rm -f "$ROOT/barriers/replay_complete"
  info "运行 ui_fixture self-check"
  build_ui_fixture
  if ! ("$UI_FIXTURE_BIN" self-check --root "$ROOT" 2>&1 \
        | tee -a "$ROOT/logs/self-check.log"); then
    tail_log "$ROOT/logs/self-check.log"
    die "self-check 失败"
  fi
  if ! wait_for_file "$ROOT/barriers/replay_complete" "$BARRIER_TIMEOUT_SECS"; then
    die "self-check 退出 0 但 replay_complete barrier 未出现"
  fi
  info "self-check 通过（replay_complete 已写）"
}

stop_pid_tree() { # $1=pid $2=kind $3=label；仅停止已核实归属的进程树
  local pid="$1" kind="$2" label="$3" sig n
  [[ -n "$pid" ]] || return 0
  kill -0 "$pid" 2>/dev/null || return 0
  if ! pid_matches_kind "$pid" "$kind"; then
    info "拒绝向不属于本 fixture 的陈旧 $label PID 发信号：$pid"
    return 0
  fi
  for sig in INT TERM KILL; do
    # 上一轮信号后的等待窗口内 PID 可能退出并被系统复用；每次升级前重做
    # 完整 command-line 归属校验，绝不向复用后的无关进程继续发信号。
    if ! pid_matches_kind "$pid" "$kind"; then
      info "$label 已退出或 PID 已复用，停止信号升级：$pid"
      return 0
    fi
    pkill "-$sig" -P "$pid" 2>/dev/null || true
    kill "-$sig" "$pid" 2>/dev/null || true
    n=0
    while kill -0 "$pid" 2>/dev/null; do
      if (( n >= 50 )); then break; fi
      sleep "$POLL_INTERVAL"; n=$((n + 1))
    done
    kill -0 "$pid" 2>/dev/null || return 0
  done
  return 0
}

stop_host() {
  local pid
  pid=$(cat "$ROOT/host.pid" 2>/dev/null || true)
  # 优先走 example 的优雅停机 barrier（watch_stop_request → abort accept 后
  # close listener，serve exit 0）；5s 内未退出再交给 stop_pid_tree 信号升级。
  if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null \
      && pid_matches_kind "$pid" host && [[ -d "$ROOT/barriers" ]]; then
    : > "$ROOT/barriers/serve_stop.request"
    local n=0
    while kill -0 "$pid" 2>/dev/null; do
      if (( n >= 50 )); then break; fi
      sleep "$POLL_INTERVAL"; n=$((n + 1))
    done
  fi
  if [[ -n "$pid" ]]; then stop_pid_tree "$pid" host host; fi
  rm -f "$ROOT/host.pid"
}

stop_desktop() {
  local pid
  pid=$(cat "$ROOT/desktop.pid" 2>/dev/null || true)
  if [[ -n "$pid" ]]; then stop_pid_tree "$pid" desktop desktop; fi
  rm -f "$ROOT/desktop.pid"
  rm -f "$ROOT/desktop.launch-path"
}

cmd_down() {
  if [[ ! -f "$ROOT/.pawork-ui-fixture" ]]; then
    info "root 无 marker，视为未 seed，无事可做：$ROOT"
    return 0
  fi
  stop_host
  stop_desktop
  info "host/desktop 已停止（数据与 barrier 保留；彻底重置用 clean）"
}

cmd_clean() {
  if [[ "$ROOT" == "/" ]]; then die "拒绝清理根目录"; fi
  if [[ "$ROOT" == "$REPO_ROOT" ]]; then die "拒绝清理仓库根目录"; fi
  if [[ ! -d "$ROOT" ]]; then
    info "root 不存在，无事可做：$ROOT"
    return 0
  fi
  # clean 只删带 .pawork-ui-fixture marker 的目录（冻结契约，防误删）。
  [[ -f "$ROOT/.pawork-ui-fixture" ]] || \
    die "root 缺少 .pawork-ui-fixture marker，拒绝清理：$ROOT"
  stop_host
  stop_desktop
  # ROOT 已 realpath + 保护路径校验，且 marker 已确认；用语言原生 API 对
  # 这个精确字面路径做单目录删除，不使用 glob/未解析变量。
  python3 - "$ROOT" <<'PY'
import shutil
import sys

shutil.rmtree(sys.argv[1])
PY
  info "已清理 fixture root：$ROOT"
}

cmd_scan() {
  # 过滤 --root/--root=<dir> 自身，只把路径参数与 ROOT 传给扫描器。
  local -a paths=()
  local skip=0 arg
  for arg in "$@"; do
    if (( skip )); then skip=0; continue; fi
    case "$arg" in
      --root)    skip=1 ;;
      --root=*)  ;;
      *)         paths+=("$arg") ;;
    esac
  done
  if [[ -n "$ROOT" ]]; then paths+=("$ROOT"); fi
  if (( ${#paths[@]} == 0 )); then paths+=("$DEFAULT_SCAN_TARGET"); fi
  exec python3 "$SCRIPT_DIR/ui-fixture-scan.py" "${paths[@]}"
}

main() {
  if (( $# == 0 )); then
    usage >&2
    die "缺少子命令"
  fi
  local cmd="$1"
  shift
  extract_root "$@"
  case "$cmd" in
    seed)          require_root; require_socket_path; cmd_seed "$@" ;;
    serve)         require_root; require_socket_path; cmd_serve "$@" ;;
    desktop)       require_root; require_socket_path; cmd_desktop ;;
    desktop-restart) require_root; require_socket_path; cmd_desktop_restart ;;
    drop-socket)   require_root; require_socket_path; cmd_drop_socket ;;
    restart-host)  require_root; require_socket_path; cmd_restart_host "$@" ;;
    self-check)    require_root; require_socket_path; cmd_self_check ;;
    down)          require_root; cmd_down ;;
    clean)         require_root; cmd_clean ;;
    scan)          cmd_scan "$@" ;;
    -h|--help|help) usage ;;
    *) usage >&2; die "未知命令：$cmd" ;;
  esac
}

main "$@"
