#!/usr/bin/env bash
# P18-15 · Phase 18 Control Plane Contract / Security Gate（L2，不跑 workspace 全量）。
#
# 设计要点（参照 scripts/p15-gate.sh / scripts/phase17-host-gate.sh）：
# - 隔离 CARGO_TARGET_DIR（默认 target/gates-p18，可用 P18_GATE_TARGET_DIR 覆盖），
#   与日常 target/ 以及其他阶段的 target/gates 物理隔离；
# - trap EXIT/INT/TERM 在 finally 执行 cargo clean --target-dir（且兜底 rm -rf），
#   无论成败都不残留隔离构建缓存；
# - P18_GATE_TARGET_DIR 仅接受仓库 $ROOT/target/ 下的一级子目录，非法值启动前
#   拒绝；trap 只清理通过校验的路径；
# - run_cargo 捕获 cargo 输出：cargo 成功但测试实际 0 matched / 0 passed 视为失败，
#   并保留 cargo 原始退出码与完整日志；
# - 多类门禁逐类汇总 PASS/FAIL，单类失败不中断后续类别；
# - 全程不跑 cargo test --workspace / clippy --workspace。
#
# 用法：./scripts/p18-gate.sh
#   可选环境变量：
#     P18_GATE_TARGET_DIR=<dir>   覆盖隔离 target 目录（默认 target/gates-p18；
#                                 仅允许 $ROOT/target/ 下的一级子目录）
#     P18_GATE_KEEP_TARGET=1      门禁结束后保留隔离 target 目录（调试用）

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

# P18_GATE_TARGET_DIR 只允许仓库 $ROOT/target/ 下的一级子目录；仓库外路径、
# $ROOT/target 自身、多级嵌套、相对路径、含 .. 段等一律在启动前拒绝。
GATE_TARGET_PREFIX="$ROOT/target/"
is_valid_gate_target_dir() {
  local dir="$1"
  local rel
  case "$dir" in
    "${GATE_TARGET_PREFIX}"?*) ;;
    *) return 1 ;;
  esac
  rel="${dir#"${GATE_TARGET_PREFIX}"}"
  case "$rel" in
    "" | . | .. | */*) return 1 ;;
  esac
  return 0
}

GATE_TARGET="${P18_GATE_TARGET_DIR:-$ROOT/target/gates-p18}"
if ! is_valid_gate_target_dir "$GATE_TARGET"; then
  echo "[p18-gate] ERROR: P18_GATE_TARGET_DIR 必须是 $ROOT/target/ 下的一级子目录（实际：${GATE_TARGET}）" >&2
  exit 1
fi
# 固化通过校验的路径：clean_gate_dir 只清理它，不再引用任何未验证输入。
GATE_TARGET_VALIDATED="$GATE_TARGET"
export CARGO_TARGET_DIR="$GATE_TARGET"
# 快照必须与已提交基线匹配，禁止门禁内自动改写。
export INSTA_UPDATE=no
export CARGO_NET_OFFLINE="${CARGO_NET_OFFLINE:-false}"

# run_cargo 逐次捕获输出：日志实时可见，同时用于「0 matched / 0 passed」检查。
GATE_TMP="$(mktemp -d "${TMPDIR:-/tmp}/p18-gate.XXXXXX")" || {
  echo "[p18-gate] ERROR: 无法创建临时日志目录" >&2
  exit 1
}
GATE_RUN_SEQ=0

mkdir -p "$GATE_TARGET"

clean_gate_dir() {
  local keep="${P18_GATE_KEEP_TARGET:-0}"
  if [ "$keep" = "1" ]; then
    echo "[p18-gate] keep isolated target dir (P18_GATE_KEEP_TARGET=1): $GATE_TARGET_VALIDATED"
  else
    echo "[p18-gate] cleaning isolated target dir: $GATE_TARGET_VALIDATED"
    cargo clean --target-dir "$GATE_TARGET_VALIDATED" >/dev/null 2>&1 || true
    # 兜底：cargo clean 偶有锁竞争残留，物理移除以防污染日常 target。
    rm -rf "$GATE_TARGET_VALIDATED" >/dev/null 2>&1 || true
  fi
  # 临时日志目录与 P18_GATE_KEEP_TARGET 无关，始终清理。
  if [ -n "${GATE_TMP:-}" ]; then
    rm -rf "$GATE_TMP" >/dev/null 2>&1 || true
  fi
}
trap clean_gate_dir EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

run_cargo() {
  local args="$1"
  local out
  local rc
  GATE_RUN_SEQ=$((GATE_RUN_SEQ + 1))
  out="$GATE_TMP/run-$GATE_RUN_SEQ.log"
  echo "[p18-gate] > CARGO_TARGET_DIR=$GATE_TARGET cargo $args"
  # tee 保留实时日志；PIPESTATUS[0] 取 cargo 原始退出码（不被 tee 覆盖）。
  cargo $args 2>&1 | tee "$out"
  rc=${PIPESTATUS[0]}
  # cargo 成功但测试实际 0 matched / 0 passed：疑似 filter 未命中或静默排除，
  # 视为失败；cargo 自身失败时原样保留退出码与日志。
  if [ "$rc" -eq 0 ] \
    && ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$out" \
    && grep -Eq '^test result: ok\. 0 passed' "$out"; then
    echo "[p18-gate] FAIL: cargo 退出码为 0，但测试实际 0 matched / 0 passed（疑似 filter 未命中或静默排除）"
    rc=1
  fi
  return "$rc"
}

run_category() {
  local name="$1"
  shift
  echo ""
  echo "===== [p18-gate] category: $name ====="
  local rc=0
  local args
  for args in "$@"; do
    if ! run_cargo "$args"; then
      rc=1
    fi
  done
  if [ "$rc" -eq 0 ]; then
    echo "[p18-gate] $name: PASS"
  else
    echo "[p18-gate] $name: FAIL"
  fi
  return $rc
}

OVERALL=0

# 1) Selector / property：routing（priority / weighted / fill-first / affinity）
#    与 binding（rebind）。--lib 避开 integration target；已知 flaky
#    wait_for_probe_dedups_repeated_poll_of_same_waker 在 model-registry，不在本 crate。
run_category "selector-property" \
  "test -p provider-control --lib routing::" \
  "test -p provider-control --lib binding::" \
  || OVERALL=1

# 2) Concurrency / recovery：orchestration lease/Agent 上限与 reclaim；
#    tenant_policy 同时覆盖跨租户隔离与 export redaction（P18-13，不重复写测）。
run_category "concurrency-recovery" \
  "test -p orchestration --lib" \
  "test -p app-service --test tenant_policy" \
  || OVERALL=1

# 3) Migration / security：控制面迁移、tenant/audit/usage，以及跨 crate schema 常量对齐。
#    不与 session-store::CURRENT_SCHEMA_VERSION（9，另一 schema 族）等同。
run_category "migration-security" \
  "test -p app-database control_plane::" \
  "test -p tenant-service" \
  "test -p audit-log" \
  "test -p usage-ledger --lib" \
  "test -p app-service --test control_plane_schema" \
  || OVERALL=1

# 4) Protocol golden：Codex / Claude / ACP / IDE contract。
#    host_mock 覆盖 lifecycle / diagnostics / apply-diff-approval 回路，随 P17-9 延期落点纳入。
run_category "protocol-golden" \
  "test -p client-codex-app-server --test golden --test handshake --test lifecycle --test capabilities" \
  "test -p client-claude-gateway" \
  "test -p acp-host" \
  "test -p ide-host-adapter --test contract --test host_mock" \
  || OVERALL=1

# 5) Error / fault：既有 401/429/QuotaExceeded/cancel 矩阵；不新写 Provider mock。
run_category "error-fault" \
  "test -p provider-control --test error_matrix" \
  "test -p quota-service --lib both_endpoints_401" \
  "test -p quota-service --lib both_endpoints_429" \
  || OVERALL=1

# 6) Rollback：P18-1 schema backup/restore + feature-off legacy 回退。
#    不发明独立 restore runbook；feature-off 只跑始终可用的 legacy 模块。
run_category "rollback" \
  "test -p app-database rollback" \
  "test -p provider-control --no-default-features --lib legacy" \
  || OVERALL=1

# 7) Host Route→Lease：持久池 picker 不再回退 LegacyCredentialPicker；
#    注入仓库后 run 走 RoutingPolicy；缺 provider 仍 fail-closed。
run_category "host-route-lease" \
  "test -p provider-control --lib repository_picker_tests::" \
  "test -p core-runtime --lib persistent_pool_acquire_missing_account_is_no_candidate" \
  "test -p core-runtime --lib control_plane_repository_" \
  "test -p app-service --test credential_lease" \
  "test -p app-service --test tenant_policy run_records_route_candidate_when_repository_injected" \
  "test -p pawork --test modes run_mode_reports_structured_error_without_provider" \
  || OVERALL=1

# 8) Clippy：本轮 changed crates（provider-control / app-service / core-runtime /
#    pawork）必须覆盖，不得再因既有告警整 crate 静默剔除。既有告警只以显式 -A
#    逐项豁免，且仅作用于本轮 changed crates 的这次调用，其余 crate 保持 -D warnings：
#    - provider-control：binding.rs clippy::too_many_arguments（commit_rebind / rebind_after_release）
#    不 rustfmt 全世界，不给既有代码加 allow。
run_category "clippy-related" \
  "clippy -p provider-control -p app-service -p core-runtime -p pawork --all-targets --no-deps -- -D warnings -A clippy::too_many_arguments" \
  "clippy -p tenant-service -p usage-ledger -p audit-log -p client-adapter-api -p client-codex-app-server -p client-claude-gateway -p acp-host -p orchestration -p ide-host-adapter --all-targets --no-deps -- -D warnings" \
  || OVERALL=1

echo ""
echo "===== [p18-gate] summary ====="
if [ "$OVERALL" -eq 0 ]; then
  echo "[p18-gate] ALL CATEGORIES PASS"
else
  echo "[p18-gate] ONE OR MORE CATEGORIES FAILED"
fi
exit "$OVERALL"
