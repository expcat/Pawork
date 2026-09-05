#!/usr/bin/env bash

set -euo pipefail

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
runtime_dir="$repo_dir/target/pawork-desktop-runtime"
pawork_bin="$repo_dir/target/debug/pawork"
desktop_bin="$repo_dir/target/debug/pawork-desktop"
host_log="$runtime_dir/host.log"
macos_bundle="$runtime_dir/Pawork.app"
command_mode="${1:-start}"
instance_name="${PAWORK_DESKTOP_INSTANCE:-desktop}"
trust_workspaces="${PAWORK_DESKTOP_TRUST_WORKSPACES:-1}"
approval_mode="${PAWORK_DESKTOP_APPROVAL_MODE:-ask-for-dangerous}"

usage() {
  printf 'Usage: %s [build|start]\n' "${0##*/}"
  printf 'Start defaults: instance=desktop, approval=ask-for-dangerous, trust-workspaces=1.\n'
}

build_binaries() {
  cd "$repo_dir"
  cargo build --offline \
    -p pawork \
    -p pawork-desktop \
    --bins \
    --features gpui/runtime_shaders
}

host_is_running() {
  local status_output
  status_output="$("$pawork_bin" --instance "$instance_name" status 2>/dev/null)" || return 1
  [[ "$status_output" == *"(listening)"* ]]
}

prepare_macos_bundle() {
  local executable_dir="$macos_bundle/Contents/MacOS"
  mkdir -p "$executable_dir"
  cp "$repo_dir/apps/desktop/macos/Info.plist" "$macos_bundle/Contents/Info.plist"
  # 必须复制真实二进制，不能用符号链接：LaunchServices(open) 启动「指向
  # target/debug/pawork-desktop 的 symlink」时，新构建二进制的进程会永久卡死
  # 在 dyld 的 getOnDiskBinarySliceOffset -> open()（2026-09-05 实测：
  # symlink bundle 三次启动全部卡死，真实文件 bundle 同二进制正常启动）。
  cp -f "$desktop_bin" "$executable_dir/Pawork"
}

case "$command_mode" in
  build)
    [[ $# -eq 1 ]] || { usage >&2; exit 2; }
    build_binaries
    exit 0
    ;;
  start)
    [[ $# -le 1 ]] || { usage >&2; exit 2; }
    ;;
  -h|--help|help)
    usage
    exit 0
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac

build_binaries
mkdir -p "$runtime_dir"

case "$approval_mode" in
  always-ask|ask-for-writes|ask-for-dangerous|never-ask|read-only)
    ;;
  *)
    printf 'PAWORK_DESKTOP_APPROVAL_MODE must be always-ask, ask-for-writes, ask-for-dangerous, never-ask, or read-only.\n' >&2
    exit 2
    ;;
esac

host_was_started=0
host_process_id=""

cleanup_host() {
  if [[ "$host_was_started" -ne 1 || -z "$host_process_id" ]]; then
    return
  fi
  if kill -0 "$host_process_id" 2>/dev/null; then
    kill -INT "$host_process_id" 2>/dev/null || true
    wait "$host_process_id" 2>/dev/null || true
  fi
}
trap cleanup_host EXIT INT TERM

if host_is_running; then
  printf 'Reusing the running Pawork host.\n'
else
  host_trust_args=()
  if [[ "$trust_workspaces" == "1" ]]; then
    host_trust_args+=(--trust-workspaces)
  elif [[ "$trust_workspaces" != "0" ]]; then
    printf 'PAWORK_DESKTOP_TRUST_WORKSPACES must be 0 or 1.\n' >&2
    exit 2
  fi
  : >"$host_log"
  (
    cd "$repo_dir"
    exec "$pawork_bin" --instance "$instance_name" --approval-mode "$approval_mode" "${host_trust_args[@]}" gui serve
  ) >"$host_log" 2>&1 &
  host_process_id=$!
  host_was_started=1

  host_ready=0
  for readiness_attempt in {1..60}; do
    if ! kill -0 "$host_process_id" 2>/dev/null; then
      printf 'Pawork host exited during startup. Log: %s\n' "$host_log" >&2
      tail -n 40 "$host_log" >&2 || true
      exit 1
    fi
    if host_is_running; then
      host_ready=1
      break
    fi
    sleep 0.1
  done

  if [[ "$host_ready" -ne 1 ]]; then
    printf 'Timed out waiting for the Pawork host. Log: %s\n' "$host_log" >&2
    tail -n 40 "$host_log" >&2 || true
    exit 1
  fi
  printf 'Started the Pawork host (approval=%s, trust-workspaces=%s). Log: %s\n' \
    "$approval_mode" "$trust_workspaces" "$host_log"
fi

cd "$repo_dir"
if [[ "$(uname -s)" == "Darwin" ]]; then
  prepare_macos_bundle
  "$macos_bundle/Contents/MacOS/Pawork" --instance "$instance_name"
else
  "$desktop_bin" --instance "$instance_name"
fi
