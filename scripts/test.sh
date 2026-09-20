#!/usr/bin/env bash
# 按改动包验证真实行为；显式补齐容易漏跑的测试 feature。
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
  cat <<'HELP'
用法：bash scripts/test.sh [--print] <包名>...
      bash scripts/test.sh [--print] --host

包名可用 policy 或 pawork-policy；pawork / desktop 为应用。
一次 Cargo 调用验证所选包，自动补齐通道、存储、协议、GUI fixture 等特性。
desktop 单独执行（macOS 使用 runtime_shaders）。
--host 先构建当前 pawork，再执行 client 的真实子进程集成测试。
--print 只显示命令，不编译、不测试；不支持隐式全 workspace 或真实 Provider 请求。
HELP
}

print_only=0
host=0
packages=()
for arg in "$@"; do
  case "$arg" in
    --print) print_only=1 ;;
    --host) host=1 ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "未知选项：$arg" >&2; exit 2 ;;
    *)
      case "$arg" in
        pawork|pawork-*) pkg="$arg" ;;
        *) pkg="pawork-$arg" ;;
      esac
      case "$pkg" in *[!a-z0-9-]*) echo "非法包名：$pkg" >&2; exit 2 ;; esac
      case "$pkg" in
        pawork) manifest=apps/pawork/Cargo.toml ;;
        pawork-desktop) manifest=apps/desktop/Cargo.toml ;;
        *) manifest="crates/${pkg#pawork-}/Cargo.toml" ;;
      esac
      [ -f "$manifest" ] || { echo "未知包：$pkg" >&2; exit 2; }
      # Bash 3.2 + nounset 不展开空数组。
      if [ "${#packages[@]}" -eq 0 ] || [[ " ${packages[*]} " != *" $pkg "* ]]; then
        packages+=("$pkg")
      fi
      ;;
  esac
done

run() {
  printf '%q ' "$@"; printf '\n'
  if [ "$print_only" -eq 0 ]; then "$@"; fi
}

if [ "$host" -eq 1 ]; then
  [ "${#packages[@]}" -eq 0 ] || { echo '--host 不与包列表混用' >&2; exit 2; }
  # 用 cargo 的 artifact 消息定位本次构建出的可执行文件：config 的
  # target-dir / build.target 都不会让测试误用旧 Host。
  build_log="$(mktemp)"
  if [ "$print_only" -eq 1 ]; then
    run cargo build -p pawork --offline --bin pawork --message-format=json
    binary="<本次构建的 pawork 可执行文件>"
  else
    run cargo build -p pawork --offline --bin pawork --message-format=json > "$build_log"
    binary="$(python3 - "$build_log" <<'PYJSON'
import json, sys
exe = ""
for line in open(sys.argv[1], encoding="utf-8"):
    try:
        msg = json.loads(line)
    except ValueError:
        continue
    target = msg.get("target", {})
    if (msg.get("reason") == "compiler-artifact" and target.get("name") == "pawork"
            and "bin" in target.get("kind", []) and msg.get("executable")):
        exe = msg["executable"]
print(exe)
PYJSON
)"
    rm -f "$build_log"
    [ -n "$binary" ] && [ -x "$binary" ] || { echo '未能定位本次构建的 pawork 可执行文件' >&2; exit 1; }
  fi
  run env PAWORK_BIN="$binary" cargo test -p pawork-client --offline --features spawn-e2e --test spawn_e2e
  exit 0
fi

[ "${#packages[@]}" -gt 0 ] || { usage >&2; exit 2; }
if [[ " ${packages[*]} " == *" pawork-desktop "* ]]; then
  [ "${#packages[@]}" -eq 1 ] || { echo 'desktop 请单独执行' >&2; exit 2; }
  if [ "$(uname -s)" = Darwin ]; then
    run cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders
  else
    run cargo test -p pawork-desktop --offline --bins
  fi
  exit 0
fi

args=(test --offline --tests)
features=""
for pkg in "${packages[@]}"; do
  args+=(-p "$pkg")
  case "$pkg" in
    pawork-providers)
      for feature in anthropic chatgpt-oauth xai-oauth glm-coding opencode-go qwen-token-plan deepseek kimi-platform kimi-code; do
        features+="pawork-providers/$feature,"
      done ;;
    pawork-storage) features+='pawork-storage/compaction,pawork-storage/checkpoint,pawork-storage/protected,' ;;
    pawork-protocol) features+='pawork-protocol/typegen,' ;;
    pawork-transport) features+='pawork-transport/memory,' ;;
    pawork-orchestration) features+='pawork-orchestration/git,' ;;
    pawork-app) features+='pawork-app/ui-fixture,' ;;
    pawork-client) features+='pawork-client/probe-self-test,' ;;
  esac
done
if [ -n "$features" ]; then args+=(--features "${features%,}"); fi
run cargo "${args[@]}"
