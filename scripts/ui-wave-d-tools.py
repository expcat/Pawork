#!/usr/bin/env python3
"""R1 Wave D helpers: AX tree/frame parsing, barrier reads, normalize, evidence."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

from PIL import Image

IDENTIFIER_RE = re.compile(r'identifier="([^"]*)"')

SESSION_ROW = "session-fx-ses-alpha-today"
TIMELINE_PREFIX = "timeline-entry-evt-fx-ses-alpha-today"
NL = chr(10)
RESAMPLE = getattr(Image, "Resampling", Image).LANCZOS


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
    identifiers = set()
    for line in lines:
        identifiers.update(IDENTIFIER_RE.findall(line))
    role_unknown = sum(1 for line in lines if "role=?" in line)
    focused = []
    selected_rows = []
    timeline_entries = 0
    for line in lines:
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
    return {
        "identifiers": identifiers,
        "role_unknown": role_unknown,
        "focused": focused,
        "selected_rows": selected_rows,
        "timeline_entries_alpha_today": timeline_entries,
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


def geometry_checks(frames):
    checks = []
    metrics = {}

    def add(name, ok, detail):
        checks.append({"name": name, "pass": bool(ok), "detail": detail})

    root = frames.get("pawork-root")
    if root is None or root.get("error"):
        add("root-frame", False, "pawork-root frame missing")
        return checks, metrics
    metrics["root"] = root
    add(
        "root-1440x1024",
        near(root["w"], 1440.0, 0.5) and near(root["h"], 1024.0, 0.5),
        "root w=" + str(root["w"]) + " h=" + str(root["h"]) + " (contract 1440x1024 +/-0.5)",
    )
    rail = frames.get("task-rail")
    rail_ok = rail is not None and not rail.get("error") and near(rail["w"], 288.0, 4.32)
    add(
        "rail-width",
        rail_ok,
        "task-rail w=" + (str(rail["w"]) if rail else "missing") + " (contract 288 +/-4.32)",
    )
    insp = frames.get("inspector")
    if insp and not insp.get("error"):
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
        add(
            "composer-height",
            86.0 <= composer["h"] <= 96.0,
            "composer h=" + str(composer["h"]) + " (contract 88-94, component tolerance [86,96])",
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
    if workspace and rail and insp and not any(
        v.get("error") for v in (workspace, rail, insp)
    ):
        span = workspace["x"] + workspace["w"] - root["x"]
        add(
            "workspace-span",
            near(workspace["x"] - root["x"], rail["w"], 1.0)
            and near(span, root["w"] - insp["w"], 1.0),
            "workspace x=" + str(round(workspace["x"] - root["x"], 2))
            + ".." + str(round(span, 2))
            + " expected " + str(rail["w"]) + ".." + str(root["w"] - insp["w"]),
        )
    return checks, metrics


def skeleton_checks(tree):
    required = [
        "task-rail",
        "session-list",
        SESSION_ROW,
        "workspace",
        "timeline",
        "composer",
        "composer-input",
        "inspector",
        "inspector-tabs",
        "status-bar",
    ]
    missing = [name for name in required if name not in tree["identifiers"]]
    return [
        {
            "name": "three-column-skeleton",
            "pass": not missing,
            "detail": ("missing: " + ",".join(missing)) if missing
                else "rail/workspace/inspector/composer/statusbar present",
        },
        {
            "name": "no-unknown-role",
            "pass": tree["role_unknown"] == 0,
            "detail": "role=? count=" + str(tree["role_unknown"]),
        },
    ]


def cmd_assert(args):
    frames = parse_frames(args.frames)
    tree = parse_tree(args.tree)
    checks, metrics = geometry_checks(frames)
    checks.extend(skeleton_checks(tree))
    if args.phase == "initial":
        focused = tree["focused"]
        ok = len(focused) <= 1 and all(node == "composer-input" for node in focused)
        checks.append({
            "name": "focus-start",
            "pass": ok,
            "detail": "initial focus=" + (",".join(focused) if focused else "none")
                + " (allowed: none or composer-input)",
        })
        checks.append({
            "name": "initial-selected-observed",
            "pass": True,
            "detail": "startup selected rows=" + (",".join(tree["selected_rows"]) or "none")
                + " (observation only)",
        })
    else:
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
        "pass": all(check["pass"] for check in checks),
    }
    Path(args.out).write_text(json.dumps(payload, indent=2) + NL, "utf-8")
    for check in checks:
        print(("PASS " if check["pass"] else "FAIL ") + check["name"] + " - " + check["detail"])
    return 0 if payload["pass"] else 5

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
    image = Image.open(Path(args.shot))
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
    cropped.save(Path(args.out))
    mapping = {
        "wid": int(wid),
        "window_bounds_points": window_bounds,
        "root_frame_points": {
            "x": root["x"], "y": root["y"], "w": root["w"], "h": root["h"],
        },
        "capture_pixels": [pre_w, pre_h],
        "scale": round(scale_x, 4),
        "crop_pixels": [crop_x, crop_y, crop_w, crop_h],
        "before_resize": [post_w, post_h],
        "resized_lanczos": resized,
        "final_size": [1440, 1024],
        "icc_profile_dropped": True,
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
            mark = "PASS" if check["pass"] else "FAIL"
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
        "action-trace.txt / diff/diff-report.json / logs/ / barriers/ / normalize.json",
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
    header_h = max(24, min(composer["y"] - workspace["y"], workspace["h"]))
    header_left_w = max(1, workspace["w"] // 2)
    composer_left_w = max(1, composer["w"] // 2)
    mapping = {
        "taskrail": rail,
        "statusbar": status,
        "inspector-body": inspector,
        "header-left": {
            "x": workspace["x"],
            "y": workspace["y"],
            "w": header_left_w,
            "h": header_h,
        },
        "header-right": {
            "x": workspace["x"] + header_left_w,
            "y": workspace["y"],
            "w": max(1, workspace["w"] - header_left_w),
            "h": header_h,
        },
        "timeline": {
            "x": workspace["x"],
            "y": workspace["y"] + header_h,
            "w": workspace["w"],
            "h": max(1, composer["y"] - (workspace["y"] + header_h)),
        },
        "composer-left": {
            "x": composer["x"],
            "y": composer["y"],
            "w": composer_left_w,
            "h": composer["h"],
        },
        "composer-right": {
            "x": composer["x"] + composer_left_w,
            "y": composer["y"],
            "w": max(1, composer["w"] - composer_left_w),
            "h": composer["h"],
        },
        "inspector-right": {
            "x": inspector["x"] + max(0, inspector["w"] - 120),
            "y": inspector["y"],
            "w": min(120, inspector["w"]),
            "h": min(120, inspector["h"]),
        },
    }
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
    asrt.add_argument("--phase", choices=["initial", "final"], required=True)
    asrt.add_argument("--out", required=True)
    asrt.set_defaults(func=cmd_assert)
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
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
