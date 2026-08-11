#!/usr/bin/env bash
# P15-9 · Phase 15 功能簇集中门禁（contract / golden / fuzz / 兼容性）。
#
# 设计要点（见 plan/P15-9-provider-contract-v2.md 与 arch-rules）：
# - 使用独立 CARGO_TARGET_DIR=target/gates，与日常 target/ 物理隔离；
# - trap EXIT/INT/TERM 在 finally 执行 cargo clean（且兜底 rm），无论成败
#   都不残留隔离构建缓存；
# - 依次跑 contract / golden / fuzz / 兼容性 四类，逐类汇总 PASS/FAIL；
# - 全程不跑 workspace 全量测试，仅覆盖 P15-9 写集涉及的 crate 测试 target。
#
# 用法：./scripts/p15-gate.sh
#   可选环境变量：
#     P15_GATE_KEEP_TARGET=1   门禁结束后保留 target/gates（调试用）

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

GATE_TARGET="$ROOT/target/gates"
export CARGO_TARGET_DIR="$GATE_TARGET"
# golden 快照必须与已提交基线匹配，禁止门禁内自动改写。
export INSTA_UPDATE=no
# 门禁不依赖网络（wiremock 全本地）；保留默认离线策略可控。
export CARGO_NET_OFFLINE="${CARGO_NET_OFFLINE:-false}"

mkdir -p "$GATE_TARGET"

clean_gate_dir() {
  local keep="${P15_GATE_KEEP_TARGET:-0}"
  if [ "$keep" = "1" ]; then
    echo "[p15-gate] keep isolated target dir (P15_GATE_KEEP_TARGET=1): $GATE_TARGET"
    return 0
  fi
  echo "[p15-gate] cleaning isolated target dir: $GATE_TARGET"
  cargo clean --target-dir "$GATE_TARGET" >/dev/null 2>&1 || true
  # 兜底：cargo clean 偶有锁竞争残留，物理移除以防污染日常 target。
  rm -rf "$GATE_TARGET" >/dev/null 2>&1 || true
}
trap clean_gate_dir EXIT INT TERM

# 单条 cargo 命令运行（参数以单个字符串传入，运行时按空格拆分）。
run_cargo() {
  local args="$1"
  echo "[p15-gate] > CARGO_TARGET_DIR=target/gates cargo test $args"
  cargo test $args
}

# 一个门禁类别由一条或多条 cargo 命令组成；任一失败则类别失败，但继续后续类别。
run_category() {
  local name="$1"
  shift
  echo ""
  echo "===== [p15-gate] category: $name ====="
  local rc=0
  local args
  for args in "$@"; do
    if ! run_cargo "$args"; then
      rc=1
    fi
  done
  if [ "$rc" -eq 0 ]; then
    echo "[p15-gate] $name: PASS"
  else
    echo "[p15-gate] $name: FAIL"
  fi
  return $rc
}

OVERALL=0

# filter 使用 `p15_gate::<module>::` 前缀，精确命中各 provider
# tests/p15_gate.rs 内对应子模块，不串扰同包其他 test target。

run_category "contract" \
  "-p provider-openai -p provider-anthropic -p provider-xai --test p15_gate contract::" \
  "-p agent-engine --test no_provider_branch" \
  || OVERALL=1

run_category "golden" \
  "-p provider-openai -p provider-anthropic -p provider-xai --test p15_gate golden::" \
  || OVERALL=1

run_category "fuzz" \
  "-p provider-openai -p provider-anthropic -p provider-xai --test p15_gate fuzz::" \
  || OVERALL=1

run_category "compat" \
  "-p provider-openai -p provider-anthropic -p provider-xai --test p15_gate compat::" \
  || OVERALL=1

echo ""
echo "===== [p15-gate] summary ====="
if [ "$OVERALL" -eq 0 ]; then
  echo "[p15-gate] ALL CATEGORIES PASS"
else
  echo "[p15-gate] ONE OR MORE CATEGORIES FAILED"
fi
exit "$OVERALL"
