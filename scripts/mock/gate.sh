#!/usr/bin/env bash
#
# MOCK-8 mock 快速门禁：单入口四级定向验证。
#
# 定位：这是 mock 系列的快速定向门禁，不是全量门禁；仓库「当前未设置全量
# 门禁」的约定不变。任一级失败即非零退出并指明级别。
#
#   L0（秒级）docs/mock-simulation-plan.md 与当前改动过的 docs/**/*.md 的
#      markdown 相对链接存在性抽查 + git diff --check。
#   L1 单条 cargo 命令带齐 features 跑 pawork-providers 全部测试目标
#      （口径见 docs/spec/crates/providers.md §7，另加 kimi-code 覆盖其
#      feature 门控的 lib 测试，如 KimiCodeProvider）；--packages a,b 可
#      追加写入集定向包，逐条串行执行，遵守单 Cargo 进程纪律。
#   L2（不触外网）fixture 脱敏、OAuth、配置恢复/凭证边界回归；复用 server_smoke.py / server_scenarios_smoke.py
#      （只调用不改，内部各自以随机空闲端口启动 mock server）；另起一个
#      127.0.0.1 随机空闲端口 mock server 回放 fixtures/mock，断言 /usage
#      回放字节一致且满足 §2.3 三窗形状（能捕获录制 fixture 被改坏）。
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

PROVIDERS_FEATURES="anthropic,chatgpt-oauth,xai-oauth,glm-coding,opencode-go,qwen-token-plan,deepseek,kimi-platform,kimi-code"

usage() {
  cat <<'USAGE'
MOCK-8 mock 快速门禁（快速定向门禁，非全量门禁）

用法：
  ./scripts/mock/gate.sh [--packages <pkg1,pkg2,...>] [--level <0,1,2 的子集>]

级别：
  L0  文档抽查：docs/mock-simulation-plan.md 与当前改动过的 docs/**/*.md 的
      markdown 相对链接存在性 + git diff --check（秒级）
  L1  cargo test -p pawork-providers --offline --lib --tests --features <全套
      含 kimi-code（覆盖其 feature 门控 lib 测试）>
      （单条命令、单 Cargo 进程）；--packages 追加的定向包逐条串行执行
      cargo test -p <pkg> --offline --lib --tests
  L2  mock 回归（不触外网）：capture verify + oauth_selftest + review_selftest
      + server_smoke.py + server_scenarios_smoke.py
      + 录制树 /usage 回放字节与 §2.3 形状断言（随机空闲端口，trap 清理）
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
  printf '%s\n' docs/mock-simulation-plan.md > "$target_list"
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
  echo "=== L1 providers 全套测试（单条命令、单 Cargo 进程） ==="
  echo "$ cargo test -p pawork-providers --offline --lib --tests --features $PROVIDERS_FEATURES"
  cargo test -p pawork-providers --offline --lib --tests --features "$PROVIDERS_FEATURES"
  if [ $? -ne 0 ]; then
    echo "L1 FAIL pawork-providers"
    return 1
  fi

  if [ -n "$extra_csv" ]; then
    pkg_list="$(mktemp)"
    printf '%s' "$extra_csv" | tr ',' '\n' | tr -d ' \t' | grep -v '^$' \
      | awk '!seen[$0]++ && $0 != "pawork-providers"' > "$pkg_list"
    while IFS= read -r pkg; do
      [ -n "$pkg" ] || continue
      echo ""
      echo "=== L1 追加定向包：""$pkg""（串行，单 Cargo 进程） ==="
      echo "$ cargo test -p $pkg --offline --lib --tests"
      cargo test -p "$pkg" --offline --lib --tests
      if [ $? -ne 0 ]; then
        echo "L1 FAIL $pkg"
        rm -f "$pkg_list"
        return 1
      fi
    done < "$pkg_list"
    rm -f "$pkg_list"
  fi
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

  echo ""
  echo "=== L2 录制树 /usage 回放：字节一致 + §2.3 三窗形状（随机端口） ==="
  server_log="$(mktemp)"
  server_pid=""
  cleanup_server() {
    if [ -n "$server_pid" ] && kill -0 "$server_pid" 2>/dev/null; then
      kill "$server_pid" 2>/dev/null || true
      wait "$server_pid" 2>/dev/null || true
    fi
    rm -f "$server_log"
  }
  trap cleanup_server EXIT
  python3 scripts/mock/server.py --host 127.0.0.1 --port 0 --fixtures-root fixtures/mock > "$server_log" 2>&1 &
  server_pid=$!
  base_url=""
  for _i in $(seq 1 100); do
    base_url="$(grep -m1 -o 'http://[0-9.]*:[0-9]*' "$server_log" || true)"
    [ -n "$base_url" ] && break
    kill -0 "$server_pid" 2>/dev/null || break
    sleep 0.05
  done
  if [ -z "$base_url" ]; then
    echo "L2 FAIL mock server 未在随机端口上报 listening；日志："
    cat "$server_log" >&2
    cleanup_server
    server_pid=""
    trap - EXIT
    return 1
  fi
  python3 - "$base_url" <<'PYUSAGE'
import json
import sys
from pathlib import Path
from urllib import request as urlrequest

sys.path.insert(0, "scripts/mock")
import server_smoke as smoke  # 复用 §2.3 红线校验，单一来源

req = urlrequest.Request(
    sys.argv[1] + "/usage",
    headers={"Authorization": "Bearer mock-opencode-go"},
    method="GET",
)
try:
    with urlrequest.urlopen(req, timeout=10) as resp:
        status, body = resp.status, resp.read()
except Exception as error:  # noqa: BLE001
    print("L2 FAIL /usage 请求失败：" + repr(error))
    sys.exit(1)
try:
    payload = json.loads(body)
except Exception:  # noqa: BLE001
    payload = None
expected = Path("fixtures/mock/opencode-go/usage.json").read_bytes()
problems = []
if status != 200:
    problems.append("status=" + str(status))
if body != expected:
    problems.append("回放字节与 fixtures/mock/opencode-go/usage.json 不一致")
if not (isinstance(payload, dict) and smoke.valid_go_usage(payload)):
    problems.append("usage 形状违反 §2.3 红线（三窗独立 / percent 整数区间 / resetsAt 严格日历）")
if problems:
    print("L2 FAIL 录制树 /usage：" + "；".join(problems))
    sys.exit(1)
print("L2 录制树 /usage 回放 OK（字节一致 + §2.3 形状合法）")
PYUSAGE
  rc=$?
  cleanup_server
  server_pid=""
  trap - EXIT
  if [ "$rc" -ne 0 ]; then
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
