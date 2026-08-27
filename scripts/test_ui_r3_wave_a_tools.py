#!/usr/bin/env python3
"""R3 Wave A regressions: State C Projects-grouping phase + driver guards."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).with_name("ui-wave-d-tools.py")
DRIVER = Path(__file__).with_name("ui-r3-wave-a-projects.sh")


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
    grouping="Projects",
    project_groups=("fx-alpha-app", "fx-beta-lib"),
    date_groups=(),
    menu_open=False,
    selected=True,
    entries=2,
    empty_hint=False,
):
    lines = [
        'role=AXGroup identifier="pawork-root"',
        'role=AXGroup identifier="task-rail"',
    ]
    if grouping is None:
        lines.append('role=AXButton identifier="task-rail-grouping"')
    else:
        lines.append(
            'role=AXButton value="' + grouping
            + '" identifier="task-rail-grouping"'
        )
    lines += [
        'role=AXButton value="All projects" identifier="project-scope"',
        'role=AXStaticText value="Local · Connected · Up to date · 0" identifier="connection-status"',
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
    for key in project_groups:
        lines.append(
            'role=AXButton value="3 tasks" identifier="project-' + key + '"'
        )
        lines.append('role=AXButton identifier="project-add-' + key + '"')
    for label in date_groups:
        lines.append('role=AXStaticText identifier="date-group-' + label + '"')
    if menu_open:
        lines.append('role=AXGroup identifier="grouping-menu"')
        lines.append('role=AXButton identifier="group-projects" selected=1')
    for index in range(entries):
        lines.append(
            'role=AXGroup identifier="timeline-entry-evt-fx-ses-alpha-today-'
            + str(index) + '"'
        )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def run_assert(raw, phase="projects", frames_kwargs=None, tree_kwargs=None):
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


class R3WaveAProjectsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tools = load_tools()

    def test_parse_tree_extracts_grouping_value(self):
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "tree.txt"
            write_tree(path, grouping="Projects")
            tree = self.tools.parse_tree(path)
            self.assertEqual(tree["grouping_value"], "Projects")
            path.write_text(
                'role=AXButton identifier="task-rail-grouping"\n',
                encoding="utf-8",
            )
            tree = self.tools.parse_tree(path)
            self.assertEqual(tree["grouping_value"], "")

    def test_projects_phase_passes_with_grouping_switched_rail(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(raw)
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            self.assertTrue(payload["pass"])
            names = {check["name"]: check for check in payload["checks"]}
            self.assertTrue(names["grouping-projects-selected"]["pass"])
            self.assertTrue(names["grouping-menu-closed"]["pass"])
            self.assertTrue(names["date-groups-absent"]["pass"])
            self.assertTrue(names["project-groups-present"]["pass"])
            self.assertTrue(names["workspace-empty-hint-absent"]["pass"])
            self.assertTrue(names["session-selected"]["pass"])
            self.assertTrue(names["timeline-loaded"]["pass"])
            self.assertTrue(names["shell-skeleton"]["pass"])

    def test_projects_phase_fails_when_grouping_value_stays_timeline(self):
        # 分组切换未生效：值仍 Timeline 时相位必须失败（防 press 假成功）。
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(raw, tree_kwargs={"grouping": "Timeline"})
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["grouping-projects-selected"]["pass"])

    def test_projects_phase_fails_when_grouping_value_missing(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(raw, tree_kwargs={"grouping": None})
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["grouping-projects-selected"]["pass"])

    def test_projects_phase_fails_when_date_groups_linger(self):
        # Projects 态不得残留日期桶头。
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw, tree_kwargs={"date_groups": ("Today", "Yesterday")}
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["date-groups-absent"]["pass"])

    def test_projects_phase_fails_without_fixture_project_groups(self):
        # fixture 种子只有 alpha/beta 两组（gamma 无会话）；缺任一即失败。
        for groups in ((), ("fx-alpha-app",)):
            with tempfile.TemporaryDirectory() as raw:
                proc, payload = run_assert(
                    raw, tree_kwargs={"project_groups": groups}
                )
                self.assertEqual(proc.returncode, 5)
                names = {check["name"]: check for check in payload["checks"]}
                self.assertFalse(names["project-groups-present"]["pass"])

    def test_projects_phase_fails_when_grouping_menu_stays_open(self):
        # 选择提交后浮层必须收起；菜单残留即回归。
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(raw, tree_kwargs={"menu_open": True})
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["grouping-menu-closed"]["pass"])

    def test_projects_phase_requires_session_and_timeline_retained(self):
        # 分组切换是 rail 本地状态：选中会话与时间线不得丢失。
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(raw, tree_kwargs={"entries": 0})
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["timeline-loaded"]["pass"])
            proc, payload = run_assert(raw, tree_kwargs={"selected": False})
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["session-selected"]["pass"])

    def test_projects_phase_fails_when_empty_hint_lingers(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(raw, tree_kwargs={"empty_hint": True})
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["workspace-empty-hint-absent"]["pass"])

    def test_projects_phase_pins_1440_three_column_shell(self):
        spec = self.tools.PHASE_GEOMETRY["projects"]
        self.assertEqual(spec["root"], (1440.0, 1024.0))
        self.assertEqual(spec["rail"], (288.0, 4.32))
        self.assertEqual(spec["inspector"], "required")
        with tempfile.TemporaryDirectory() as raw:
            frames = Path(raw) / "geometry.txt"
            write_frames(frames)
            parsed = self.tools.parse_frames(frames)
            checks, _ = self.tools.geometry_checks(parsed, "projects")
            self.assertTrue(
                all(check["pass"] or not check.get("blocking", True) for check in checks)
            )
            names = [check["name"] for check in checks]
            self.assertIn("root-1440x1024", names)
            self.assertIn("rail-width", names)
            self.assertIn("inspector-width", names)

    def test_project_groups_ignore_scope_button_and_add_buttons(self):
        # project-scope / project-add-* 是控件不是分组头，不得混入分组断言。
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw, tree_kwargs={"project_groups": ("fx-alpha-app",)}
            )
            names = {check["name"]: check for check in payload["checks"]}
            detail = names["project-groups-present"]["detail"]
            self.assertIn("missing: project-fx-beta-lib", detail)
            self.assertEqual(proc.returncode, 5)

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
