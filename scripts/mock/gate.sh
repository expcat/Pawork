#!/usr/bin/env bash
#
# MOCK-8 mock 快速门禁：单入口四级定向验证。
#
# 定位：这是 mock 系列的快速定向门禁，不是全量门禁；仓库「当前未设置全量
# 门禁」的约定不变。任一级失败即非零退出并指明级别。
#
#   L0（秒级）docs/ROADMAP.md 与当前改动过的 docs/**/*.md 的
#      markdown 相对链接存在性抽查 + git diff --check。
#   L1 单条 cargo 命令带齐 features 跑 pawork-providers 全部测试目标
#      （口径见 docs/spec/crates/providers.md §7，另加 kimi-code 覆盖其
#      feature 门控的 lib 测试，如 KimiCodeProvider）；--packages a,b 可
#      追加写入集定向包，同一条 Cargo 命令执行。
#   L2（不触外网）fixture 脱敏、OAuth、配置恢复/凭证边界回归；复用 server_smoke.py / server_scenarios_smoke.py
#      （内部各自以随机空闲端口启动 mock server）；/usage 录制字节及形状
#      在 server_smoke.py 的录制回放阶段一次验证，不重复启动 server。
#   L3 真实 Provider 冒烟不入门禁：保持手动触发并单独记录，口径为
#      docs/spec/verification.md §2.1（opencode-go / glm-5.3-flash）。
#
# 用法：
#   ./scripts/mock/gate.sh
#   ./scripts/mock/gate.sh --packages pawork-auth,pawork-app
#   ./scripts/mock/gate.sh --level 0,2

set -u -o pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO_ROOT"

usage() {
  cat <<'USAGE'
MOCK-8 mock 快速门禁（快速定向门禁，非全量门禁）

用法：
  ./scripts/mock/gate.sh [--packages <pkg1,pkg2,...>] [--level <0,1,2 的子集>]

级别：
  L0  文档抽查：docs/ROADMAP.md 与当前改动过的 docs/**/*.md 的
      markdown 相对链接存在性 + git diff --check（秒级）
  L1  cargo test -p pawork-providers --offline --tests --features <全套
      含 kimi-code（覆盖其 feature 门控 lib 测试）>
      （单条命令、单 Cargo 进程）；--packages 追加 -p <pkg>，去重后一次执行
      --tests 包含 lib / bin 的单测及集成目标；Desktop 走其 Spec 专用命令
  L2  mock 回归（不触外网）：capture verify + oauth_selftest + review_selftest
      + server_smoke.py + server_scenarios_smoke.py
      （server_smoke 已含录制树 /usage 字节与形状验证）
  L3  真实 Provider 冒烟不入门禁：保持手动，按 docs/spec/verification.md §2.1
      （opencode-go / glm-5.3-flash）口径执行并单独记录

退出码：0 全绿；1 用法错误；2 L0 失败；3 L1 失败；4 L2 失败。
本门禁不改变仓库「当前未设置全量门禁」的约定。

--level 只跑指定级别（如 --level 0,2），便于并行开发期 cargo 锁占用时
排障；默认全跑。未选级别的退出码语义不变。
USAGE
}

extra_csv=""
run_l0=1
run_l1=1
run_l2=1

parse_levels() {
  run_l0=0
  run_l1=0
  run_l2=0
  old_ifs="$IFS"
  IFS=','
  for item in $1; do
    item="$(printf '%s' "$item" | tr -d ' \t')"
    case "$item" in
      0) run_l0=1 ;;
      1) run_l1=1 ;;
      2) run_l2=1 ;;
      '') ;;
      *)
        echo "gate: 未知级别：""$item""（可用：0,1,2）" >&2
        IFS="$old_ifs"
        return 1
        ;;
    esac
  done
  IFS="$old_ifs"
  if [ "$run_l0" -eq 0 ] && [ "$run_l1" -eq 0 ] && [ "$run_l2" -eq 0 ]; then
    echo "gate: --level 未指定任何有效级别（可用：0,1,2）" >&2
    return 1
  fi
}

while [ $# -gt 0 ]; do
  case "$1" in
    --packages)
      shift
      if [ $# -eq 0 ]; then
        echo "gate: --packages 需要一个逗号分隔的包列表" >&2
        exit 1
      fi
      if [ -n "$extra_csv" ]; then
        extra_csv="$extra_csv,$1"
      else
        extra_csv="$1"
      fi
      ;;
    --packages=*)
      value="$(printf '%s' "$1" | cut -d= -f2-)"
      if [ -n "$extra_csv" ]; then
        extra_csv="$extra_csv,$value"
      else
        extra_csv="$value"
      fi
      ;;
    --level)
      shift
      if [ $# -eq 0 ]; then
        echo "gate: --level 需要一个逗号分隔的级别列表（0,1,2）" >&2
        exit 1
      fi
      parse_levels "$1"
      if [ $? -ne 0 ]; then
        exit 1
      fi
      ;;
    --level=*)
      parse_levels "$(printf '%s' "$1" | cut -d= -f2-)"
      if [ $? -ne 0 ]; then
        exit 1
      fi
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "gate: 未知参数：$1" >&2
      usage >&2
      exit 1
      ;;
  esac
  shift
done

now_ms() { python3 -c 'import time; print(int(round(time.time() * 1000)))'; }
fmt_s() { awk -v ms="$1" 'BEGIN { printf "%.1fs", ms / 1000 }'; }

l0_ms=0
l1_ms=0
l2_ms=0

fail_level() {
  level="$1"
  exit_code="$2"
  echo ""
  echo "GATE FAIL at L""$level""（分级耗时：$(level_summary)）"
  exit "$exit_code"
}

level_summary() {
  parts=""
  if [ "$run_l0" -eq 1 ]; then
    parts="L0 $(fmt_s "$l0_ms")"
  fi
  if [ "$run_l1" -eq 1 ]; then
    if [ -n "$parts" ]; then
      parts="$parts | "
    fi
    parts="$parts""L1 $(fmt_s "$l1_ms")"
  fi
  if [ "$run_l2" -eq 1 ]; then
    if [ -n "$parts" ]; then
      parts="$parts | "
    fi
    parts="$parts""L2 $(fmt_s "$l2_ms")"
  fi
  printf '%s' "$parts"
}

level0() {
  echo "=== L0 文档链接抽查 + git diff --check ==="
  target_list="$(mktemp)"
  printf '%s\n' docs/ROADMAP.md > "$target_list"
  changed="$(
    {
      git diff --name-only HEAD -- docs
      git ls-files -o --exclude-standard -- ':(glob)docs/**/*.md'
    } | sort -u | grep '\.md$' || true
  )"
  printf '%s\n' "$changed" | while IFS= read -r file; do
    if [ -n "$file" ] && [ -f "$file" ] && ! grep -Fxq "$file" "$target_list"; then
      printf '%s\n' "$file" >> "$target_list"
    fi
  done
  python3 - "$target_list" <<'PYLINK'
import re
import sys
from pathlib import Path

repo = Path.cwd()
fence = chr(96) * 3
link_re = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
broken = []
checked = 0
listing = Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()
for arg in [line.strip() for line in listing if line.strip()]:
    md = repo / arg
    if not md.is_file():
        broken.append(arg + ": 文件不存在")
        continue
    checked += 1
    in_fence = False
    for lineno, line in enumerate(md.read_text(encoding="utf-8").splitlines(), 1):
        if line.lstrip().startswith(fence):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        for target in link_re.findall(line):
            if target.startswith(("#", "mailto:", "http://", "https://", "data:", "//", "<")):
                continue
            path = target.split("#", 1)[0]
            if not path:
                continue
            if not (md.parent / path).resolve().exists():
                broken.append(arg + ":" + str(lineno) + ": " + target)
if broken:
    print("L0 FAIL markdown 相对链接缺失：")
    for item in broken:
        print("  " + item)
    sys.exit(1)
print("L0 markdown 相对链接 OK（" + str(checked) + " 个文件）")
PYLINK
  if [ $? -ne 0 ]; then
    rm -f "$target_list"
    return 1
  fi
  rm -f "$target_list"

  if ! git diff --check HEAD; then
    echo "L0 FAIL git diff --check 报告了空白错误"
    return 1
  fi
  echo "L0 git diff --check OK"
}

level1() {
  local packages
  local selected=(pawork-providers)
  local pkg
  # shared entry owns package validation, deduplication and feature selection.
  IFS=',' read -r -a packages <<< "pawork-providers,$extra_csv"
  for pkg in "${packages[@]}"; do
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
  echo "=== L2 fixture 脱敏/形状、OAuth 与配置恢复回归 ==="
  python3 scripts/mock/capture.py verify || return 1
  python3 scripts/mock/oauth_selftest.py || return 1
  python3 scripts/mock/review_selftest.py || return 1
  echo "=== L2 server_smoke.py（§2.2 端点形状 + §2.3 usage 形状） ==="
  python3 scripts/mock/server_smoke.py
  if [ $? -ne 0 ]; then
    echo "L2 FAIL server_smoke.py"
    return 1
  fi

  echo ""
  echo "=== L2 server_scenarios_smoke.py（场景库：错误归一 / 截断 / 定速 / 触发） ==="
  python3 scripts/mock/server_scenarios_smoke.py
  if [ $? -ne 0 ]; then
    echo "L2 FAIL server_scenarios_smoke.py"
    return 1
  fi

}

gate_start="$(now_ms)"
echo "MOCK-8 mock 快速门禁（快速定向门禁，非全量门禁；L3 真实冒烟保持手动）"
echo "repo: $REPO_ROOT"

if [ "$run_l0" -eq 1 ]; then
  t="$(now_ms)"
  level0
  rc=$?
  l0_ms=$(( $(now_ms) - t ))
  if [ "$rc" -ne 0 ]; then
    fail_level 0 2
  fi
  echo "[L0] PASS $(fmt_s "$l0_ms")"
fi

if [ "$run_l1" -eq 1 ]; then
  t="$(now_ms)"
  level1
  rc=$?
  l1_ms=$(( $(now_ms) - t ))
  if [ "$rc" -ne 0 ]; then
    fail_level 1 3
  fi
  echo "[L1] PASS $(fmt_s "$l1_ms")"
fi

if [ "$run_l2" -eq 1 ]; then
  t="$(now_ms)"
  level2
  rc=$?
  l2_ms=$(( $(now_ms) - t ))
  if [ "$rc" -ne 0 ]; then
    fail_level 2 4
  fi
  echo "[L2] PASS $(fmt_s "$l2_ms")"
fi

total_ms=$(( $(now_ms) - gate_start ))
echo ""
if [ "$run_l0" -eq 1 ] && [ "$run_l1" -eq 1 ] && [ "$run_l2" -eq 1 ]; then
  pass_label="L0/L1/L2 全绿"
else
  pass_label="所选级别全绿"
fi
echo "GATE PASS（""$pass_label""）总耗时 $(fmt_s "$total_ms")"
echo "  $(level_summary)"
echo "L3（不入门禁）：真实 Provider 冒烟保持手动，按 docs/spec/verification.md §2.1（opencode-go / glm-5.3-flash）口径执行并单独记录。"
exit 0
