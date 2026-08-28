#!/usr/bin/env python3
"""R1 Wave D helpers: AX tree/frame parsing, barrier reads, normalize, evidence."""

from __future__ import annotations

import argparse
import io
import json
import re
import subprocess
import sys
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

from PIL import Image, ImageCms

IDENTIFIER_RE = re.compile(r'identifier="([^"]*)"')
VALUE_RE = re.compile(r'value="([^"]*)"')
DESCRIPTION_RE = re.compile(r'description="([^"]*)"')
HELP_RE = re.compile(r'help="([^"]*)"')
ENABLED_RE = re.compile(r'enabled=([01])')

SESSION_ROW = "session-fx-ses-alpha-today"
TIMELINE_PREFIX = "timeline-entry-evt-fx-ses-alpha-today"
SEED_IDENTIFIER_PREFIX = "timeline-entry-evt-fx-"
NL = chr(10)
RESAMPLE = getattr(Image, "Resampling", Image).LANCZOS

# Wave B/C 相位合同：root 尺寸 / rail 宽度 / Inspector 列是否必须在场。
# narrow：窄窗（1080 宽）rail=240 且 Inspector 折叠缺席；
# collapsed：1440 宽下 Inspector 列折叠缺席（State B，Popover 由骨架断言覆盖）。
# disconnected/connect-failed/reconnected（Wave C）：1440 三栏壳层不因断连/重连变化。
PHASE_GEOMETRY = {
    "initial": {"root": (1440.0, 1024.0), "rail": (288.0, 4.32), "inspector": "required"},
    "empty": {"root": (1440.0, 1024.0), "rail": (288.0, 4.32), "inspector": "required"},
    "final": {"root": (1440.0, 1024.0), "rail": (288.0, 4.32), "inspector": "required"},
    "restored": {"root": (1440.0, 1024.0), "rail": (288.0, 4.32), "inspector": "required"},
    "resumed": {"root": (1440.0, 1024.0), "rail": (288.0, 4.32), "inspector": "required"},
    "narrow": {"root": (1080.0, 1024.0), "rail": (240.0, 3.6), "inspector": "absent"},
    "collapsed": {"root": (1440.0, 1024.0), "rail": (288.0, 4.32), "inspector": "absent"},
    "disconnected": {"root": (1440.0, 1024.0), "rail": (288.0, 4.32), "inspector": "required"},
    "connect-failed": {"root": (1440.0, 1024.0), "rail": (288.0, 4.32), "inspector": "required"},
    "reconnected": {"root": (1440.0, 1024.0), "rail": (288.0, 4.32), "inspector": "required"},
    # R3 Wave A State C：Projects 分组态。三栏壳层与 rail 宽度不随分组模式
    # 变化；分组语义差异由 skeleton 检查（grouping 值 / 项目块 / 日期桶）承载。
    "projects": {"root": (1440.0, 1024.0), "rail": (288.0, 4.32), "inspector": "required"},
}

SKELETON_BASE = [
    "task-rail",
    "session-list",
    SESSION_ROW,
    "workspace",
    "timeline",
    "composer",
    "composer-input",
    "status-bar",
]

# R4 Wave B states（S1-S9）增量断言的常量：审批卡 / 终态摘要 / 工具行 /
# 虚拟化逻辑行数。文案与 identifier 均核实自 apps/desktop AX 层与
# fixtures/ui/seed.json；Failed 摘要原因为 WS-1 落地的种子原因原文。
APPROVAL_CARD = "approval-card"
APPROVAL_BUTTONS = ("approve-once", "approve-for-run", "approve-deny")
APPROVAL_SESSION_ID = "fx-ses-beta-pending"
APPROVAL_SESSION_ROW = "session-" + APPROVAL_SESSION_ID
FAILED_REASON = "fixture scripted provider failure"
TOOL_FAILED_DETAIL = "fixture tool failure"
# live RunChanged{Failed} 不带 ErrorContext（wire 契约），摘要卡只能诚实
# 兜底；真实原因经快照重放（project_event 读 domain RunFailed）才可见。
LIVE_FAILED_FALLBACK = "The run failed."
# fx-ses-beta-long 每轮 4 条目（RunStarted + user + assistant + RunCompleted）
# × 16 轮 = 64；reducer 条目数即逻辑行数，与是否被 RunSummary 吸收无关。
BETA_LONG_LOGICAL_ROWS = 64
STATES_PHASES = (
    "approval-visible",
    "approval-resolved",
    "approval-replayed",
    "failed-summary",
    "cancelled-summary",
    "tool-failed",
    "virtualized",
    "streamed-summary",
    "live-failed",
    "failed-replayed",
    "hang-cancelable",
    "hang-cancelled",
    "disconnected-retained",
    "reconnected-replay",
)


def now_iso():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_frames(path):
    frames = {}
    for line in Path(path).read_text("utf-8").splitlines():
        if not line.startswith("id="):
            continue
        fields = {}
        for part in line.split():
            if "=" in part:
                key, value = part.split("=", 1)
                fields[key] = value
        if "id" not in fields:
            continue
        try:
            frames[fields["id"]] = {
                "role": fields.get("role", "?"),
                "x": float(fields["x"]),
                "y": float(fields["y"]),
                "w": float(fields["w"]),
                "h": float(fields["h"]),
            }
        except (KeyError, ValueError):
            frames[fields["id"]] = {"role": fields.get("role", "?"), "error": True}
    return frames


def parse_tree(path):
    text = Path(path).read_text("utf-8")
    lines = text.splitlines()
    tree_lines = [line for line in lines if not line.lstrip().startswith("#")]
    identifiers = set()
    for line in tree_lines:
        identifiers.update(IDENTIFIER_RE.findall(line))
    role_unknown = sum(1 for line in tree_lines if "role=?" in line)
    focused = []
    selected_rows = []
    timeline_entries = 0
    for line in tree_lines:
        ids = IDENTIFIER_RE.findall(line)
        if not ids:
            continue
        identifier = ids[0]
        if "focused=1" in line:
            focused.append(identifier)
        if "selected=1" in line and identifier.startswith("session-"):
            selected_rows.append(identifier)
        if identifier.startswith(TIMELINE_PREFIX + "-"):
            timeline_entries += 1
    summary = ""
    for line in lines:
        if line.startswith("# summary"):
            summary = line[2:].strip()
            break
    connection_status = None
    for line in tree_lines:
        ids = IDENTIFIER_RE.findall(line)
        if ids and ids[0] == "connection-status":
            values = VALUE_RE.findall(line)
            connection_status = values[0] if values else ""
            break
    grouping_value = None
    for line in tree_lines:
        ids = IDENTIFIER_RE.findall(line)
        if ids and ids[0] == "task-rail-grouping":
            values = VALUE_RE.findall(line)
            grouping_value = values[0] if values else ""
            break
    return {
        "identifiers": identifiers,
        "role_unknown": role_unknown,
        "focused": focused,
        "selected_rows": selected_rows,
        "timeline_entries_alpha_today": timeline_entries,
        "connection_status": connection_status,
        "grouping_value": grouping_value,
        "summary": summary,
    }


def cmd_barrier_read(args):
    try:
        payload = json.loads(Path(args.file).read_text("utf-8"))
    except (OSError, ValueError):
        return 1
    if args.field == "seq":
        print(payload.get("settle_seq", ""))
    elif args.field == "session":
        print(payload.get("session_id", ""))
    elif args.field == "entries":
        print(payload.get("entry_count", ""))
    else:
        print(
            str(payload.get("settle_seq", ""))
            + " "
            + str(payload.get("session_id", ""))
            + " "
            + str(payload.get("entry_count", ""))
        )
    return 0


def near(value, expected, tol):
    return abs(value - expected) <= tol + 1e-9


def geometry_checks(frames, phase="initial"):
    checks = []
    metrics = {}
    if phase not in PHASE_GEOMETRY:
        raise ValueError("unknown phase: " + str(phase))
    spec = PHASE_GEOMETRY[phase]
    root_w, root_h = spec["root"]
    rail_target, rail_tol = spec["rail"]
    inspector_required = spec["inspector"] == "required"

    def add(name, ok, detail, *, blocking=True):
        checks.append({
            "name": name,
            "pass": bool(ok),
            "blocking": bool(blocking),
            "detail": detail,
        })

    root = frames.get("pawork-root")
    if root is None or root.get("error"):
        add("root-frame", False, "pawork-root frame missing")
        return checks, metrics
    metrics["root"] = root
    root_name = "root-" + str(int(root_w)) + "x" + str(int(root_h))
    add(
        root_name,
        near(root["w"], root_w, 0.5) and near(root["h"], root_h, 0.5),
        "root w=" + str(root["w"]) + " h=" + str(root["h"])
        + " (contract " + str(int(root_w)) + "x" + str(int(root_h)) + " +/-0.5)",
    )
    rail = frames.get("task-rail")
    rail_ok = rail is not None and not rail.get("error") and near(rail["w"], rail_target, rail_tol)
    add(
        "rail-width",
        rail_ok,
        "task-rail w=" + (str(rail["w"]) if rail else "missing")
        + " (contract " + str(int(rail_target)) + " +/-" + str(rail_tol) + ")",
    )
    insp = frames.get("inspector")
    if not inspector_required:
        add(
            "inspector-absent",
            insp is None,
            "inspector frame absent (phase=" + phase + ")"
            if insp is None
            else "inspector frame present w=" + str(insp.get("w")) + " (phase=" + phase + ")",
        )
    elif insp and not insp.get("error"):
        metrics["inspector"] = insp
        rel_x = insp["x"] - root["x"]
        add(
            "inspector-width",
            near(insp["w"], 440.0, 6.6),
            "inspector w=" + str(insp["w"]) + " (contract 440 +/-6.6)",
        )
        add(
            "inspector-x",
            near(rel_x, 1000.0, 6.6),
            "inspector rel_x=" + str(round(rel_x, 2)) + " (expected 1000)",
        )
    else:
        add("inspector-width", False, "inspector frame missing")
    status = frames.get("status-bar")
    if status and not status.get("error"):
        metrics["statusbar"] = status
        add(
            "statusbar-height",
            near(status["h"], 24.0, 0.5),
            "status-bar h=" + str(status["h"]) + " (contract 24 +/-0.5)",
        )
    else:
        add("statusbar-height", False, "status-bar frame missing")
    composer = frames.get("composer")
    if composer and not composer.get("error"):
        metrics["composer"] = composer
        composer_contract_ok = 86.0 <= composer["h"] <= 96.0
        known_r1_height = near(composer["h"], 156.0, 1.0)
        add(
            "composer-height",
            composer_contract_ok,
            "composer h=" + str(composer["h"])
            + " (contract 88-94, component tolerance [86,96]; "
            + (
                "known 156+/-1 F-09 visual drift, recorded but nonblocking for the R1 baseline)"
                if known_r1_height
                else "not the frozen R1 baseline; blocking drift)"
            ),
            blocking=not known_r1_height,
        )
        if status and not status.get("error"):
            composer_bottom = composer["y"] + composer["h"] - root["y"]
            status_top = status["y"] - root["y"]
            add(
                "composer-above-statusbar",
                abs(composer_bottom - status_top) <= 1.0,
                "composer bottom=" + str(round(composer_bottom, 2))
                + " statusbar top=" + str(round(status_top, 2)),
            )
    else:
        add("composer-height", False, "composer frame missing")
    workspace = frames.get("workspace")
    inspector_span_ok = (
        inspector_required
        and insp is not None
        and workspace is not None
        and rail is not None
        and not any(v.get("error") for v in (workspace, rail, insp))
    )
    rail_span_ok = (
        not inspector_required
        and workspace is not None
        and rail is not None
        and not any(v.get("error") for v in (workspace, rail))
    )
    if inspector_span_ok:
        span = workspace["x"] + workspace["w"] - root["x"]
        add(
            "workspace-span",
            near(workspace["x"] - root["x"], rail["w"], 1.0)
            and near(span, root["w"] - insp["w"], 1.0),
            "workspace x=" + str(round(workspace["x"] - root["x"], 2))
            + ".." + str(round(span, 2))
            + " expected " + str(rail["w"]) + ".." + str(root["w"] - insp["w"]),
        )
    elif rail_span_ok:
        span = workspace["x"] + workspace["w"] - root["x"]
        add(
            "workspace-span",
            near(workspace["x"] - root["x"], rail["w"], 1.0)
            and near(span, root["w"], 1.0),
            "workspace x=" + str(round(workspace["x"] - root["x"], 2))
            + ".." + str(round(span, 2))
            + " expected " + str(rail["w"]) + ".." + str(root["w"])
            + " (inspector column absent)",
        )
    return checks, metrics


def skeleton_checks(tree, phase="initial"):
    if phase not in PHASE_GEOMETRY:
        raise ValueError("unknown phase: " + str(phase))
    required = list(SKELETON_BASE)
    if PHASE_GEOMETRY[phase]["inspector"] == "required":
        required += ["inspector", "inspector-tabs"]
    missing = [name for name in required if name not in tree["identifiers"]]
    checks = [
        {
            "name": "shell-skeleton",
            "pass": not missing,
            "detail": ("missing: " + ",".join(missing)) if missing
                else "shell skeleton present (phase=" + phase + ")",
        },
        {
            "name": "no-unknown-role",
            "pass": tree["role_unknown"] == 0,
            "detail": "role=? count=" + str(tree["role_unknown"]),
        },
    ]
    identifiers = tree["identifiers"]
    if phase == "empty":
        checks.append({
            "name": "workspace-empty-hint-present",
            "pass": "workspace-empty-hint" in identifiers,
            "detail": "workspace-empty-hint "
                + ("present" if "workspace-empty-hint" in identifiers else "missing"),
        })
        # 空态相位 = Connected 无会话：Reconnect 不得发布（AX/视觉同源谓词，
        # 防 app.rs 镜像漂移回 !connected 旧条件）。
        checks.append({
            "name": "reconnect-absent",
            "pass": "reconnect" not in identifiers,
            "detail": "reconnect "
                + ("stray present" if "reconnect" in identifiers else "absent"),
        })
    if phase in ("collapsed", "resumed", "disconnected", "connect-failed", "reconnected", "projects"):
        # 已选中会话相位：空态引导必须消失（防谓词回归成恒真）。
        # disconnected 保留旧条目（gui-design 空态原则），同样不得出现引导。
        checks.append({
            "name": "workspace-empty-hint-absent",
            "pass": "workspace-empty-hint" not in identifiers,
            "detail": "workspace-empty-hint "
                + ("stray present" if "workspace-empty-hint" in identifiers else "absent"),
        })
    if phase in ("narrow", "collapsed"):
        stray = [
            name for name in ("inspector", "inspector-tabs")
            if name in identifiers
        ]
        checks.append({
            "name": "inspector-column-absent",
            "pass": not stray,
            "detail": ("stray inspector ids: " + ",".join(stray)) if stray
                else "inspector/inspector-tabs absent from AX tree",
        })
        # 折叠态触发器是 F-12 迁移前的临时主路径，两个折叠相位都必须在场。
        checks.append({
            "name": "inspector-toggle-present",
            "pass": "inspector-toggle" in identifiers,
            "detail": "inspector-toggle "
                + ("present" if "inspector-toggle" in identifiers else "missing"),
        })
    if phase == "collapsed":
        checks.append({
            "name": "activity-popover-present",
            "pass": "activity-popover" in identifiers,
            "detail": "activity-popover "
                + ("present" if "activity-popover" in identifiers else "missing"),
        })
    if phase in ("disconnected", "connect-failed"):
        # 两种断连相位：Reconnect 手动入口必须在场。show_reconnect 对
        # Disconnected/ConnectFailed 均发布（AX 与视觉同源谓词）。
        checks.append({
            "name": "reconnect-present",
            "pass": "reconnect" in identifiers,
            "detail": "reconnect "
                + ("present" if "reconnect" in identifiers else "missing"),
        })
        status = tree.get("connection_status")
        if phase == "disconnected":
            # drop-socket / host-stopped：Disconnected · …；拒绝 Connected /
            # Connecting / Connect failed，避免循环 1 在 Failed 瞬态误过。
            ok = bool(status) and status.startswith("Disconnected ·")
            name = "connection-status-disconnected"
        else:
            # host 停机后重试：必须是 Connect failed · …，不能只靠 reconnect。
            ok = bool(status) and status.startswith("Connect failed ·")
            name = "connection-status-connect-failed"
        checks.append({
            "name": name,
            "pass": ok,
            "detail": "connection-status value="
                + (status if status else "absent"),
        })
    if phase == "reconnected":
        # 重连成功相位：Connected 无需重连入口，reconnect 不得残留。
        checks.append({
            "name": "reconnect-absent",
            "pass": "reconnect" not in identifiers,
            "detail": "reconnect "
                + ("stray present" if "reconnect" in identifiers else "absent"),
        })
        # Connecting 瞬态同样满足 reconnect 缺席 + 条目保留（show_reconnect
        # 对 Connecting 为 false）；connection-status 文案区分两者，防
        # 未来调用方跳过 settle barrier 直接断言时把瞬态误判为重连成功。
        # F-03 起可见文案带「Local · 」前缀（spec desktop.md §3.2：
        # Local · Connected[ · {resume 文案}]），Connecting… 不变。
        status = tree.get("connection_status")
        checks.append({
            "name": "connection-status-connected",
            "pass": bool(status) and status.startswith("Local · Connected"),
            "detail": "connection-status value="
                + (status if status else "absent"),
        })
    if phase == "projects":
        # State C 语义：菜单选择已提交（值切到 Projects）且浮层已收起；
        # 日期桶头不得残留，fixture 的两个项目块必须在场（gamma-notes 无
        # 会话、全部会话都有 workspace，故不出现第三组/Unassigned）。
        value = tree.get("grouping_value")
        checks.append({
            "name": "grouping-projects-selected",
            "pass": value == "Projects",
            "detail": "task-rail-grouping value=" + (value if value else "absent"),
        })
        checks.append({
            "name": "grouping-menu-closed",
            "pass": "grouping-menu" not in identifiers,
            "detail": "grouping-menu "
                + ("stray present" if "grouping-menu" in identifiers else "absent"),
        })
        stray_date_groups = sorted(
            name for name in identifiers if name.startswith("date-group-")
        )
        checks.append({
            "name": "date-groups-absent",
            "pass": not stray_date_groups,
            "detail": ("stray date groups: " + ",".join(stray_date_groups))
                if stray_date_groups
                else "date-group-* absent from AX tree",
        })
        project_groups = sorted(
            name for name in identifiers
            if name.startswith("project-")
            and name != "project-scope"
            and not name.startswith("project-add-")
        )
        required_groups = ["project-fx-alpha-app", "project-fx-beta-lib"]
        missing_groups = [
            name for name in required_groups if name not in project_groups
        ]
        checks.append({
            "name": "project-groups-present",
            "pass": not missing_groups,
            "detail": ("missing: " + ",".join(missing_groups)) if missing_groups
                else "projects: " + ",".join(project_groups),
        })
    return checks


def cmd_assert(args):
    frames = parse_frames(args.frames)
    tree = parse_tree(args.tree)
    checks, metrics = geometry_checks(frames, args.phase)
    checks.extend(skeleton_checks(tree, args.phase))
    if args.phase in ("initial", "empty"):
        focused = tree["focused"]
        allowed_focus = {"pawork-root", "composer-input"}
        ok = len(focused) <= 1 and all(node in allowed_focus for node in focused)
        checks.append({
            "name": "focus-start",
            "pass": ok,
            "detail": "initial focus=" + (",".join(focused) if focused else "none")
                + " (allowed: none, pawork-root, or composer-input; exactly one at most)",
        })
        if args.phase == "initial":
            checks.append({
                "name": "initial-selected-observed",
                "pass": True,
                "detail": "startup selected rows=" + (",".join(tree["selected_rows"]) or "none")
                    + " (observation only)",
            })
        else:
            checks.append({
                "name": "timeline-empty",
                "pass": tree["timeline_entries_alpha_today"] == 0,
                "detail": TIMELINE_PREFIX + "-* count="
                + str(tree["timeline_entries_alpha_today"]) + " (empty state)",
            })
    else:
        if args.phase != "narrow" and args.phase != "restored":
            checks.append({
                "name": "session-selected",
                "pass": SESSION_ROW in tree["selected_rows"],
                "detail": "selected=1 rows: " + (",".join(tree["selected_rows"]) or "none"),
            })
            checks.append({
                "name": "timeline-loaded",
                "pass": tree["timeline_entries_alpha_today"] >= 1,
                "detail": TIMELINE_PREFIX + "-* count=" + str(tree["timeline_entries_alpha_today"]),
            })
        if args.phase == "final":
            checks.append({
                "name": "focus-composer-after-select",
                "pass": tree["focused"] == ["composer-input"],
                "detail": "focus after select=" + (",".join(tree["focused"]) or "none"),
            })
    payload = {
        "phase": args.phase,
        "generated_at": now_iso(),
        "checks": checks,
        "metrics": metrics,
        "pass": all(
            check["pass"] or not check.get("blocking", True)
            for check in checks
        ),
    }
    Path(args.out).write_text(json.dumps(payload, indent=2) + NL, "utf-8")
    for check in checks:
        if check["pass"]:
            prefix = "PASS "
        elif check.get("blocking", True):
            prefix = "FAIL "
        else:
            prefix = "OBSERVED-FAIL "
        print(prefix + check["name"] + " - " + check["detail"])
    return 0 if payload["pass"] else 5


def cmd_shell_manifest(args):
    out_dir = Path(args.dir)
    phases = {}
    for path in sorted(out_dir.glob("assert-*.json")):
        name = path.stem[len("assert-"):]
        try:
            payload = json.loads(path.read_text("utf-8"))
            phases[name] = {
                "pass": bool(payload.get("pass")),
                "checks": [
                    {"name": check["name"], "pass": bool(check["pass"])}
                    for check in payload.get("checks", [])
                ],
            }
        except ValueError:
            phases[name] = {"pass": False, "error": "invalid json"}
    evidence = sorted(p.name for p in out_dir.iterdir() if p.is_file())
    git_head = None
    try:
        git_head = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=args.repo,
            capture_output=True, text=True, check=True,
        ).stdout.strip()
    except subprocess.CalledProcessError:
        pass
    structural_pass = bool(phases) and all(entry["pass"] for entry in phases.values())
    manifest = {
        "scenario": args.scenario,
        "label": args.label,
        "generated_at": now_iso(),
        "git_head": git_head,
        "phases": phases,
        "structural_pass": structural_pass,
        "evidence_files": evidence,
    }
    (out_dir / "run-manifest.json").write_text(json.dumps(manifest, indent=2) + NL, "utf-8")
    print("shell manifest written; phases=" + ",".join(phases)
        + "; structural_pass=" + str(structural_pass))
    return 0

def cmd_normalize(args):
    wid = Path(args.wid).read_text("utf-8").strip()
    window_bounds = None
    for line in Path(args.tree).read_text("utf-8").splitlines():
        stripped = line.lstrip("# ").strip()
        if not stripped.startswith("wid="):
            continue
        tokens = stripped.split()
        if tokens[0] != "wid=" + wid:
            continue
        for token in tokens:
            if token.startswith("bounds={") and token.endswith("}"):
                nums = token[len("bounds={"):-1].split(",")
                if len(nums) == 4:
                    window_bounds = [int(n) for n in nums]
        if window_bounds:
            break
    if not window_bounds:
        print("wid " + wid + " bounds not found in ax tree", file=sys.stderr)
        return 3
    win_x, win_y, win_w, win_h = window_bounds
    frames = parse_frames(args.frames)
    root = frames.get("pawork-root")
    if root is None or root.get("error"):
        print("pawork-root frame missing", file=sys.stderr)
        return 3
    # normalize 的合同是 1440×1024 视觉门禁：窄窗（如 1080 相位）截图
    # 送进本命令会被静默放大且 final_size 标注失真，显式拒绝（P3 防误用）。
    if not near(root["w"], 1440.0, 0.5) or not near(root["h"], 1024.0, 0.5):
        print(
            "normalize requires 1440x1024 root frame; got w=" + str(root["w"])
            + " h=" + str(root["h"]),
            file=sys.stderr,
        )
        return 3
    image = Image.open(Path(args.shot))
    source_mode = image.mode
    source_icc = image.info.get("icc_profile")
    source_profile = "none"
    if source_icc:
        try:
            input_profile = ImageCms.ImageCmsProfile(io.BytesIO(source_icc))
            source_profile = ImageCms.getProfileDescription(input_profile).strip()
            srgb_profile = ImageCms.createProfile("sRGB")
            image = ImageCms.profileToProfile(
                image,
                input_profile,
                srgb_profile,
                outputMode="RGB",
            )
            color_conversion = "embedded ICC -> sRGB"
        except Exception as error:
            print("invalid screenshot ICC profile: " + str(error), file=sys.stderr)
            return 3
    else:
        image = image.convert("RGB")
        color_conversion = "untagged RGB assumed sRGB"
    pre_w, pre_h = image.size
    scale_x = pre_w / float(win_w)
    scale_y = pre_h / float(win_h)
    if abs(scale_x - scale_y) > 0.02:
        print("non-uniform scale " + str(scale_x) + " " + str(scale_y), file=sys.stderr)
        return 3
    crop_x = int(round((root["x"] - win_x) * scale_x))
    crop_y = int(round((root["y"] - win_y) * scale_y))
    crop_w = int(round(root["w"] * scale_x))
    crop_h = int(round(root["h"] * scale_y))
    cropped = image.crop((crop_x, crop_y, crop_x + crop_w, crop_y + crop_h)).convert("RGB")
    post_w, post_h = cropped.size
    resized = post_w != 1440 or post_h != 1024
    if resized:
        cropped = cropped.resize((1440, 1024), RESAMPLE)
    # Pillow carries the source ICC metadata through crop/resize. Copy pixels to
    # a fresh RGB image so the visual gate receives explicitly untagged sRGB.
    untagged = Image.new("RGB", cropped.size)
    untagged.paste(cropped)
    untagged.save(Path(args.out), format="PNG")
    mapping = {
        "wid": int(wid),
        "window_bounds_points": window_bounds,
        "root_frame_points": {
            "x": root["x"], "y": root["y"], "w": root["w"], "h": root["h"],
        },
        "capture_pixels": [pre_w, pre_h],
        "source_mode": source_mode,
        "source_icc_profile_bytes": len(source_icc) if source_icc else 0,
        "source_icc_profile_description": source_profile,
        "color_conversion": color_conversion,
        "scale": round(scale_x, 4),
        "crop_pixels": [crop_x, crop_y, crop_w, crop_h],
        "before_resize": [post_w, post_h],
        "resized_lanczos": resized,
        "final_size": [1440, 1024],
        "icc_profile_dropped": bool(source_icc),
        "output_color_interpretation": "untagged RGB; sRGB bytes",
    }
    Path(args.json).write_text(json.dumps(mapping, indent=2) + NL, "utf-8")
    print(json.dumps(mapping))
    return 0


def cmd_manifest(args):
    out_dir = Path(args.dir)
    assert_initial = json.loads((out_dir / "assert-initial.json").read_text("utf-8"))
    assert_final = json.loads((out_dir / "assert-final.json").read_text("utf-8"))
    normalize = json.loads((out_dir / "normalize.json").read_text("utf-8"))
    diff_report = json.loads((out_dir / "diff" / "diff-report.json").read_text("utf-8"))
    seed = json.loads(Path(args.seed).read_text("utf-8"))
    try:
        head = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=args.repo,
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        status = subprocess.run(
            ["git", "status", "--porcelain"], cwd=args.repo,
            capture_output=True, text=True, check=True,
        ).stdout.splitlines()
        theme_diff = subprocess.run(
            ["git", "diff", "--", "apps/desktop/src/ui/theme.rs"],
            cwd=args.repo, capture_output=True, text=True, check=True,
        ).stdout
    except subprocess.CalledProcessError as error:
        print("git evidence failed: " + str(error), file=sys.stderr)
        return 3
    sw = subprocess.run(["sw_vers"], capture_output=True, text=True).stdout.strip().splitlines()
    display = subprocess.run(
        ["system_profiler", "SPDisplaysDataType"], capture_output=True, text=True,
    ).stdout
    display_lines = [
        line.strip() for line in display.splitlines()
        if "Resolution:" in line or "UI Looks like:" in line
    ][:4]
    zones = diff_report["zones"]
    manifest = {
        "scenario": args.scenario,
        "label": args.label,
        "generated_at": now_iso(),
        "seed": {
            "path": str(Path(args.seed).relative_to(args.repo)),
            "fixture_version": seed.get("fixture_version"),
            "now_ms": seed.get("now_ms"),
        },
        "git": {
            "head": head,
            "dirty_file_count": len(status),
            "dirty_sample": status[:12],
            "theme_rs_dirty": bool(theme_diff.strip()),
        },
        "host": {"sw_vers": sw, "display": display_lines},
        "window": normalize,
        "assertions": {"initial": assert_initial, "final": assert_final},
        "structural_pass": assert_initial["pass"] and assert_final["pass"],
        "gate": {
            "exit_code": args.gate_exit,
            "threshold": diff_report["threshold"],
            "zone_pass": sum(1 for zone in zones if zone["pass"]),
            "zone_total": len(zones),
            "zone_results": [
                {"id": zone["id"], "ssim": zone["ssim"], "pass": zone["pass"]}
                for zone in zones
            ],
            "global_ssim": diff_report["global"]["ssim"],
        },
        "evidence": {
            "current": "current.png",
            "ax_initial": "ax-tree-initial.txt",
            "ax_final": "ax-tree.txt",
            "geometry_initial": "geometry-initial.txt",
            "geometry_final": "geometry-final.txt",
            "action_press": "action-press-session.txt",
            "action_trace": "action-trace.txt",
            "window_placement": "window-place.txt",
            "diff": "diff/",
            "logs": "logs/",
            "barriers": "barriers/",
        },
    }
    (out_dir / "run-manifest.json").write_text(json.dumps(manifest, indent=2) + NL, "utf-8")
    print("manifest written")
    return 0


def cmd_checklist(args):
    out_dir = Path(args.dir)
    manifest = json.loads((out_dir / "run-manifest.json").read_text("utf-8"))
    lines = [
        "# State A checklist-current - " + manifest["label"],
        "",
        "Generated " + manifest["generated_at"] + "; manifest: run-manifest.json.",
        "",
        "Structural assertions (AX tree + frame probe):",
        "",
    ]
    for phase in ("initial", "final"):
        for check in manifest["assertions"][phase]["checks"]:
            if check["pass"]:
                mark = "PASS"
            elif check.get("blocking", True):
                mark = "FAIL"
            else:
                mark = "OBSERVED-FAIL"
            lines.append("- [" + mark + "] " + phase + "/" + check["name"] + " - " + check["detail"])
    gate = manifest["gate"]
    lines += [
        "",
        "Visual gate (R1 record; zone FAIL expected until R2 restoration):",
        "",
        "- gate exit=" + str(gate["exit_code"]) + "; zones passed "
        + str(gate["zone_pass"]) + "/" + str(gate["zone_total"])
        + " (threshold=" + str(gate["threshold"]) + "); global SSIM="
        + format(gate["global_ssim"], ".6f") + " (auxiliary)",
    ]
    for zone in gate["zone_results"]:
        mark = "PASS" if zone["pass"] else "FAIL"
        lines.append("- [" + mark + "] zone " + zone["id"] + " ssim=" + format(zone["ssim"], ".6f"))
    lines += [
        "",
        "Evidence pointers: current.png / ax-tree-initial.txt / ax-tree.txt /",
        "geometry-initial.txt / geometry-final.txt / action-press-session.txt /",
        "action-trace.txt / window-place.txt / diff/diff-report.json / logs/ /",
        "barriers/ / normalize.json",
    ]
    (out_dir / "checklist-current.md").write_text(NL.join(lines) + NL, "utf-8")
    print("checklist written")
    return 0


def zone_fingerprint(report):
    keys = (
        "id", "anchor", "reference", "current", "common_size",
        "aligned_reference_origin", "aligned_current_origin",
        "coverage_reference", "coverage_current", "evaluated_pixels",
        "masked_pixels", "mask_fraction", "max_mask_fraction",
        "ssim_channels", "ssim", "pass",
    )
    return {
        "size": report.get("size"),
        "threshold": report.get("threshold"),
        "max_mask_fraction": report.get("max_mask_fraction"),
        "mask_count": report.get("mask_count"),
        "zones": [{key: zone[key] for key in keys} for zone in report["zones"]],
        "global": {key: report["global"][key] for key in keys if key in report["global"]},
        "heatmap_masked_pixels": report.get("heatmap_masked_pixels"),
        "heatmap_mask_fraction": report.get("heatmap_mask_fraction"),
        "pass": report.get("pass"),
    }


def cmd_compare(args):
    report_a = json.loads((Path(args.a) / "diff" / "diff-report.json").read_text("utf-8"))
    report_b = json.loads((Path(args.b) / "diff" / "diff-report.json").read_text("utf-8"))
    fa = zone_fingerprint(report_a)
    fb = zone_fingerprint(report_b)
    identical = json.dumps(fa, sort_keys=True) == json.dumps(fb, sort_keys=True)
    differences = []
    for za, zb in zip(fa["zones"], fb["zones"]):
        if za != zb:
            for key in za:
                if za[key] != zb[key]:
                    differences.append({"zone": za["id"], "field": key, "a": za[key], "b": zb[key]})
    payload = {
        "generated_at": now_iso(),
        "run_a": str(Path(args.a)),
        "run_b": str(Path(args.b)),
        "compared": "diff-report zones/global numeric fields (paths and timestamps excluded)",
        "identical": identical,
        "differences": differences,
        "zone_ssim": {
            zone["id"]: [zone["ssim"], next(
                z["ssim"] for z in fb["zones"] if z["id"] == zone["id"]
            )]
            for zone in fa["zones"]
        },
    }
    Path(args.report).write_text(json.dumps(payload, indent=2) + NL, "utf-8")
    print("identical=" + str(identical) + " differences=" + str(len(differences)))
    return 0 if identical else 6


def cmd_write_current_zones(args):
    zones = json.loads(Path(args.zones).read_text("utf-8"))
    frames = parse_frames(args.frames)
    root = frames.get("pawork-root")
    if root is None or root.get("error"):
        print("pawork-root frame missing", file=sys.stderr)
        return 3
    origin_x = root["x"]
    origin_y = root["y"]

    def relative(identifier):
        frame = frames.get(identifier)
        if frame is None or frame.get("error"):
            return None
        return {
            "x": int(round(frame["x"] - origin_x)),
            "y": int(round(frame["y"] - origin_y)),
            "w": int(round(frame["w"])),
            "h": int(round(frame["h"])),
        }

    rail = relative("task-rail")
    workspace = relative("workspace")
    inspector = relative("inspector")
    composer = relative("composer")
    status = relative("status-bar")
    if not all((rail, workspace, inspector, composer, status)):
        print("missing AX frames for zone mapping", file=sys.stderr)
        return 3
    zones_by_id = {zone["id"]: zone for zone in zones["zones"]}

    def anchored(zone_id, region):
        zone = zones_by_id[zone_id]
        width = min(int(zone["w"]), region["w"])
        height = min(int(zone["h"]), region["h"])
        anchor = zone.get("anchor", "top-left")
        x = region["x"]
        y = region["y"]
        if anchor.endswith("right"):
            x += region["w"] - width
        if anchor.startswith("bottom"):
            y += region["h"] - height
        return {"x": x, "y": y, "w": width, "h": height}

    # AX exposes workspace/timeline as broad semantic regions rather than a
    # dedicated header frame. Preserve the frozen zone sizes and anchors inside
    # the measured live regions; geometry assertions separately retain evidence
    # for extra or missing layout space (notably the known tall Composer).
    header_height = min(zones_by_id["header-left"]["h"], workspace["h"])
    header_region = {
        "x": workspace["x"],
        "y": workspace["y"],
        "w": workspace["w"],
        "h": header_height,
    }
    timeline_region = {
        "x": workspace["x"],
        "y": workspace["y"] + header_height,
        "w": workspace["w"],
        "h": max(1, composer["y"] - (workspace["y"] + header_height)),
    }
    rail_surface = dict(rail)
    rail_surface["h"] = max(1, int(round(root["h"])) - rail_surface["y"])
    mapping = {
        # The AX rail excludes the global 24px StatusBar, while the frozen
        # TaskRail visual zone spans the full root height at the measured width.
        "taskrail": anchored("taskrail", rail_surface),
        "header-left": anchored("header-left", header_region),
        "header-right": anchored("header-right", header_region),
        "timeline": anchored("timeline", timeline_region),
        "composer-left": anchored("composer-left", composer),
        "composer-right": anchored("composer-right", composer),
        "inspector-body": anchored("inspector-body", inspector),
        "inspector-right": anchored("inspector-right", inspector),
        "statusbar": anchored("statusbar", status),
    }
    zones["current_mapping_note"] = (
        "Measured AX regions with frozen reference-sized, anchor-aligned crops; "
        "structural geometry assertions record uncovered live-region drift."
    )
    updated = []
    for zone in zones["zones"]:
        rect = mapping.get(zone["id"])
        if rect is None:
            print("no current rect for zone " + zone["id"], file=sys.stderr)
            return 3
        if any(rect[key] < 0 for key in ("x", "y", "w", "h")):
            print("negative current rect for zone " + zone["id"], file=sys.stderr)
            return 3
        zone["current"] = rect
        updated.append(zone["id"])
    Path(args.out).write_text(json.dumps(zones, indent=2) + NL, "utf-8")
    print("zones current updated: " + ",".join(updated))
    return 0


def parse_states_tree(path):
    """R4 Wave B states 解析：审批卡 / 终态摘要 / 工具行 / composer 按钮。

    与 parse_tree 独立（增量面）：label 落在 description= 属性、节点
    description 落在 help= 属性（macos.rs 桥接语义，r4-wave-a 证据核实）。
    """
    lines = [
        line
        for line in Path(path).read_text("utf-8").splitlines()
        if not line.lstrip().startswith("#")
    ]
    identifiers = set()
    nodes = {}
    connection_status = ""
    for line in lines:
        ids = IDENTIFIER_RE.findall(line)
        if not ids:
            continue
        identifier = ids[0]
        identifiers.add(identifier)
        values = VALUE_RE.findall(line)
        descriptions = DESCRIPTION_RE.findall(line)
        helps = HELP_RE.findall(line)
        enabled = ENABLED_RE.findall(line)
        nodes[identifier] = {
            "value": values[0] if values else "",
            "label": descriptions[0] if descriptions else "",
            "help": helps[0] if helps else "",
            "enabled": enabled[0] if enabled else None,
            "selected": "selected=1" in line,
        }
        if identifier == "connection-status":
            connection_status = values[0] if values else ""
    timeline_values = {
        identifier: facts["value"]
        for identifier, facts in nodes.items()
        if identifier.startswith("timeline-entry-")
    }
    return {
        "identifiers": identifiers,
        "nodes": nodes,
        "connection_status": connection_status,
        "timeline_values": timeline_values,
    }


def states_check(name, ok, detail):
    return {"name": name, "pass": bool(ok), "detail": detail}


def states_selected_row(tree, expected):
    selected = [
        identifier
        for identifier, facts in tree["nodes"].items()
        if identifier.startswith("session-") and facts["selected"]
    ]
    return states_check(
        "selected-" + expected,
        selected == [expected],
        "selected rows: " + (",".join(selected) or "none") + " (expect " + expected + ")",
    )


def states_approval_checks(tree, present):
    card = tree["nodes"].get(APPROVAL_CARD)
    buttons = [tree["nodes"].get(name) for name in APPROVAL_BUTTONS]
    if present:
        checks = [
            states_check(
                "approval-card-present",
                card is not None,
                "approval-card " + (
                    "present value=" + card["value"] if card is not None else "missing"
                ),
            ),
            states_check(
                "approval-card-tool",
                card is not None and "write_file" in card["value"],
                "approval-card value="
                + (card["value"] if card is not None else "absent")
                + " (expect pending write_file)",
            ),
        ]
        missing = [
            name for name, button in zip(APPROVAL_BUTTONS, buttons)
            if button is None or button["enabled"] != "1"
        ]
        checks.append(states_check(
            "approval-buttons-enabled",
            not missing,
            ("missing/disabled: " + ",".join(missing)) if missing
            else "approve-once/approve-for-run/approve-deny present and enabled",
        ))
    else:
        stray = [name for name in (APPROVAL_CARD, *APPROVAL_BUTTONS) if name in tree["identifiers"]]
        checks = [states_check(
            "approval-card-absent",
            not stray,
            ("stray approval nodes: " + ",".join(stray)) if stray
            else "approval-card and decision buttons absent",
        )]
    return checks


def states_summary_checks(tree, title, help_contains=None, footer_prefix=None, help_exact=None):
    cards = [
        (identifier, facts)
        for identifier, facts in tree["nodes"].items()
        if identifier.startswith("run-summary-card-") and facts["label"] == title
    ]
    checks = [states_check(
        "run-summary-card-" + title,
        bool(cards),
        "run-summary-card-* with label " + title + ": "
        + (",".join(identifier for identifier, _ in cards) or "none"),
    )]
    if help_contains is not None:
        reasons = [facts["help"] for _, facts in cards]
        checks.append(states_check(
            "run-summary-help-contains",
            any(help_contains in reason for reason in reasons),
            "card help values: " + (" | ".join(reasons) or "none")
            + " (expect substring: " + help_contains + ")",
        ))
    if help_exact is not None:
        reasons = [facts["help"] for _, facts in cards]
        checks.append(states_check(
            "run-summary-help-exact",
            any(help_exact == reason for reason in reasons),
            "card help values: " + (" | ".join(reasons) or "none")
            + " (expect exact: " + help_exact + ")",
        ))
    if footer_prefix is not None:
        footers = [
            facts["label"]
            for identifier, facts in tree["nodes"].items()
            if identifier.startswith("run-footer-") and facts["label"].startswith(footer_prefix)
        ]
        checks.append(states_check(
            "run-footer-" + footer_prefix,
            bool(footers),
            "run-footer labels: "
            + (" | ".join(footers[:4]) if footers else "none")
            + " (expect prefix: " + footer_prefix + ")",
        ))
    return checks


def states_timeline_value_check(tree, needle, name):
    hits = [
        identifier
        for identifier, value in tree["timeline_values"].items()
        if needle in value
    ]
    return states_check(
        name,
        bool(hits),
        "timeline-entry values containing " + needle + ": "
        + (",".join(hits[:4]) or "none"),
    )


def states_composer_checks(tree, cancel_enabled, send_enabled):
    checks = []
    for name, expected in (("cancel", cancel_enabled), ("send", send_enabled)):
        button = tree["nodes"].get(name)
        wanted = "1" if expected else "0"
        checks.append(states_check(
            "composer-" + name + "-enabled-" + wanted,
            button is not None and button["enabled"] == wanted,
            name + " enabled=" + (button["enabled"] if button else "absent")
            + " (expect " + wanted + ")",
        ))
    return checks


def states_ax_timeline_rows(tree):
    return sorted(
        identifier
        for identifier in tree["nodes"]
        if identifier.startswith("timeline-entry-") or identifier.startswith("run-summary-")
    )


def states_phase_checks(tree, phase, logical_entries=None):
    if phase not in STATES_PHASES:
        raise ValueError("unknown states phase: " + str(phase))
    checks = []
    nodes = tree["nodes"]
    if phase == "approval-visible":
        checks.append(states_selected_row(tree, APPROVAL_SESSION_ROW))
        checks.extend(states_approval_checks(tree, True))
        pending_help = nodes.get(APPROVAL_SESSION_ROW, {}).get("help", "")
        checks.append(states_check(
            "rail-needs-input",
            "Needs input" in pending_help,
            APPROVAL_SESSION_ROW + " help=" + (pending_help or "absent"),
        ))
        checks.append(states_timeline_value_check(
            tree, "approval requested", "timeline-approval-requested",
        ))
    elif phase == "approval-resolved":
        checks.append(states_selected_row(tree, APPROVAL_SESSION_ROW))
        checks.extend(states_approval_checks(tree, False))
        pending_help = nodes.get(APPROVAL_SESSION_ROW, {}).get("help", "")
        checks.append(states_check(
            "rail-needs-input-cleared",
            "Needs input" not in pending_help,
            APPROVAL_SESSION_ROW + " help=" + (pending_help or "absent"),
        ))
        # wire 契约：决策行只经快照重放（重选/重连）出现，live 不推——
        # 即时断言交给 approval-replayed；这里只断言 Resolved 调用
        # 诚实显示 failed（仿 tool-failed 臂扫描，限当前会话行防他
        # session 的 failed 行污染）。
        failed_rows = [
            (identifier, facts)
            for identifier, facts in nodes.items()
            if identifier.startswith("tool-row-")
            and APPROVAL_SESSION_ID in identifier
            and facts["value"] == "failed"
        ]
        checks.append(states_check(
            "approval-tool-row-failed-value",
            bool(failed_rows),
            "approval tool-row value=failed: "
            + (",".join(identifier for identifier, _ in failed_rows) or "none")
            + " (session " + APPROVAL_SESSION_ID + ")",
        ))
    elif phase == "approval-replayed":
        # 重选会话后的快照重放一致性：选中行回到 beta-pending、卡不
        # 复活、决策行 approval approve_once 出现。
        checks.append(states_selected_row(tree, APPROVAL_SESSION_ROW))
        checks.extend(states_approval_checks(tree, False))
        checks.append(states_timeline_value_check(
            tree, "approval approve_once", "timeline-approval-decision",
        ))
    elif phase == "failed-summary":
        checks.append(states_selected_row(tree, "session-fx-ses-alpha-yesterday"))
        checks.extend(states_summary_checks(
            tree, "Run failed", help_contains=FAILED_REASON, footer_prefix="Run failed ·",
        ))
    elif phase == "cancelled-summary":
        checks.append(states_selected_row(tree, "session-fx-ses-beta-cancelled"))
        checks.extend(states_summary_checks(
            tree, "Run cancelled", footer_prefix="Run cancelled ·",
        ))
    elif phase == "tool-failed":
        checks.append(states_selected_row(tree, "session-fx-ses-beta-toolfailed"))
        rows = [
            (identifier, facts)
            for identifier, facts in nodes.items()
            if identifier.startswith("tool-row-")
        ]
        failed = [(identifier, facts) for identifier, facts in rows if facts["value"] == "failed"]
        checks.append(states_check(
            "tool-row-failed-value",
            bool(failed),
            "tool-row value=failed: " + (",".join(i for i, _ in failed) or "none"),
        ))
        checks.append(states_check(
            "tool-row-failure-detail",
            any(TOOL_FAILED_DETAIL in facts["help"] for _, facts in failed),
            "failed tool-row help: "
            + (" | ".join(facts["help"] for _, facts in failed) or "none")
            + " (expect substring: " + TOOL_FAILED_DETAIL + ")",
        ))
    elif phase == "virtualized":
        checks.append(states_selected_row(tree, "session-fx-ses-beta-long"))
        expected = BETA_LONG_LOGICAL_ROWS
        if logical_entries is not None:
            expected = logical_entries
        checks.append(states_check(
            "virtualization-logical-rows",
            expected == BETA_LONG_LOGICAL_ROWS,
            "barrier entry_count=" + str(expected)
            + " (fixture contract " + str(BETA_LONG_LOGICAL_ROWS) + " logical rows)",
        ))
        ax_rows = states_ax_timeline_rows(tree)
        checks.append(states_check(
            "virtualization-window-slice",
            len(ax_rows) < expected,
            "AX timeline-entry-*/run-summary-* nodes=" + str(len(ax_rows))
            + " < logical rows=" + str(expected)
            + " (capacity=ceil(frame.height/52) window slice)",
        ))
    elif phase == "streamed-summary":
        checks.append(states_selected_row(tree, "session-fx-ses-alpha-today"))
        checks.extend(states_summary_checks(tree, "Ready for review"))
        completed = [
            identifier
            for identifier, value in tree["timeline_values"].items()
            if value in ("run completed", "Run completed")
        ]
        checks.append(states_check(
            "timeline-run-completed",
            bool(completed),
            "terminal run rows: " + (",".join(completed[:4]) or "none"),
        ))
        composer = nodes.get("composer-input")
        checks.append(states_check(
            "composer-cleared",
            composer is not None and composer["value"] == "",
            "composer-input value=" + (repr(composer["value"]) if composer else "absent"),
        ))
    elif phase == "live-failed":
        checks.append(states_selected_row(tree, "session-fx-ses-alpha-today"))
        checks.extend(states_summary_checks(
            tree, "Run failed", help_exact=LIVE_FAILED_FALLBACK,
            footer_prefix="Run failed ·",
        ))
    elif phase == "failed-replayed":
        checks.append(states_selected_row(tree, "session-fx-ses-alpha-today"))
        checks.extend(states_summary_checks(
            tree, "Run failed", help_contains=FAILED_REASON,
            footer_prefix="Run failed ·",
        ))
    elif phase == "hang-cancelable":
        checks.append(states_selected_row(tree, "session-fx-ses-alpha-today"))
        checks.extend(states_composer_checks(tree, cancel_enabled=True, send_enabled=False))
    elif phase == "hang-cancelled":
        checks.append(states_selected_row(tree, "session-fx-ses-alpha-today"))
        checks.extend(states_summary_checks(
            tree, "Run cancelled", footer_prefix="Run cancelled ·",
        ))
        checks.extend(states_composer_checks(tree, cancel_enabled=False, send_enabled=True))
    elif phase == "disconnected-retained":
        checks.append(states_check(
            "reconnect-present",
            "reconnect" in tree["identifiers"],
            "reconnect " + ("present" if "reconnect" in tree["identifiers"] else "missing"),
        ))
        status = tree["connection_status"]
        checks.append(states_check(
            "connection-status-disconnected",
            bool(status) and status.startswith("Disconnected ·"),
            "connection-status value=" + (status or "absent"),
        ))
        checks.append(states_selected_row(tree, "session-fx-ses-alpha-today"))
        checks.append(states_check(
            "timeline-retained",
            bool(tree["timeline_values"]),
            "timeline-entry-* retained count=" + str(len(tree["timeline_values"])),
        ))
    elif phase == "reconnected-replay":
        checks.append(states_check(
            "reconnect-absent",
            "reconnect" not in tree["identifiers"],
            "reconnect " + ("stray present" if "reconnect" in tree["identifiers"] else "absent"),
        ))
        status = tree["connection_status"]
        checks.append(states_check(
            "connection-status-connected",
            bool(status) and status.startswith("Local · Connected"),
            "connection-status value=" + (status or "absent"),
        ))
        checks.append(states_selected_row(tree, "session-fx-ses-alpha-today"))
        checks.append(states_check(
            "timeline-replayed",
            bool(tree["timeline_values"]),
            "timeline-entry-* replayed count=" + str(len(tree["timeline_values"])),
        ))
    return checks


def cmd_states_assert(args):
    tree = parse_states_tree(args.tree)
    checks = states_phase_checks(tree, args.phase, args.logical_entries)
    payload = {
        "phase": args.phase,
        "generated_at": now_iso(),
        "checks": checks,
        "pass": all(check["pass"] for check in checks),
    }
    Path(args.out).write_text(json.dumps(payload, indent=2) + NL, "utf-8")
    for check in checks:
        prefix = "PASS " if check["pass"] else "FAIL "
        print(prefix + check["name"] + " - " + check["detail"])
    return 0 if payload["pass"] else 5


def cmd_approval_read(args):
    try:
        payload = json.loads(Path(args.file).read_text("utf-8"))
    except (OSError, ValueError):
        return 1
    tool = payload.get("tool")
    run_id = payload.get("run_id")
    if not tool or not run_id:
        return 1
    print(str(tool) + " " + str(run_id))
    return 0


def normalize_live_value(value):
    """live/replay 唯一已拍板的文案差：run failed · <reason> 归一为 run failed。"""
    if value.startswith("run failed · "):
        return "run failed"
    return value


def live_identifier_class(identifier):
    """live 产生行的三类天然 id 形态（计数报告用，不参与相等判定）。"""
    if identifier.startswith("timeline-entry-app-evt-"):
        return "app-evt"
    if identifier.startswith("timeline-entry-local-echo-"):
        return "local-echo"
    return "persisted-evt"


def live_class_counts(live_rows):
    counts = Counter(
        live_identifier_class(identifier) for identifier in live_rows
    )
    return {
        "total": len(live_rows),
        "app-evt": counts["app-evt"],
        "local-echo": counts["local-echo"],
        "persisted-evt": counts["persisted-evt"],
    }


def value_multiset_diff_summary(only_a, only_b):
    def render(counter):
        return ",".join(
            value + " x" + str(count) for value, count in sorted(counter.items())
        ) or "none"

    return "only-before: " + render(only_a) + "; only-after: " + render(only_b)


def cmd_entry_compare(args):
    """R4 Wave B WS-4b entry-compare v2 三重合同：
    1. barrier entry_count 相等；
    2. 种子行（evt-fx-*）identifier 集合一致——种子历史是重放一致性主体；
    3. live 产生行（非 evt-fx-*）value 多重集一致（比较前做 run failed · …
       诚实降级归一）；identifier 不参与——app-evt-* / local-echo-* /
       持久化 evt-* 三类 id 天然不同。
    """
    tree_a = parse_states_tree(args.tree_a)
    tree_b = parse_states_tree(args.tree_b)
    seed_a = {
        identifier
        for identifier in tree_a["timeline_values"]
        if identifier.startswith(SEED_IDENTIFIER_PREFIX)
    }
    seed_b = {
        identifier
        for identifier in tree_b["timeline_values"]
        if identifier.startswith(SEED_IDENTIFIER_PREFIX)
    }
    live_a = {
        identifier: value
        for identifier, value in tree_a["timeline_values"].items()
        if not identifier.startswith(SEED_IDENTIFIER_PREFIX)
    }
    live_b = {
        identifier: value
        for identifier, value in tree_b["timeline_values"].items()
        if not identifier.startswith(SEED_IDENTIFIER_PREFIX)
    }
    values_a = Counter(
        normalize_live_value(value) for value in live_a.values()
    )
    values_b = Counter(
        normalize_live_value(value) for value in live_b.values()
    )
    classes_a = live_class_counts(live_a)
    classes_b = live_class_counts(live_b)

    def class_line():
        return (
            "live classes before: app-evt=" + str(classes_a["app-evt"])
            + " local-echo=" + str(classes_a["local-echo"])
            + " persisted-evt=" + str(classes_a["persisted-evt"])
            + "; after: app-evt=" + str(classes_b["app-evt"])
            + " local-echo=" + str(classes_b["local-echo"])
            + " persisted-evt=" + str(classes_b["persisted-evt"])
        )

    checks = [
        states_check(
            "barrier-entry-count-equal",
            args.entries_a == args.entries_b,
            "timeline_stable entry_count before=" + str(args.entries_a)
            + " after=" + str(args.entries_b),
        ),
        states_check(
            "timeline-seed-identifier-sets-identical",
            seed_a == seed_b,
            "seed evt-fx identifier sets "
            + ("identical" if seed_a == seed_b else "differ")
            + " (before=" + str(len(seed_a)) + " after=" + str(len(seed_b)) + ")",
        ),
        states_check(
            "timeline-live-value-multisets-equal",
            values_a == values_b,
            "live value multisets (normalized) "
            + ("identical" if values_a == values_b else "differ")
            + " (before=" + str(sum(values_a.values()))
            + " after=" + str(sum(values_b.values())) + "); " + class_line(),
        ),
    ]
    only_seed_a = sorted(seed_a - seed_b)
    only_seed_b = sorted(seed_b - seed_a)
    if only_seed_a or only_seed_b:
        checks.append(states_check(
            "timeline-seed-identifier-diffs",
            False,
            "only-before: " + (",".join(only_seed_a[:6]) or "none")
            + "; only-after: " + (",".join(only_seed_b[:6]) or "none"),
        ))
    only_values_a = values_a - values_b
    only_values_b = values_b - values_a
    if only_values_a or only_values_b:
        checks.append(states_check(
            "timeline-live-value-diffs",
            False,
            value_multiset_diff_summary(only_values_a, only_values_b),
        ))
    payload = {
        "generated_at": now_iso(),
        "checks": checks,
        "pass": all(check["pass"] for check in checks),
        "entries_before": args.entries_a,
        "entries_after": args.entries_b,
        "seed_counts": {"before": len(seed_a), "after": len(seed_b)},
        "live_counts": {"before": classes_a, "after": classes_b},
    }
    Path(args.out).write_text(json.dumps(payload, indent=2) + NL, "utf-8")
    for check in checks:
        prefix = "PASS " if check["pass"] else "FAIL "
        print(prefix + check["name"] + " - " + check["detail"])
    return 0 if payload["pass"] else 6


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    barrier = sub.add_parser("barrier-read")
    barrier.add_argument("--file", required=True)
    barrier.add_argument("--field", choices=["line", "seq", "session", "entries"], default="line")
    barrier.set_defaults(func=cmd_barrier_read)
    asrt = sub.add_parser("assert")
    asrt.add_argument("--frames", required=True)
    asrt.add_argument("--tree", required=True)
    asrt.add_argument(
        "--phase",
        choices=[
            "initial",
            "empty",
            "final",
            "restored",
            "resumed",
            "narrow",
            "collapsed",
            "disconnected",
            "connect-failed",
            "reconnected",
            "projects",
        ],
        required=True,
    )
    asrt.add_argument("--out", required=True)
    asrt.set_defaults(func=cmd_assert)
    shell_man = sub.add_parser("shell-manifest")
    shell_man.add_argument("--dir", required=True)
    shell_man.add_argument("--repo", required=True)
    shell_man.add_argument("--scenario", required=True)
    shell_man.add_argument("--label", required=True)
    shell_man.set_defaults(func=cmd_shell_manifest)
    norm = sub.add_parser("normalize")
    norm.add_argument("--shot", required=True)
    norm.add_argument("--tree", required=True)
    norm.add_argument("--wid", required=True)
    norm.add_argument("--frames", required=True)
    norm.add_argument("--out", required=True)
    norm.add_argument("--json", required=True)
    norm.set_defaults(func=cmd_normalize)
    man = sub.add_parser("manifest")
    man.add_argument("--dir", required=True)
    man.add_argument("--repo", required=True)
    man.add_argument("--seed", required=True)
    man.add_argument("--scenario", required=True)
    man.add_argument("--label", required=True)
    man.add_argument("--gate-exit", type=int, required=True)
    man.set_defaults(func=cmd_manifest)
    chk = sub.add_parser("checklist")
    chk.add_argument("--dir", required=True)
    chk.set_defaults(func=cmd_checklist)
    cmp_parser = sub.add_parser("compare")
    cmp_parser.add_argument("--a", required=True)
    cmp_parser.add_argument("--b", required=True)
    cmp_parser.add_argument("--report", required=True)
    cmp_parser.set_defaults(func=cmd_compare)
    zones_cmd = sub.add_parser("write-current-zones")
    zones_cmd.add_argument("--zones", required=True)
    zones_cmd.add_argument("--frames", required=True)
    zones_cmd.add_argument("--out", required=True)
    zones_cmd.set_defaults(func=cmd_write_current_zones)
    states_asrt = sub.add_parser("states-assert")
    states_asrt.add_argument("--tree", required=True)
    states_asrt.add_argument("--phase", choices=list(STATES_PHASES), required=True)
    states_asrt.add_argument("--out", required=True)
    states_asrt.add_argument("--logical-entries", type=int, default=None)
    states_asrt.set_defaults(func=cmd_states_assert)
    approval_read = sub.add_parser("approval-read")
    approval_read.add_argument("--file", required=True)
    approval_read.set_defaults(func=cmd_approval_read)
    entry_cmp = sub.add_parser("entry-compare")
    entry_cmp.add_argument("--tree-a", required=True)
    entry_cmp.add_argument("--tree-b", required=True)
    entry_cmp.add_argument("--entries-a", type=int, required=True)
    entry_cmp.add_argument("--entries-b", type=int, required=True)
    entry_cmp.add_argument("--out", required=True)
    entry_cmp.set_defaults(func=cmd_entry_compare)
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
