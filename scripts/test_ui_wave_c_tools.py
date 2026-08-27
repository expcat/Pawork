#!/usr/bin/env python3
"""Wave C regressions: disconnected/reconnected phase assertions + driver guards."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).with_name("ui-wave-d-tools.py")
DRIVER = Path(__file__).with_name("ui-wave-c-connect.sh")


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
    reconnect=False,
    connection="Connected · pawork-test",
):
    # disconnected 相位正向用例必须显式传入 Disconnected/Connect failed 文案；
    # 默认 Connected 只适用于 reconnected。
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
        'role=AXGroup identifier="inspector"',
        'role=AXTabGroup identifier="inspector-tabs"',
    ]
    if empty_hint:
        lines.append('role=AXStaticText identifier="workspace-empty-hint"')
    if reconnect:
        lines.append('role=AXButton identifier="reconnect"')
    if connection is not None:
        lines.append(
            'role=AXStaticText value="' + connection
            + '" identifier="connection-status"'
        )
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


class WaveCToolsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tools = load_tools()

    def test_disconnected_phase_requires_reconnect_and_retained_state(self):
        # 断连语义：Reconnect 在场；壳层/会话选中/旧条目全部保留。
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "disconnected",
                tree_kwargs={
                    "reconnect": True,
                    "selected": True,
                    "entries": 2,
                    "connection": "Disconnected · transport error: ConnectionClosed",
                },
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            self.assertTrue(payload["pass"])
            names = {check["name"]: check for check in payload["checks"]}
            self.assertTrue(names["reconnect-present"]["pass"])
            self.assertTrue(names["shell-skeleton"]["pass"])
            self.assertTrue(names["rail-width"]["pass"])
            self.assertTrue(names["session-selected"]["pass"])
            self.assertTrue(names["timeline-loaded"]["pass"])
            self.assertTrue(names["workspace-empty-hint-absent"]["pass"])
            self.assertTrue(names["connection-status-disconnected"]["pass"])

    def test_disconnected_phase_fails_without_reconnect_button(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "disconnected",
                tree_kwargs={
                    "reconnect": False,
                    "selected": True,
                    "entries": 2,
                    "connection": "Disconnected · transport error: ConnectionClosed",
                },
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["reconnect-present"]["pass"])
            self.assertFalse(payload["pass"])

    def test_disconnected_phase_fails_when_timeline_cleared(self):
        # gui-design 空态原则：Disconnected 保留旧条目；条目被清空即回归。
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "disconnected",
                tree_kwargs={
                    "reconnect": True,
                    "selected": True,
                    "entries": 0,
                    "connection": "Disconnected · transport error: ConnectionClosed",
                },
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["timeline-loaded"]["pass"])

    def test_disconnected_phase_fails_when_empty_hint_lingers(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "disconnected",
                tree_kwargs={
                    "reconnect": True,
                    "selected": True,
                    "entries": 2,
                    "empty_hint": True,
                    "connection": "Disconnected · transport error: ConnectionClosed",
                },
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["workspace-empty-hint-absent"]["pass"])

    def test_disconnected_phase_rejects_connect_failed_status(self):
        # drop-socket 相位不得把 ConnectFailed 文案当成 Disconnected。
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "disconnected",
                tree_kwargs={
                    "reconnect": True,
                    "selected": True,
                    "entries": 2,
                    "connection": "Connect failed · transport error: ConnectionFailed",
                },
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["connection-status-disconnected"]["pass"])

    def test_connect_failed_phase_requires_failed_status_and_reconnect(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "connect-failed",
                tree_kwargs={
                    "reconnect": True,
                    "selected": True,
                    "entries": 2,
                    "connection": "Connect failed · transport error: ConnectionFailed",
                },
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertTrue(names["reconnect-present"]["pass"])
            self.assertTrue(names["connection-status-connect-failed"]["pass"])
            self.assertTrue(names["session-selected"]["pass"])
            self.assertTrue(names["timeline-loaded"]["pass"])

    def test_connect_failed_phase_rejects_disconnected_status(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "connect-failed",
                tree_kwargs={
                    "reconnect": True,
                    "selected": True,
                    "entries": 2,
                    "connection": "Disconnected · transport error: ConnectionClosed",
                },
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["connection-status-connect-failed"]["pass"])

    def test_disconnected_phase_rejects_connected_status(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "disconnected",
                tree_kwargs={
                    "reconnect": True,
                    "selected": True,
                    "entries": 2,
                    "connection": "Connected · pawork-test",
                },
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["connection-status-disconnected"]["pass"])

    def test_disconnected_phase_rejects_connecting_status(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "disconnected",
                tree_kwargs={
                    "reconnect": True,
                    "selected": True,
                    "entries": 2,
                    "connection": "Connecting…",
                },
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["connection-status-disconnected"]["pass"])

    def test_reconnected_phase_requires_reconnect_absent_and_restored_session(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "reconnected",
                tree_kwargs={"reconnect": False, "selected": True, "entries": 2},
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            self.assertTrue(payload["pass"])
            names = {check["name"]: check for check in payload["checks"]}
            self.assertTrue(names["reconnect-absent"]["pass"])
            self.assertTrue(names["session-selected"]["pass"])
            self.assertTrue(names["timeline-loaded"]["pass"])
            self.assertTrue(names["workspace-empty-hint-absent"]["pass"])
            self.assertTrue(names["connection-status-connected"]["pass"])

    def test_reconnected_phase_fails_when_reconnect_button_lingers(self):
        # Connected 后重连入口必须撤下（Connecting/Connected 均不发布）。
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "reconnected",
                tree_kwargs={"reconnect": True, "selected": True, "entries": 2},
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["reconnect-absent"]["pass"])

    def test_reconnected_phase_rejects_connecting_transient(self):
        # Connecting 瞬态满足 reconnect 缺席 + 条目保留；connection-status
        # 文案是区分两者的防线（审查 P3-2 加固）。
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "reconnected",
                tree_kwargs={
                    "selected": True,
                    "entries": 2,
                    "connection": "Connecting…",
                },
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["connection-status-connected"]["pass"])

    def test_reconnected_phase_rejects_missing_connection_status(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "reconnected",
                tree_kwargs={"selected": True, "entries": 2, "connection": None},
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["connection-status-connected"]["pass"])

    def test_new_phases_pin_1440_three_column_shell(self):
        frames = {
            "pawork-root": {"role": "AXGroup", "x": 100.0, "y": 200.0, "w": 1440.0, "h": 1024.0},
            "task-rail": {"role": "AXGroup", "x": 100.0, "y": 200.0, "w": 288.0, "h": 1024.0},
            "workspace": {"role": "AXGroup", "x": 388.0, "y": 200.0, "w": 712.0, "h": 1000.0},
            "inspector": {"role": "AXGroup", "x": 1100.0, "y": 200.0, "w": 440.0, "h": 1000.0},
            "composer": {"role": "AXGroup", "x": 388.0, "y": 1044.0, "w": 712.0, "h": 156.0},
            "status-bar": {"role": "AXGroup", "x": 100.0, "y": 1200.0, "w": 1440.0, "h": 24.0},
        }
        for phase in ("disconnected", "connect-failed", "reconnected"):
            spec = self.tools.PHASE_GEOMETRY[phase]
            self.assertEqual(spec["root"], (1440.0, 1024.0))
            self.assertEqual(spec["inspector"], "required")
            checks, _ = self.tools.geometry_checks(frames, phase)
            self.assertTrue(
                all(check["pass"] or not check.get("blocking", True) for check in checks),
                phase,
            )
            names = [check["name"] for check in checks]
            self.assertIn("root-1440x1024", names)
            self.assertIn("rail-width", names)
            self.assertIn("inspector-width", names)

    def test_assert_rejects_unknown_phase(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            frames = root / "geometry.txt"
            tree = root / "tree.txt"
            write_frames(frames)
            write_tree(tree)
            proc = subprocess.run(
                [
                    sys.executable, str(TOOLS), "assert",
                    "--frames", str(frames),
                    "--tree", str(tree),
                    "--phase", "bogus",
                    "--out", str(root / "assert.json"),
                ],
                check=False, capture_output=True, text=True,
            )
            self.assertEqual(proc.returncode, 2)
            self.assertIn("invalid choice", proc.stderr)

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

    def test_driver_passes_bash_syntax_check(self):
        proc = subprocess.run(
            ["bash", "-n", str(DRIVER)],
            check=False, capture_output=True, text=True,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)


if __name__ == "__main__":
    unittest.main()
