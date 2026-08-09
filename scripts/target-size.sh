#!/usr/bin/env bash
# scripts/target-size.sh
# 用途：诊断 target/ 体积分布（只读脚本，不执行任何清理）。
# 只读取目录大小信息（du），不删除、不移动、不修改任何文件。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET_DIR="${SCRIPT_DIR}/../target"

if [ ! -d "${TARGET_DIR}" ]; then
    echo "[target-size] target/ 不存在，跳过体积诊断（只读脚本，不做任何清理）。" >&2
    exit 0
fi

echo "[target-size] 各目录体积 (MB)："
for d in \
    "${TARGET_DIR}/debug" \
    "${TARGET_DIR}/release" \
    "${TARGET_DIR}/gates" \
    "${TARGET_DIR}/debug/deps" \
    "${TARGET_DIR}/debug/incremental" \
    "${TARGET_DIR}/debug/build"; do
    if [ -d "${d}" ]; then
        du -sm "${d}" 2>/dev/null || true
    fi
done | sort -rn | head -10

echo "[target-size] target/ 总大小 (MB)："
du -sm "${TARGET_DIR}"
