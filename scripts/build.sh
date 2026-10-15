#!/usr/bin/env bash
# 构建正式 Host/Desktop，stdout 按顺序返回本次构建的两个可执行文件路径。
set -euo pipefail

if [[ $# -eq 1 && ( "$1" == -h || "$1" == --help ) ]]; then
  printf 'Usage: %s\nBuild Pawork host and desktop (offline).\n' "${0##*/}"
  exit 0
fi
[[ $# -eq 0 ]] || { printf 'Usage: %s\n' "${0##*/}" >&2; exit 2; }

cd "$(dirname "${BASH_SOURCE[0]}")/.."
args=(--offline -p pawork -p pawork-desktop --bins)
if [[ "$(uname -s)" == Darwin ]]; then args+=(--features gpui/runtime_shaders); fi
build_log=$(mktemp)
trap 'rm -f "$build_log"' EXIT
if cargo build "${args[@]}" --message-format=json >"$build_log"; then
  :
else
  status=$?
  cat "$build_log" >&2
  exit "$status"
fi

# 不猜 target 路径，兼容 Cargo 的 target-dir / build.target 配置并排除测试产物。
python3 - "$build_log" <<'PY'
import json
import os
import sys

found = {}
with open(sys.argv[1], encoding="utf-8") as lines:
    for line in lines:
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if not isinstance(msg, dict) or msg.get("reason") != "compiler-artifact":
            continue
        target = msg.get("target", {})
        if ("bin" in target.get("kind", [])
                and not msg.get("profile", {}).get("test") and msg.get("executable")):
            found[target.get("name")] = msg["executable"]
for name in ("pawork", "pawork-desktop"):
    path = found.get(name, "")
    if not os.path.isfile(path) or not os.access(path, os.X_OK):
        raise SystemExit("未能定位本次构建的 %s 可执行文件" % name)
    print(path)
PY
