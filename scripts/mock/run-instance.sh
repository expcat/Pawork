#!/usr/bin/env bash
# MOCK-6：stdlib 编排。用法见 run_instance.py --help。
set -euo pipefail
exec python3 "$(dirname "$0")/run_instance.py" "$@"
