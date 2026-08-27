#!/usr/bin/env python3
"""Wave B regressions: phase-aware assertions, shell manifest, driver guards."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from PIL import Image

TOOLS = Path(__file__).with_name("ui-wave-d-tools.py")
DRIVER = Path(__file__).with_name("ui-wave-b-states.sh")
FIXTURE = Path(__file__).with_name("ui-fixture.sh")
FOCUS = Path(__file__).with_name("ui-focus-switch.sh")
STATE_A = Path(__file__).with_name("ui-wave-d-state-a.sh")
AX_FRAMES_SWIFT = Path(__file__).with_name("ui-ax-frames.swift")


def load_tools():
    spec = importlib.util.spec_from_file_location("ui_wave_d_tools", TOOLS)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def write_frames(path, root_w=1440.0, rail_w=288.0, inspector=True):
    root_x = 100.0
    root_y = 200.0
    ws_x = root_x + rail_w
    ws_w = root_w - rail_w - (440.0 if inspector else 0.0)
    insp_x = root_x + root_w - 440.0
    lines = [
        "id=pawork-root role=AXGroup x=" + str(root_x) + " y=" + str(root_y)
        + " w=" + str(root_w) + " h=1024.0",
        "id=task-rail role=AXGroup x=" + str(root_x) + " y=" + str(root_y)
        + " w=" + str(rail_w) + " h=1000.0",
        "id=workspace role=AXGroup x=" + str(ws_x) + " y=" + str(root_y)
        + " w=" + str(ws_w) + " h=1000.0",
        "id=timeline role=AXGroup x=" + str(ws_x) + " y=204.0 w=" + str(ws_w) + " h=840.0",
        "id=composer role=AXGroup x=" + str(ws_x) + " y=1044.0 w=" + str(ws_w)
        + " h=156.0",
        "id=composer-input role=AXTextArea x=" + str(ws_x) + " y=1044.0 w="
        + str(ws_w) + " h=156.0",
        "id=status-bar role=AXGroup x=" + str(root_x) + " y=1200.0 w="
        + str(root_w) + " h=24.0",
    ]
    if inspector:
        lines.append(
            "id=inspector role=AXGroup x=" + str(insp_x) + " y=" + str(root_y)
            + " w=440.0 h=1000.0"
        )
        lines.append(
            "id=inspector-tabs role=AXTabGroup x=" + str(insp_x) + " y="
            + str(root_y) + " w=440.0 h=40.0"
        )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def write_tree(
    path,
    selected=False,
    entries=0,
    empty_hint=False,
    inspector=True,
    popover=False,
    reconnect=False,
    toggle=True,
):
    lines = [
        'role=AXGroup identifier="pawork-root"',
        'role=AXGroup identifier="task-rail"',
        'role=AXGroup identifier="session-list"',
        'role=AXRow identifier="session-fx-ses-alpha-today"'
        + (" selected=1" if selected else ""),
        'role=AXGroup identifier="workspace"',
        'role=AXGroup identifier="timeline"',
        'role=AXGroup identifier="composer"',
        'role=AXTextArea identifier="composer-input" focused=1',
        'role=AXGroup identifier="status-bar"',
    ]
    if empty_hint:
        lines.append('role=AXStaticText identifier="workspace-empty-hint"')
    if reconnect:
        lines.append('role=AXButton identifier="reconnect"')
    if inspector:
        lines.append('role=AXGroup identifier="inspector"')
        lines.append('role=AXTabGroup identifier="inspector-tabs"')
    else:
        if toggle:
            lines.append('role=AXButton identifier="inspector-toggle"')
        if popover:
            lines.append('role=AXGroup identifier="activity-popover"')
    for index in range(entries):
        lines.append(
            'role=AXGroup identifier="timeline-entry-evt-fx-ses-alpha-today-'
            + str(index) + '"'
        )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def run_assert(raw, phase, frames_kwargs=None, tree_kwargs=None):
    frames_kwargs = frames_kwargs if frames_kwargs is not None else {}
    tree_kwargs = tree_kwargs if tree_kwargs is not None else {}
    frames = Path(raw) / ("geometry-" + phase + ".txt")
    tree = Path(raw) / ("ax-tree-" + phase + ".txt")
    out = Path(raw) / ("assert-" + phase + ".json")
    write_frames(frames, **frames_kwargs)
    write_tree(tree, **tree_kwargs)
    proc = subprocess.run(
        [
            sys.executable, str(TOOLS), "assert",
            "--frames", str(frames),
            "--tree", str(tree),
            "--phase", phase,
            "--out", str(out),
        ],
        check=False, capture_output=True, text=True,
    )
    payload = json.loads(out.read_text(encoding="utf-8"))
    return proc, payload


class WaveBToolsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tools = load_tools()

    def test_narrow_phase_accepts_1080_shell_without_inspector(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "narrow",
                frames_kwargs={"root_w": 1080.0, "rail_w": 240.0, "inspector": False},
                tree_kwargs={"inspector": False},
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            self.assertTrue(payload["pass"])
            names = {check["name"]: check for check in payload["checks"]}
            self.assertIn("root-1080x1024", names)
            self.assertTrue(names["root-1080x1024"]["pass"])
            self.assertTrue(names["rail-width"]["pass"])
            self.assertTrue(names["inspector-absent"]["pass"])
            self.assertTrue(names["statusbar-height"]["pass"])
            self.assertTrue(names["workspace-span"]["pass"])
            self.assertTrue(names["inspector-column-absent"]["pass"])
            self.assertNotIn("session-selected", names)

    def test_narrow_phase_fails_when_inspector_column_stays(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "narrow",
                frames_kwargs={"root_w": 1080.0, "rail_w": 240.0, "inspector": True},
                tree_kwargs={"inspector": True},
            )
            self.assertEqual(proc.returncode, 5)
            self.assertFalse(payload["pass"])
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["inspector-absent"]["pass"])
            self.assertFalse(names["inspector-column-absent"]["pass"])
            # narrow 骨架本身不要求 Inspector 列；缺席约束由
            # inspector-absent / inspector-column-absent 两个检查承担。
            self.assertTrue(names["shell-skeleton"]["pass"])

    def test_collapsed_phase_requires_toggle_and_popover_without_column(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "collapsed",
                frames_kwargs={"root_w": 1440.0, "rail_w": 288.0, "inspector": False},
                tree_kwargs={
                    "inspector": False,
                    "popover": True,
                    "selected": True,
                    "entries": 2,
                },
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertTrue(names["inspector-toggle-present"]["pass"])
            self.assertTrue(names["activity-popover-present"]["pass"])
            self.assertTrue(names["inspector-column-absent"]["pass"])
            self.assertTrue(names["session-selected"]["pass"])
            self.assertTrue(names["timeline-loaded"]["pass"])

    def test_collapsed_phase_fails_without_popover(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "collapsed",
                frames_kwargs={"root_w": 1440.0, "rail_w": 288.0, "inspector": False},
                tree_kwargs={"inspector": False, "popover": False, "selected": True},
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["activity-popover-present"]["pass"])
            self.assertFalse(payload["pass"])

    def test_empty_phase_requires_workspace_empty_hint_and_empty_timeline(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw, "empty", tree_kwargs={"empty_hint": True}
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertTrue(names["workspace-empty-hint-present"]["pass"])
            self.assertTrue(names["timeline-empty"]["pass"])

    def test_empty_phase_fails_without_hint(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(raw, "empty", tree_kwargs={"empty_hint": False})
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["workspace-empty-hint-present"]["pass"])

    def test_empty_phase_fails_when_reconnect_published(self):
        # AX/视觉同源谓词回归防线：Connected 空态不得发布 reconnect 节点。
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw, "empty", tree_kwargs={"empty_hint": True, "reconnect": True}
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["reconnect-absent"]["pass"])

    def test_collapsed_phase_fails_when_empty_hint_lingers(self):
        # 选中会话后空态引导必须消失（防谓词回归成恒真）。
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "collapsed",
                frames_kwargs={"root_w": 1440.0, "rail_w": 288.0, "inspector": False},
                tree_kwargs={
                    "inspector": False,
                    "popover": True,
                    "selected": True,
                    "entries": 2,
                    "empty_hint": True,
                },
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["workspace-empty-hint-absent"]["pass"])

    def test_narrow_phase_fails_without_inspector_toggle(self):
        # 折叠态触发器是 F-12 迁移前的临时主路径，窄窗相位也必须在场。
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "narrow",
                frames_kwargs={"root_w": 1080.0, "rail_w": 240.0, "inspector": False},
                tree_kwargs={"inspector": False, "toggle": False},
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["inspector-toggle-present"]["pass"])

    def test_normalize_rejects_non_1440_root(self):
        # 1080 相位截图送进 normalize 必须显式拒绝，不静默放大（P3 防误用）。
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            shot = root / "shot.png"
            Image.new("RGB", (1080, 1024), (20, 30, 40)).save(shot)
            tree = root / "tree.txt"
            tree.write_text("# wid=7 pid=123 bounds={0,0,1080,1024}\n", "utf-8")
            wid = root / "wid.txt"
            wid.write_text("7\n", "utf-8")
            frames = root / "frames.txt"
            frames.write_text(
                "id=pawork-root role=AXGroup x=0.0 y=0.0 w=1080.0 h=1024.0\n",
                "utf-8",
            )
            proc = subprocess.run(
                [
                    sys.executable, str(TOOLS), "normalize",
                    "--shot", str(shot),
                    "--tree", str(tree),
                    "--wid", str(wid),
                    "--frames", str(frames),
                    "--out", str(root / "current.png"),
                    "--json", str(root / "normalize.json"),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 3)
            self.assertIn("requires 1440x1024", proc.stderr)
            self.assertFalse((root / "current.png").exists())

    def test_restored_skips_session_checks_but_resumed_requires_them(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(raw, "restored", tree_kwargs={"selected": False})
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertNotIn("session-selected", names)
            self.assertTrue(names["root-1440x1024"]["pass"])
            self.assertTrue(names["inspector-width"]["pass"])
        with tempfile.TemporaryDirectory() as raw:
            proc, _ = run_assert(raw, "resumed", tree_kwargs={"selected": False})
            self.assertEqual(proc.returncode, 5)
            proc, payload = run_assert(
                raw,
                "resumed",
                tree_kwargs={"selected": True, "entries": 1},
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            self.assertTrue(payload["pass"])

    def test_geometry_default_phase_contract_unchanged(self):
        frames = {
            "pawork-root": {"role": "AXGroup", "x": 100.0, "y": 200.0, "w": 1440.0, "h": 1024.0},
            "task-rail": {"role": "AXGroup", "x": 100.0, "y": 200.0, "w": 288.0, "h": 1024.0},
            "workspace": {"role": "AXGroup", "x": 388.0, "y": 200.0, "w": 712.0, "h": 1000.0},
            "inspector": {"role": "AXGroup", "x": 1100.0, "y": 200.0, "w": 440.0, "h": 1000.0},
            "composer": {"role": "AXGroup", "x": 388.0, "y": 1044.0, "w": 712.0, "h": 156.0},
            "status-bar": {"role": "AXGroup", "x": 100.0, "y": 1200.0, "w": 1440.0, "h": 24.0},
        }
        checks, _ = self.tools.geometry_checks(frames)
        self.assertTrue(
            all(check["pass"] or not check.get("blocking", True) for check in checks)
        )
        names = [check["name"] for check in checks]
        self.assertIn("root-1440x1024", names)
        self.assertIn("rail-width", names)
        self.assertIn("inspector-width", names)

    def test_shell_manifest_aggregates_phase_assertions(self):
        with tempfile.TemporaryDirectory() as raw:
            out = Path(raw)
            (out / "assert-empty.json").write_text(
                json.dumps(
                    {"pass": True, "checks": [{"name": "shell-skeleton", "pass": True}]}
                ),
                encoding="utf-8",
            )
            (out / "assert-narrow.json").write_text(
                json.dumps(
                    {"pass": True, "checks": [{"name": "rail-width", "pass": True}]}
                ),
                encoding="utf-8",
            )
            cmd = [
                sys.executable, str(TOOLS), "shell-manifest",
                "--dir", str(out), "--repo", str(out),
                "--scenario", "wave-b-states", "--label", "test",
            ]
            proc = subprocess.run(cmd, check=False, capture_output=True, text=True)
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            manifest = json.loads((out / "run-manifest.json").read_text("utf-8"))
            self.assertEqual(sorted(manifest["phases"]), ["empty", "narrow"])
            self.assertTrue(manifest["structural_pass"])
            (out / "assert-narrow.json").write_text(
                json.dumps(
                    {"pass": False, "checks": [{"name": "rail-width", "pass": False}]}
                ),
                encoding="utf-8",
            )
            subprocess.run(cmd, check=True, capture_output=True, text=True)
            manifest = json.loads((out / "run-manifest.json").read_text("utf-8"))
            self.assertFalse(manifest["structural_pass"])

    def test_driver_rejects_stale_output_and_unknown_arguments(self):
        with tempfile.TemporaryDirectory() as raw:
            out = Path(raw) / "evidence"
            out.mkdir()
            (out / "stale.json").write_text("{}\n", encoding="utf-8")
            proc = subprocess.run(
                ["bash", str(DRIVER), "run", "--out", str(out)],
                check=False, capture_output=True, text=True,
            )
            self.assertEqual(proc.returncode, 3)
            self.assertIn("must be new or empty", proc.stderr)
            self.assertTrue((out / "stale.json").exists())
            proc = subprocess.run(
                [
                    "bash", str(DRIVER), "run",
                    "--out", str(Path(raw) / "fresh"), "--bogus",
                ],
                check=False, capture_output=True, text=True,
            )
            self.assertEqual(proc.returncode, 2)
            self.assertIn("unknown argument", proc.stderr)

    def test_shell_scripts_pass_bash_syntax_check(self):
        for script in (DRIVER, STATE_A, FIXTURE, FOCUS):
            proc = subprocess.run(
                ["bash", "-n", str(script)],
                check=False, capture_output=True, text=True,
            )
            self.assertEqual(proc.returncode, 0, script.name + ": " + proc.stderr)

    def test_fixture_help_documents_desktop_restart(self):
        proc = subprocess.run(
            ["bash", str(FIXTURE), "help"],
            check=False, capture_output=True, text=True,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("desktop-restart", proc.stdout)

    def test_ax_frames_helper_declares_resize_contract(self):
        source = AX_FRAMES_SWIFT.read_text(encoding="utf-8")
        self.assertIn("--place-main", source)
        self.assertIn("--resize", source)
        self.assertIn("kAXSizeAttribute", source)
        self.assertIn("kAXPositionAttribute", source)
        self.assertIn("0.5", source)


if __name__ == "__main__":
    unittest.main()
