#!/usr/bin/env bash
# P17-12 · Phase 17 Host Cluster 定向门禁（L2，不跑 workspace 全量）。
#
# 设计要点（参照 scripts/p16-gate.sh）：
# - 隔离 CARGO_TARGET_DIR（默认 target/gates，可用 PHASE17_GATE_TARGET_DIR 覆盖），
#   与日常 target/ 物理隔离；
# - trap EXIT/INT/TERM 在 finally 执行 cargo clean --target-dir（且兜底 rm -rf），
#   无论成败都不残留隔离构建缓存；
# - 四类门禁：cluster test / cluster clippy / 边界检查 / 失败清理自检，
#   逐类汇总 PASS/FAIL，单类失败不中断后续类别；
# - cluster test / clippy 覆盖 Phase 17 host 集群全部 7 个 crate：
#   acp-host / agent-sdk / headless-json / ide-host-adapter /
#   browser-computer-runtime / transport-remote / remote-control-adapter
#   （remote-control-adapter 测试含 tests/transport_remote_carrier.rs 承载集成测试，
#     transport-remote 承载真实远程 transport 测试）；
# - 边界检查为纯静态检查：每个集群 crate 必须存在于 crates/<name>/Cargo.toml 且
#   登记于 workspace members；[dependencies] 不得出现
#   gui-protocol / gui-server / gui-client / agent-engine（agent-engine 默认禁止，
#   当前 7 个 crate 均无必需直依赖）；
#   remote-control-adapter 另保留原检查：session-store / app-database /
#   provider-* / transport-remote 禁止直依赖；必需直依赖 agent-domain /
#   app-service / core-api / subscription-hub / transport-api 必须在场
#   （Core 单一事实源方向）；transport-remote 仅允许以 dev-dependency 形式
#   承载集成测试；
# - 失败清理自检：以 PHASE17_GATE_FORCE_FAIL=1 递归调用本脚本（指向独立隔离
#   目录），断言内层非零退出且隔离目录已被 trap 清理；
# - 全程不跑 workspace 全量测试/clippy，仅覆盖 Phase 17 host 集群写集。
#
# 用法：./scripts/phase17-host-gate.sh
#   可选环境变量：
#     PHASE17_GATE_TARGET_DIR=<dir>   覆盖隔离 target 目录（默认 target/gates）
#     PHASE17_GATE_KEEP_TARGET=1      门禁结束后保留隔离 target 目录（调试用）
#     PHASE17_GATE_CLEANUP_TEST=1     仅运行「失败清理自检」入口
#     PHASE17_GATE_FORCE_FAIL=1       强制失败类别（内部用，供清理自检）

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

GATE_TARGET="${PHASE17_GATE_TARGET_DIR:-$ROOT/target/gates}"
export CARGO_TARGET_DIR="$GATE_TARGET"
# 门禁内不允许自动改写快照。
export INSTA_UPDATE=no
export CARGO_NET_OFFLINE="${CARGO_NET_OFFLINE:-false}"

# Phase 17 host 集群 crate（remote-control-adapter 兜底保留其定向边界检查）。
CLUSTER_CRATES="acp-host agent-sdk headless-json ide-host-adapter browser-computer-runtime transport-remote remote-control-adapter"
CLUSTER_P_FLAGS=""
for crate in $CLUSTER_CRATES; do
  CLUSTER_P_FLAGS="$CLUSTER_P_FLAGS -p $crate"
done

mkdir -p "$GATE_TARGET"

clean_gate_dir() {
  local keep="${PHASE17_GATE_KEEP_TARGET:-0}"
  if [ "$keep" = "1" ]; then
    echo "[phase17-gate] keep isolated target dir (PHASE17_GATE_KEEP_TARGET=1): $GATE_TARGET"
    return 0
  fi
  echo "[phase17-gate] cleaning isolated target dir: $GATE_TARGET"
  cargo clean --target-dir "$GATE_TARGET" >/dev/null 2>&1 || true
  # 兜底：cargo clean 偶有锁竞争残留，物理移除以防污染日常 target。
  rm -rf "$GATE_TARGET" >/dev/null 2>&1 || true
}
trap clean_gate_dir EXIT INT TERM

run_cargo() {
  local args="$1"
  echo "[phase17-gate] > CARGO_TARGET_DIR=$GATE_TARGET cargo $args"
  cargo $args
}

run_category() {
  local name="$1"
  shift
  echo ""
  echo "===== [phase17-gate] category: $name ====="
  local rc=0
  local args
  for args in "$@"; do
    if ! run_cargo "$args"; then
      rc=1
    fi
  done
  if [ "$rc" -eq 0 ]; then
    echo "[phase17-gate] $name: PASS"
  else
    echo "[phase17-gate] $name: FAIL"
  fi
  return $rc
}

print_summary() {
  echo ""
  echo "===== [phase17-gate] summary ====="
  if [ "$1" -eq 0 ]; then
    echo "[phase17-gate] ALL CATEGORIES PASS"
  else
    echo "[phase17-gate] ONE OR MORE CATEGORIES FAILED"
  fi
  exit "$1"
}

run_cleanup_selftest() {
  echo ""
  echo "===== [phase17-gate] category: cleanup-selftest ====="
  local selftest_target="$ROOT/target/gates-cleanup-selftest"
  rm -rf "$selftest_target"
  PHASE17_GATE_TARGET_DIR="$selftest_target" \
  PHASE17_GATE_FORCE_FAIL=1 \
  PHASE17_GATE_CLEANUP_TEST=0 \
  PHASE17_GATE_KEEP_TARGET=0 \
    bash "$0"
  local inner_rc=$?
  if [ "$inner_rc" -ne 0 ] && [ ! -e "$selftest_target" ]; then
    echo "[phase17-gate] cleanup-selftest: PASS (inner rc=$inner_rc, isolated dir removed)"
    return 0
  fi
  local still="no"
  [ -e "$selftest_target" ] && still="yes"
  echo "[phase17-gate] cleanup-selftest: FAIL (inner rc=$inner_rc, dir still exists=$still)"
  return 1
}

# ---- 入口 A：强制失败类别（cleanup-selftest 内部递归用）。----
if [ "${PHASE17_GATE_FORCE_FAIL:-0}" = "1" ]; then
  OVERALL=0
  run_category "forced-failure" \
    "this-subcommand-does-not-exist --phase17-cleanup-selftest" || OVERALL=1
  print_summary "$OVERALL"
fi

# ---- 入口 B：仅运行失败清理自检（PHASE17_GATE_CLEANUP_TEST=1）。----
if [ "${PHASE17_GATE_CLEANUP_TEST:-0}" = "1" ]; then
  OVERALL=0
  run_cleanup_selftest || OVERALL=1
  print_summary "$OVERALL"
fi

OVERALL=0

# 类别 1：host 集群测试（remote-control-adapter 含 tests/transport_remote_carrier.rs 承载集成测试）。
run_category "cluster-test" \
  "test $CLUSTER_P_FLAGS" || OVERALL=1

# 类别 2：host 集群 clippy（全部 target，warnings 视为失败）。
run_category "cluster-clippy" \
  "clippy $CLUSTER_P_FLAGS --all-targets -- -D warnings" || OVERALL=1

# 类别 3：边界检查（纯静态，不编译）。
# 3a：每个集群 crate 的存在性、workspace 登记与依赖方向；
# 3b：remote-control-adapter 保留的定向边界检查。
echo ""
echo "===== [phase17-gate] category: boundary-check ====="
BOUNDARY_RC=0
META_JSON="$(cargo metadata --format-version 1 --no-deps 2>/dev/null || true)"
for crate in $CLUSTER_CRATES; do
  MANIFEST="$ROOT/crates/$crate/Cargo.toml"
  if [ ! -f "$MANIFEST" ]; then
    echo "[phase17-gate] boundary: FAIL - missing manifest: $MANIFEST"
    BOUNDARY_RC=1
    continue
  fi
  if printf '%s\n' "$META_JSON" | grep -q "\"name\":\"$crate\""; then
    echo "[phase17-gate] boundary: $crate workspace member registered"
  else
    echo "[phase17-gate] boundary: FAIL - $crate not registered in workspace members"
    BOUNDARY_RC=1
  fi
  DEPS_SEC="$(sed -n '/^\[dependencies\]/,/^\[/p' "$MANIFEST")"
  for forbidden in gui-protocol gui-server gui-client agent-engine; do
    if printf '%s\n' "$DEPS_SEC" | grep -Eq "^${forbidden}([[:space:]]|=|\.)"; then
      echo "[phase17-gate] boundary: FAIL - $crate forbidden direct dependency in [dependencies]: $forbidden"
      BOUNDARY_RC=1
    fi
  done
done

MANIFEST="$ROOT/crates/remote-control-adapter/Cargo.toml"
if [ ! -f "$MANIFEST" ]; then
  echo "[phase17-gate] boundary: FAIL - missing manifest: $MANIFEST"
  BOUNDARY_RC=1
else
  if cargo metadata --format-version 1 --no-deps 2>/dev/null | grep -q '"name":"remote-control-adapter"'; then
    echo "[phase17-gate] boundary: workspace member registered"
  else
    echo "[phase17-gate] boundary: FAIL - remote-control-adapter not registered in workspace members"
    BOUNDARY_RC=1
  fi

  DEPS_SEC="$(sed -n '/^\[dependencies\]/,/^\[/p' "$MANIFEST")"
  DEV_SEC="$(sed -n '/^\[dev-dependencies\]/,/^\[/p' "$MANIFEST")"

  for forbidden in gui-protocol gui-server gui-client agent-engine session-store app-database transport-remote; do
    if printf '%s\n' "$DEPS_SEC" | grep -Eq "^${forbidden}([[:space:]]|=|\.)"; then
      echo "[phase17-gate] boundary: FAIL - forbidden direct dependency in [dependencies]: $forbidden"
      BOUNDARY_RC=1
    fi
  done
  if printf '%s\n' "$DEPS_SEC" | grep -Eq '^provider-[A-Za-z0-9_-]+([[:space:]]|=|\.)'; then
    echo "[phase17-gate] boundary: FAIL - provider-* direct dependency in [dependencies]"
    BOUNDARY_RC=1
  fi
  for required in agent-domain app-service core-api subscription-hub transport-api; do
    if ! printf '%s\n' "$DEPS_SEC" | grep -Eq "^${required}([[:space:]]|=|\.)"; then
      echo "[phase17-gate] boundary: FAIL - required dependency missing: $required"
      BOUNDARY_RC=1
    fi
  done
  if printf '%s\n' "$DEV_SEC" | grep -Eq '^transport-remote([[:space:]]|=|\.)'; then
    echo "[phase17-gate] boundary: transport-remote present as dev-only carrier dependency"
  else
    echo "[phase17-gate] boundary: FAIL - transport-remote missing from [dev-dependencies] (carrier integration evidence)"
    BOUNDARY_RC=1
  fi
fi
if [ "$BOUNDARY_RC" -eq 0 ]; then
  echo "[phase17-gate] boundary-check: PASS"
else
  echo "[phase17-gate] boundary-check: FAIL"
  OVERALL=1
fi

# 类别 4：失败清理自检（trap EXIT 清理路径）。
run_cleanup_selftest || OVERALL=1

print_summary "$OVERALL"
