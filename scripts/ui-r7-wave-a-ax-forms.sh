#!/usr/bin/env bash
# R7 Wave A macOS AX 启动形态对照：同一 Desktop build 分别以裸二进制和
# 最小、ad-hoc 签名的 .app/Contents/MacOS/pawork-desktop 首启。
#
# 每种形态只启动一次，绝不调用 desktop-restart。等 timeline_stable 只为
# 避免把“窗口尚未渲染”混入 AX 注册分类；随后只做一次 AX probe，并把该
# 首次分类原样归档。递归 AXApplication、仅系统 chrome、缺 Pawork 稳定
# identifiers 均 fail-closed。
#
# Usage:
#   scripts/ui-r7-wave-a-ax-forms.sh run --out <new-or-empty-dir>
#     [--label <name>]
# Exit: 0 两种形态均取得 Pawork AX tree；2 usage；3 infrastructure；4 AX gate。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
FIXTURE="$SCRIPT_DIR/ui-fixture.sh"
AXDUMP_SRC="$SCRIPT_DIR/ui-ax-dump.swift"
WINDOW_TIMEOUT_SECS="${PAWORK_UI_WINDOW_TIMEOUT_SECS:-120}"
PROBE_TIMEOUT_SECS="${PAWORK_UI_PHASE_TIMEOUT_SECS:-30}"

die() { echo "ui-r7-ax-forms: $*" >&2; exit 3; }
usage() {
  echo "usage: scripts/ui-r7-wave-a-ax-forms.sh run --out <dir> [--label <name>]" >&2
  exit 2
}

MODE=""
OUT=""
LABEL="r7-wave-a-ax-forms"
while (( $# )); do
  case "$1" in
    run) MODE=run; shift ;;
    --out) (( $# >= 2 )) || usage; OUT="$2"; shift 2 ;;
    --label) (( $# >= 2 )) || usage; LABEL="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done
[[ "$MODE" == run && -n "$OUT" ]] || usage
[[ -x "$FIXTURE" ]] || die "missing executable $FIXTURE"
[[ -f "$AXDUMP_SRC" ]] || die "missing $AXDUMP_SRC"
for tool in cargo swiftc codesign plutil shasum sw_vers python3; do
  command -v "$tool" >/dev/null 2>&1 || die "required tool not found: $tool"
done
if [[ -e "$OUT" && ! -d "$OUT" ]]; then
  die "--out must name a new or empty directory: $OUT"
fi
if [[ -d "$OUT" ]]; then
  shopt -s nullglob dotglob
  entries=("$OUT"/*)
  shopt -u nullglob dotglob
  (( ${#entries[@]} == 0 )) \
    || die "--out must be new or empty to prevent stale evidence: $OUT"
fi

WORK="$(mktemp -d /tmp/pawork-r7-ax-forms.XXXXXX)"
ROOTS=()
cleanup() {
  local status=$? root
  trap - EXIT
  for root in "${ROOTS[@]}"; do
    if [[ -f "$root/.pawork-ui-fixture" ]]; then
      "$FIXTURE" down --root "$root" >/dev/null 2>&1 || true
      "$FIXTURE" clean --root "$root" >/dev/null 2>&1 || true
    fi
  done
  python3 - "$WORK" <<'PY'
import shutil
import sys

shutil.rmtree(sys.argv[1], ignore_errors=True)
PY
  exit "$status"
}
trap cleanup EXIT

# 在证据目录出现文件前冻结源码状态；脚本自身与调用前改动会如实进入快照。
(cd "$REPO_ROOT" && git rev-parse HEAD) > "$WORK/git-head.txt"
(cd "$REPO_ROOT" && git status --short) > "$WORK/git-status.txt"
sw_vers > "$WORK/sw-vers.txt"

mkdir -p "$OUT/raw/logs" "$OUT/raw/barriers" \
  "$OUT/bundled-signed/logs" "$OUT/bundled-signed/barriers"
OUT="$(cd "$OUT" && pwd -P)"
cp "$WORK/git-status.txt" "$OUT/git-status.txt"
cp "$WORK/sw-vers.txt" "$OUT/sw-vers.txt"
TRACE="$OUT/action-trace.txt"
: > "$TRACE"
trace() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" | tee -a "$TRACE" >&2; }

if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  if [[ "$CARGO_TARGET_DIR" == /* ]]; then
    TARGET_DIR="$CARGO_TARGET_DIR"
  else
    TARGET_DIR="$REPO_ROOT/$CARGO_TARGET_DIR"
  fi
else
  TARGET_DIR="$REPO_ROOT/target"
fi
BUILD_BIN="$TARGET_DIR/debug/pawork-desktop"
RAW_BIN="$WORK/raw/pawork-desktop"

trace "build Desktop exactly once"
(cd "$REPO_ROOT" && cargo build -p pawork-desktop --offline \
  --features gpui/runtime_shaders --bin pawork-desktop)
[[ -x "$BUILD_BIN" ]] || die "Desktop build output missing: $BUILD_BIN"
mkdir -p "$(dirname "$RAW_BIN")"
cp -p "$BUILD_BIN" "$RAW_BIN"
RAW_SHA="$(shasum -a 256 "$RAW_BIN" | awk '{print $1}')"
BUILD_SHA="$(shasum -a 256 "$BUILD_BIN" | awk '{print $1}')"
[[ "$RAW_SHA" == "$BUILD_SHA" ]] || die "raw launch copy differs from Desktop build output"

APP="$WORK/Pawork AX Forms.app"
APP_EXEC="$APP/Contents/MacOS/pawork-desktop"
mkdir -p "$APP/Contents/MacOS"
cp -p "$RAW_BIN" "$APP_EXEC"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>pawork-desktop</string>
  <key>CFBundleIdentifier</key><string>dev.pawork.desktop.ax-forms</string>
  <key>CFBundleName</key><string>Pawork</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleShortVersionString</key><string>0.0.0-r7</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" > "$OUT/bundled-signed/plutil.txt"
APP_PRE_SIGN_SHA="$(shasum -a 256 "$APP_EXEC" | awk '{print $1}')"
[[ "$APP_PRE_SIGN_SHA" == "$RAW_SHA" ]] \
  || die "bundle executable differs from the one Desktop build before signing"
codesign --force --sign - "$APP" > "$OUT/bundled-signed/codesign-sign.txt" 2>&1 \
  || die "ad-hoc codesign failed (see bundled-signed/codesign-sign.txt)"
codesign --verify --deep --strict --verbose=4 "$APP" \
  > "$OUT/bundled-signed/codesign-verify.txt" 2>&1 \
  || die "signed app verification failed (see bundled-signed/codesign-verify.txt)"
APP_SHA="$(shasum -a 256 "$APP_EXEC" | awk '{print $1}')"
cp "$APP/Contents/Info.plist" "$OUT/bundled-signed/Info.plist"

set +e
codesign --display --verbose=4 "$RAW_BIN" > "$OUT/raw/codesign.txt" 2>&1
RAW_CODESIGN_RC=$?
codesign --display --verbose=4 "$APP" > "$OUT/bundled-signed/codesign.txt" 2>&1
APP_CODESIGN_RC=$?
set -e
printf '\nexit_code=%s\n' "$RAW_CODESIGN_RC" >> "$OUT/raw/codesign.txt"
printf '\nexit_code=%s\n' "$APP_CODESIGN_RC" >> "$OUT/bundled-signed/codesign.txt"
{
  printf '%s  build-output:%s\n' "$BUILD_SHA" "$BUILD_BIN"
  printf '%s  raw-launch-copy:%s\n' "$RAW_SHA" "$RAW_BIN"
} > "$OUT/raw/sha256.txt"
{
  printf '%s  source-before-sign:%s\n' "$APP_PRE_SIGN_SHA" "$BUILD_BIN"
  printf '%s  launch-after-sign:%s\n' "$APP_SHA" "$APP_EXEC"
} > "$OUT/bundled-signed/sha256.txt"

trace "compile AX probe helper"
swiftc -O -o "$WORK/ui-ax-dump" "$AXDUMP_SRC" \
  2> "$WORK/swiftc-axdump.err" \
  || { cp "$WORK/swiftc-axdump.err" "$OUT/swiftc-axdump.err"; die "ui-ax-dump compile failed"; }
AXDUMP="$WORK/ui-ax-dump"

run_bounded() {
  local timeout="$1"; shift
  python3 - "$timeout" "$@" <<'PY'
import os
import signal
import subprocess
import sys

proc = subprocess.Popen(sys.argv[2:], start_new_session=True)
try:
    raise SystemExit(proc.wait(timeout=float(sys.argv[1])))
except subprocess.TimeoutExpired:
    os.killpg(proc.pid, signal.SIGKILL)
    proc.wait()
    raise SystemExit(124)
PY
}

classify_probe() { # $1=probe $2=probe_rc $3=classification.json
  python3 - "$1" "$2" "$3" <<'PY'
import json
import re
import sys
from pathlib import Path

probe_path, rc_raw, out_path = sys.argv[1:]
text = Path(probe_path).read_text(encoding="utf-8", errors="replace") if Path(probe_path).exists() else ""
rc = int(rc_raw)
required = ["pawork-root", "task-rail", "session-list", "workspace", "status-bar"]
present = sorted(set(re.findall(r'identifier="([^"]+)"', text)))
missing = [value for value in required if value not in present]
app_count = len(re.findall(r"(?:^|\s)role=AXApplication(?:\s|$)", text, re.MULTILINE))
recursive = "# WARN ax-fallback=axwindows" in text or (
    app_count >= 3 and "# identifiers (none)" in text
)
if rc != 0:
    classification = "probe-error"
elif recursive:
    classification = "recursive-axapplication"
elif not present and "role=AXWindow" in text:
    classification = "system-chrome-only"
elif missing:
    classification = "missing-pawork-identifiers"
else:
    classification = "pawork-identifiers"
payload = {
    "classification": classification,
    "pass": classification == "pawork-identifiers",
    "probe_exit_code": rc,
    "ax_application_nodes": app_count,
    "required_identifiers": required,
    "missing_identifiers": missing,
    "identifier_count": len(present),
    "used_axwindows_fallback": "# WARN ax-fallback=axwindows" in text,
}
Path(out_path).write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
PY
}

FORM_FAILURE=0
run_form() { # $1=raw|bundled-signed $2=launch executable
  local form="$1" launch="$2" root pid deadline probe_rc ready=0 file
  root="$(mktemp -d /tmp/pawork-ui-fixture.XXXXXX)"
  root="$(cd "$root" && pwd -P)"
  ROOTS+=("$root")
  printf '%s\n' "$launch" > "$OUT/$form/launch-path.txt"
  trace "$form: seed and serve isolated fixture"
  "$FIXTURE" seed --root "$root"
  "$FIXTURE" serve --root "$root"
  trace "$form: first and only Desktop launch (no automatic restart)"
  PAWORK_UI_DESKTOP_BIN="$launch" "$FIXTURE" desktop --root "$root"
  pid="$(cat "$root/desktop.pid")"
  printf '%s\n' "$pid" > "$OUT/$form/desktop-pid.txt"

  deadline=$(( SECONDS + WINDOW_TIMEOUT_SECS ))
  while (( SECONDS < deadline )); do
    if [[ -f "$root/barriers/timeline_stable" ]]; then ready=1; break; fi
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.2
  done
  printf 'timeline_stable=%s\n' "$ready" > "$OUT/$form/readiness.txt"

  set +e
  run_bounded "$PROBE_TIMEOUT_SECS" "$AXDUMP" --pid "$pid" --max-depth 16 \
    --out "$OUT/$form/ax-probe.txt" >/dev/null 2>&1
  probe_rc=$?
  set -e
  if [[ ! -f "$OUT/$form/ax-probe.txt" ]]; then
    printf '# ui-ax-dump did not produce output\n# process_alive=%s\n' \
      "$(kill -0 "$pid" 2>/dev/null && echo true || echo false)" \
      > "$OUT/$form/ax-probe.txt"
  fi
  classify_probe "$OUT/$form/ax-probe.txt" "$probe_rc" \
    "$OUT/$form/first-classification.json"
  if ! python3 - "$OUT/$form/first-classification.json" <<'PY'
import json
import sys

raise SystemExit(0 if json.load(open(sys.argv[1], encoding="utf-8"))["pass"] else 1)
PY
  then
    FORM_FAILURE=1
  fi

  cp "$root/logs/serve.log" "$OUT/$form/logs/serve.log" 2>/dev/null || true
  cp "$root/logs/desktop.log" "$OUT/$form/logs/desktop.log" 2>/dev/null || true
  for file in "$root"/barriers/*; do
    [[ -f "$file" ]] && cp "$file" "$OUT/$form/barriers/" 2>/dev/null || true
  done
  "$FIXTURE" down --root "$root" >/dev/null
  "$FIXTURE" clean --root "$root" >/dev/null
  trace "$form: first classification archived"
}

run_form raw "$RAW_BIN"
run_form bundled-signed "$APP_EXEC"

python3 - "$OUT" "$LABEL" "$BUILD_BIN" "$RAW_BIN" "$RAW_SHA" "$APP_EXEC" \
  "$APP_PRE_SIGN_SHA" "$APP_SHA" "$RAW_CODESIGN_RC" "$APP_CODESIGN_RC" \
  "$WORK/git-head.txt" "$WORK/git-status.txt" "$WORK/sw-vers.txt" <<'PY'
import datetime
import json
import sys
from pathlib import Path

(out_raw, label, build_path, raw_path, raw_sha, app_path, app_pre_sha, app_sha,
 raw_codesign_rc, app_codesign_rc, head_path, status_path, sw_path) = sys.argv[1:]
out = Path(out_raw)
forms = {}
for name in ("raw", "bundled-signed"):
    forms[name] = {
        "launch_path": (out / name / "launch-path.txt").read_text(encoding="utf-8").strip(),
        "first_classification": json.loads(
            (out / name / "first-classification.json").read_text(encoding="utf-8")
        ),
        "evidence": {
            "ax_probe": f"{name}/ax-probe.txt",
            "classification": f"{name}/first-classification.json",
            "codesign": f"{name}/codesign.txt",
            "sha256": f"{name}/sha256.txt",
            "desktop_log": f"{name}/logs/desktop.log",
        },
    }
payload = {
    "scenario": "r7-wave-a-ax-launch-forms",
    "label": label,
    "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z"),
    "git": {
        "head": Path(head_path).read_text(encoding="utf-8").strip(),
        "status": Path(status_path).read_text(encoding="utf-8").splitlines(),
    },
    "host": {"sw_vers": Path(sw_path).read_text(encoding="utf-8").splitlines()},
    "single_desktop_build": {
        "build_output_path": build_path,
        "raw_launch_path": raw_path,
        "raw_sha256": raw_sha,
        "bundle_launch_path": app_path,
        "bundle_sha256_before_sign": app_pre_sha,
        "bundle_sha256_after_sign": app_sha,
        "same_payload_before_sign": raw_sha == app_pre_sha,
    },
    "codesign_display_exit_codes": {
        "raw": int(raw_codesign_rc),
        "bundled_signed": int(app_codesign_rc),
    },
    "forms": forms,
    "pass": all(item["first_classification"]["pass"] for item in forms.values()),
    "policy": "fail-closed on recursive AXApplication, system chrome only, or missing Pawork identifiers",
}
(out / "run-manifest.json").write_text(
    json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
)
PY

if (( FORM_FAILURE )); then
  echo "ui-r7-ax-forms: AX gate failed; see $OUT/run-manifest.json" >&2
  exit 4
fi
trace "PASS: raw and bundled-signed first-launch AX trees contain Pawork identifiers"
