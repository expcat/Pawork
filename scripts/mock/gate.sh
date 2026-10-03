#!/usr/bin/env bash
# mock 定向门禁：文档、所选 Rust 包、本地 Python/HTTP 回归；非全 workspace 门禁。
set -euo pipefail
cd "$(dirname "$0")/../.."

usage() {
  cat <<'HELP'
用法：bash scripts/mock/gate.sh [--packages <包名,...>] [--level <0,1,2 的子集>]

L0  ROADMAP 与当前改动 Markdown 的相对链接存在性、git diff --check。
L1  scripts/test.sh providers <追加包...>；同一次 Cargo 调用补齐 feature。
L2  fixture 脱敏、OAuth、配置/凭证恢复、测试入口、UI 扫描和 mock HTTP 场景。
默认运行 L0/L1/L2；Desktop 仍单独用 scripts/test.sh desktop。
真实 Provider 冒烟按 docs/spec/verification.md §2.1 手动执行，不入本门禁。
退出码：1 用法错误；2 L0 失败；3 L1 失败；4 L2 失败。
HELP
}

extra_csv=""
levels=",0,1,2,"
parse_levels() {
  local csv="${1//[[:space:]]/}" item
  local items=()
  IFS=',' read -r -a items <<< "$csv"
  levels=","
  for item in "${items[@]+"${items[@]}"}"; do
    case "$item" in
      0|1|2) levels+="$item," ;;
      '') ;;
      *) echo "gate: 未知级别：${item}（可用：0,1,2）" >&2; return 1 ;;
    esac
  done
  [ "$levels" != "," ] || { echo 'gate: --level 未指定有效级别' >&2; return 1; }
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --packages|--level)
      option="$1"
      [ "$#" -ge 2 ] || { echo "gate: $option 需要参数" >&2; exit 1; }
      value="$2"
      shift 2
      ;;
    --packages=*|--level=*) option="${1%%=*}"; value="${1#*=}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "gate: 未知参数：$1" >&2; usage >&2; exit 1 ;;
  esac
  case "$option" in
    --packages) extra_csv+="$value," ;;
    --level) parse_levels "$value" || exit 1 ;;
  esac
done

level0() {
  python3 - <<'PY'
import re
import subprocess
from pathlib import Path

files = {"docs/ROADMAP.md"}
for args in (
    ["git", "diff", "--name-only", "-z", "HEAD", "--", "*.md"],
    ["git", "ls-files", "--others", "--exclude-standard", "-z", "--", "*.md"],
):
    files.update(subprocess.check_output(args).decode().split("\0"))
link_re = re.compile(r'\[[^\]]*\]\((<[^>]+>|[^)\s]+)(?:\s+"[^"]*")?\)')
broken = []
checked = 0
for name in sorted(files):
    md = Path(name)
    if not name or not md.is_file():
        continue
    checked += 1
    fence = None
    for lineno, line in enumerate(md.read_text(encoding="utf-8").splitlines(), 1):
        stripped = line.lstrip()
        if stripped.startswith(("```", "~~~")):
            marker = stripped[:3]
            if fence is None:
                fence = marker
            elif marker == fence:
                fence = None
            continue
        if fence:
            continue
        for target in link_re.findall(line):
            target = target.strip("<>")
            if target.startswith(("#", "mailto:", "http://", "https://", "data:", "//")):
                continue
            path = target.split("#", 1)[0]
            if path and not (md.parent / path).resolve().exists():
                broken.append(f"{name}:{lineno}: {target}")
if broken:
    raise SystemExit("相对链接缺失：\n" + "\n".join(broken))
print(f"Markdown 相对链接 OK（{checked} 个文件）")
PY
  [ "$?" -eq 0 ] || return 1
  git diff --check HEAD
}

level1() {
  local pkg packages=() selected=(providers)
  IFS=',' read -r -a packages <<< "$extra_csv"
  for pkg in "${packages[@]+"${packages[@]}"}"; do
    pkg="${pkg//[[:space:]]/}"
    [ -n "$pkg" ] || continue
    case "$pkg" in
      -*|*[!a-z0-9-]*) echo "gate: 非法包名：$pkg" >&2; return 1 ;;
    esac
    selected+=("$pkg")
  done
  bash scripts/test.sh "${selected[@]}"
}

level2() {
  local script
  python3 scripts/mock/capture.py verify || return 1
  for script in scripts/mock/oauth_selftest.py scripts/mock/review_selftest.py \
                scripts/test_ui_fixture_scan.py scripts/mock/server_smoke.py; do
    echo "Running: $script"
    python3 "$script" || return 1
  done
}

started=$SECONDS
summary=()
for level in 0 1 2; do
  [[ "$levels" == *",$level,"* ]] || continue
  level_started=$SECONDS
  if "level$level"; then
    summary+=("L$level $((SECONDS - level_started))s")
    echo "L$level PASS ($((SECONDS - level_started))s)"
  else
    echo "GATE FAIL L$level ($((SECONDS - level_started))s)" >&2
    exit "$((level + 2))"
  fi
done
printf 'GATE PASS (%ss): %s\n' "$((SECONDS - started))" "${summary[*]}"
