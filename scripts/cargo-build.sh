#!/usr/bin/env bash
# 供构建入口 source：从本次 Cargo artifact 消息取产物，不猜 target 路径。
# $1=按输出顺序排列的目标名（逗号分隔），其余参数为构建命令；可加 run 包装。
cargo_build_artifacts() (
  local targets="$1" build_log paths="" status=0
  shift
  build_log=$(mktemp) || return 1
  trap 'rm -f "$build_log"' EXIT
  if "$@" --message-format=json > "$build_log"; then
    if paths=$(python3 - "$build_log" "$targets" <<'PY'
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
        if (set(target.get("kind", [])) & {"bin", "example"}
                and not msg.get("profile", {}).get("test") and msg.get("executable")):
            found[target.get("name")] = msg["executable"]
for name in sys.argv[2].split(","):
    path = found.get(name, "")
    if not os.path.isfile(path) or not os.access(path, os.X_OK):
        raise SystemExit("未能定位本次构建的 %s 可执行文件" % name)
    print(path)
PY
    ); then :; else status=$?; fi
  else
    status=$?
    cat "$build_log" >&2
  fi
  [ "$status" -eq 0 ] || return "$status"
  printf '%s\n' "$paths"
)
