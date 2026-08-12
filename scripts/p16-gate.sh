#!/usr/bin/env bash
# P16 · Phase 16 Modern Agent Workflow 定向门禁（L2，不跑 workspace 全量）。
#
# 设计要点（参照 scripts/p15-gate.sh）：
# - 独立 CARGO_TARGET_DIR=target/gates，与日常 target/ 物理隔离；
# - trap EXIT/INT/TERM 在 finally 执行 cargo clean（且兜底 rm），无论成败
#   都不残留隔离构建缓存；
# - 四类门禁：P16 crates test / P16 crates clippy / app-service 正式链 /
#   schema check，逐类汇总 PASS/FAIL，单类失败不中断后续类别；
# - 正式链 = cargo check -p app-service（编译闭包 app-service → agent-engine
#   → agent-events/agent-domain，覆盖 Phase 16 新增 AgentEvent 变体的穷举处理）
#   + 两条 workflow 事件折叠回归（agent-engine::recovery 与
#   app-service::supervisor 的 workflow_events 测试，过滤无匹配即失败）；
# - schema check = cargo run -p schema-typegen -- --check（只比对已提交声明，
#   不在门禁内改写 schemas/）；
# - 全程不跑 workspace 全量测试，仅覆盖 P16 写集涉及的 crate 测试 target。
#
# 用法：./scripts/p16-gate.sh
#   可选环境变量：
#     P16_GATE_KEEP_TARGET=1   门禁结束后保留 target/gates（调试用）

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

GATE_TARGET="$ROOT/target/gates"
export CARGO_TARGET_DIR="$GATE_TARGET"
# 快照必须与已提交基线匹配，禁止门禁内自动改写。
export INSTA_UPDATE=no
# 门禁不依赖网络；保留默认离线策略可控。
export CARGO_NET_OFFLINE="${CARGO_NET_OFFLINE:-false}"

mkdir -p "$GATE_TARGET"

clean_gate_dir() {
  local keep="${P16_GATE_KEEP_TARGET:-0}"
  if [ "$keep" = "1" ]; then
    echo "[p16-gate] keep isolated target dir (P16_GATE_KEEP_TARGET=1): $GATE_TARGET"
    return 0
  fi
  echo "[p16-gate] cleaning isolated target dir: $GATE_TARGET"
  cargo clean --target-dir "$GATE_TARGET" >/dev/null 2>&1 || true
  # 兜底：cargo clean 偶有锁竞争残留，物理移除以防污染日常 target。
  rm -rf "$GATE_TARGET" >/dev/null 2>&1 || true
}
trap clean_gate_dir EXIT INT TERM

# 单条 cargo 命令运行（参数以单个字符串传入，运行时按空格拆分）。
run_cargo() {
  local args="$1"
  echo "[p16-gate] > CARGO_TARGET_DIR=target/gates cargo $args"
  cargo $args
}

# 一个门禁类别由一条或多条 cargo 命令组成；任一失败则类别失败，但继续后续类别。
run_category() {
  local name="$1"
  shift
  echo ""
  echo "===== [p16-gate] category: $name ====="
  local rc=0
  local args
  for args in "$@"; do
    if ! run_cargo "$args"; then
      rc=1
    fi
  done
  if [ "$rc" -eq 0 ]; then
    echo "[p16-gate] $name: PASS"
  else
    echo "[p16-gate] $name: FAIL"
  fi
  return $rc
}

OVERALL=0

# P16 相关 crates：
# - 7 个 Phase 16 新 crate：plan-service / goal-service / task-manager /
#   automation-service / monitor-service / memory-service / review-engine；
# - 共享 canonical 域：agent-domain（workflow 领域类型）、agent-events
#   （AgentEvent 的 7 个 P16 wrapping 变体）、provider-api
#   （P16-7 扩展的 canonical EmbeddingProvider）；
# - session-store（P16-9 外部会话兼容导入）。

run_category "crates-test" \
  "test -p agent-domain -p agent-events -p provider-api" \
  "test -p plan-service -p goal-service" \
  "test -p task-manager -p automation-service -p monitor-service" \
  "test -p memory-service -p review-engine" \
  "test -p session-store" \
  || OVERALL=1

run_category "crates-clippy" \
  "clippy -p agent-domain -p agent-events -p provider-api -p plan-service -p goal-service -p task-manager -p automation-service -p monitor-service -p memory-service -p review-engine -p session-store --all-targets -- -D warnings" \
  || OVERALL=1

run_category "official-chain" \
  "check -p app-service" \
  "test -p agent-engine --lib workflow_events" \
  "test -p app-service --lib workflow_events" \
  || OVERALL=1

run_category "schema-check" \
  "run -p schema-typegen -- --check" \
  || OVERALL=1

echo ""
echo "===== [p16-gate] summary ====="
if [ "$OVERALL" -eq 0 ]; then
  echo "[p16-gate] ALL CATEGORIES PASS"
else
  echo "[p16-gate] ONE OR MORE CATEGORIES FAILED"
fi
exit "$OVERALL"
