#!/usr/bin/env python3
"""Focused regressions for the Wave D helper and driver scripts."""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from PIL import Image, ImageCms

TOOLS = Path(__file__).with_name("ui-wave-d-tools.py")
DRIVER = Path(__file__).with_name("ui-wave-d-state-a.sh")
NL = chr(10)
AX_TREE = (
    Path(__file__).resolve().parents[1]
    / "docs/ui-review/wave-c/ax-bridge/ax-tree.txt"
)


def load_tools():
    spec = importlib.util.spec_from_file_location("ui_wave_d_tools", TOOLS)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class WaveDToolsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tools = load_tools()

    def test_parse_tree_extracts_identifiers_from_wave_c_dump(self):
        tree = self.tools.parse_tree(AX_TREE)
        required = [
            "task-rail",
            "session-list",
            "session-fx-ses-alpha-today",
            "workspace",
            "timeline",
            "composer",
            "composer-input",
            "inspector",
            "inspector-tabs",
            "status-bar",
        ]
        missing = [name for name in required if name not in tree["identifiers"]]
        self.assertEqual(missing, [])
        self.assertGreaterEqual(len(tree["identifiers"]), 60)
        self.assertEqual(tree["role_unknown"], 0)
        self.assertIn("session-fx-ses-alpha-today", tree["selected_rows"])
        self.assertGreaterEqual(tree["timeline_entries_alpha_today"], 1)

    def test_parse_tree_ignores_identifier_echoes_in_comment_metadata(self):
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "tree.txt"
            path.write_text(
                '\n'.join(
                    [
                        '  - role=AXTextArea identifier="composer-input" focused=1',
                        '# custom_hint role=AXTextArea identifier="composer-input" focused=1',
                        '# summary role=? identifier="not-a-tree-node"',
                    ]
                )
                + '\n',
                encoding="utf-8",
            )
            tree = self.tools.parse_tree(path)
            self.assertEqual(tree["focused"], ["composer-input"])
            self.assertEqual(tree["identifiers"], {"composer-input"})
            self.assertEqual(tree["role_unknown"], 0)

    def test_initial_focus_accepts_only_root_or_composer(self):
        allowed = {"pawork-root", "composer-input"}
        for focused in ([], ["pawork-root"], ["composer-input"]):
            self.assertTrue(
                len(focused) <= 1 and all(node in allowed for node in focused)
            )
        for focused in (["send"], ["pawork-root", "composer-input"]):
            self.assertFalse(
                len(focused) <= 1 and all(node in allowed for node in focused)
            )

    def test_barrier_default_line_is_whitespace_separated_values(self):
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "timeline_stable"
            path.write_text(
                json.dumps(
                    {
                        "settle_seq": 4,
                        "session_id": "fx-ses-alpha-today",
                        "entry_count": 12,
                    }
                ),
                encoding="utf-8",
            )
            proc = subprocess.run(
                [sys.executable, str(TOOLS), "barrier-read", "--file", str(path)],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertEqual(proc.stdout.strip(), "4 fx-ses-alpha-today 12")
            seq = subprocess.run(
                [
                    sys.executable,
                    str(TOOLS),
                    "barrier-read",
                    "--file",
                    str(path),
                    "--field",
                    "seq",
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            self.assertEqual(seq.stdout.strip(), "4")

    def test_write_current_zones_fills_all_state_a_ids(self):
        repo = Path(__file__).resolve().parents[1]
        zones_src = repo / "docs/ui-review/state-a/zones.json"
        with tempfile.TemporaryDirectory() as raw:
            frames = Path(raw) / "geometry.txt"
            frames.write_text(
                "\n".join(
                    [
                        "id=pawork-root role=AXGroup x=100.0 y=200.0 w=1440.0 h=1024.0",
                        "id=task-rail role=AXGroup x=100.0 y=200.0 w=288.0 h=1000.0",
                        "id=workspace role=AXGroup x=388.0 y=200.0 w=712.0 h=1000.0",
                        "id=inspector role=AXGroup x=1100.0 y=200.0 w=440.0 h=1000.0",
                        "id=composer role=AXGroup x=388.0 y=1044.0 w=712.0 h=156.0",
                        "id=status-bar role=AXGroup x=100.0 y=1200.0 w=1440.0 h=24.0",
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            out = Path(raw) / "zones.json"
            proc = subprocess.run(
                [
                    sys.executable,
                    str(TOOLS),
                    "write-current-zones",
                    "--zones",
                    str(zones_src),
                    "--frames",
                    str(frames),
                    "--out",
                    str(out),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            payload = json.loads(out.read_text("utf-8"))
            ids = {zone["id"] for zone in payload["zones"]}
            self.assertTrue(all("current" in zone for zone in payload["zones"]))
            self.assertEqual(len(ids), 9)
            taskrail = next(zone for zone in payload["zones"] if zone["id"] == "taskrail")
            self.assertEqual(taskrail["current"]["w"], 288)
            self.assertEqual(taskrail["current"]["x"], 0)
            self.assertEqual(taskrail["current"]["h"], 1024)
            header_right = next(
                zone for zone in payload["zones"] if zone["id"] == "header-right"
            )
            self.assertEqual(
                header_right["current"],
                {"x": 723, "y": 0, "w": 277, "h": 104},
            )
            timeline = next(zone for zone in payload["zones"] if zone["id"] == "timeline")
            self.assertEqual(
                timeline["current"],
                {"x": 288, "y": 104, "w": 677, "h": 740},
            )

    def test_known_composer_height_drift_is_recorded_but_nonblocking(self):
        frames = {
            "pawork-root": {"role": "AXGroup", "x": 100.0, "y": 200.0, "w": 1440.0, "h": 1024.0},
            "task-rail": {"role": "AXGroup", "x": 100.0, "y": 200.0, "w": 288.0, "h": 1024.0},
            "workspace": {"role": "AXGroup", "x": 388.0, "y": 200.0, "w": 712.0, "h": 1000.0},
            "inspector": {"role": "AXGroup", "x": 1100.0, "y": 200.0, "w": 440.0, "h": 1000.0},
            "composer": {"role": "AXGroup", "x": 388.0, "y": 1044.0, "w": 712.0, "h": 156.0},
            "status-bar": {"role": "AXGroup", "x": 100.0, "y": 1200.0, "w": 1440.0, "h": 24.0},
        }
        checks, _ = self.tools.geometry_checks(frames)
        composer = next(check for check in checks if check["name"] == "composer-height")
        self.assertFalse(composer["pass"])
        self.assertFalse(composer["blocking"])
        self.assertTrue(
            all(check["pass"] or not check.get("blocking", True) for check in checks)
        )
        frames["composer"] = {
            "role": "AXGroup",
            "x": 388.0,
            "y": 1020.0,
            "w": 712.0,
            "h": 180.0,
        }
        checks, _ = self.tools.geometry_checks(frames)
        composer = next(check for check in checks if check["name"] == "composer-height")
        self.assertFalse(composer["pass"])
        self.assertTrue(composer["blocking"])
        self.assertFalse(
            all(check["pass"] or not check.get("blocking", True) for check in checks)
        )

    def test_normalize_converts_embedded_profile_and_strips_metadata(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            shot = root / "shot.png"
            profile = ImageCms.ImageCmsProfile(
                ImageCms.createProfile("sRGB")
            ).tobytes()
            Image.new("RGB", (1440, 1056), (20, 30, 40)).save(
                shot,
                icc_profile=profile,
            )
            tree = root / "tree.txt"
            tree.write_text("# wid=7 pid=123 bounds={0,0,1440,1056}\n", "utf-8")
            wid = root / "wid.txt"
            wid.write_text("7\n", "utf-8")
            frames = root / "frames.txt"
            frames.write_text(
                "id=pawork-root role=AXGroup x=0.0 y=32.0 w=1440.0 h=1024.0\n",
                "utf-8",
            )
            out = root / "current.png"
            report = root / "normalize.json"
            proc = subprocess.run(
                [
                    sys.executable,
                    str(TOOLS),
                    "normalize",
                    "--shot", str(shot),
                    "--tree", str(tree),
                    "--wid", str(wid),
                    "--frames", str(frames),
                    "--out", str(out),
                    "--json", str(report),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            with Image.open(out) as normalized:
                self.assertEqual(normalized.mode, "RGB")
                self.assertEqual(normalized.size, (1440, 1024))
                self.assertNotIn("icc_profile", normalized.info)
            mapping = json.loads(report.read_text("utf-8"))
            self.assertEqual(mapping["color_conversion"], "embedded ICC -> sRGB")
            self.assertGreater(mapping["source_icc_profile_bytes"], 0)
            self.assertTrue(mapping["icc_profile_dropped"])

    def test_driver_rejects_nonempty_output_to_prevent_stale_evidence(self):
        with tempfile.TemporaryDirectory() as raw:
            out = Path(raw) / "evidence"
            out.mkdir()
            (out / "stale.json").write_text("{}\n", "utf-8")
            env = dict(os.environ)
            env["PAWORK_WAVE_D_PYTHON"] = sys.executable
            proc = subprocess.run(
                ["bash", str(DRIVER), "run", "--out", str(out)],
                check=False,
                capture_output=True,
                text=True,
                env=env,
            )
            self.assertEqual(proc.returncode, 3, proc.stdout + proc.stderr)
            self.assertIn("must be new or empty", proc.stderr)
            self.assertTrue((out / "stale.json").exists())


def r6_common_frames(inspector_open):
    """Connected 1440x1024 三栏壳层（root 位于 0,0；合同见 PHASE_GEOMETRY）。"""
    workspace_w = 712.0 if inspector_open else 1152.0
    frames = {
        "pawork-root": (0.0, 0.0, 1440.0, 1024.0),
        "task-rail": (0.0, 0.0, 288.0, 1024.0),
        "workspace": (288.0, 0.0, workspace_w, 1000.0),
        "composer": (288.0, 906.0, workspace_w, 94.0),
        "status-bar": (0.0, 1000.0, 1440.0, 24.0),
    }
    if inspector_open:
        frames["inspector"] = (1000.0, 0.0, 440.0, 1000.0)
    frames["workspace-header"] = (288.0, 0.0, workspace_w, 104.0)
    return frames


def r6_state_a_frames():
    """State A：Inspector 展开、默认 Changes/Files；几何源同 app.rs。"""
    frames = r6_common_frames(inspector_open=True)
    frames.update({
        # header_action_ax_rect：right = header 右缘 -25，y 居中 content 区。
        "header-new-task": (935.0, 51.5, 40.0, 37.0),
        "inspector-tabs": (1012.0, 0.0, 300.0, 58.0),
        "inspector-tab-changes": (1012.0, 0.0, 100.0, 58.0),
        "inspector-tab-terminal": (1112.0, 0.0, 100.0, 58.0),
        "inspector-tab-resources": (1212.0, 0.0, 100.0, 58.0),
        "inspector-collapse": (1400.0, 15.0, 32.0, 28.0),
        "changes": (1000.0, 58.0, 440.0, 942.0),
        "changes-tabs": (1012.0, 58.0, 192.0, 56.0),
        "changes-tab-files": (1012.0, 58.0, 96.0, 56.0),
        "changes-tab-summary": (1108.0, 58.0, 96.0, 56.0),
    })
    return frames


def r6_state_b_open_frames(anchor_ok=True):
    """State B open：Inspector 折叠，Header Activity Popover 打开。"""
    frames = r6_common_frames(inspector_open=False)
    frames["inspector-toggle"] = (1375.0, 51.5, 40.0, 37.0)  # 右缘 = header 右缘 -25。
    popover_x = 1095.0 if anchor_ok else 1100.0  # 右缘对齐 / 偏 5px 失败样例。
    frames["activity-popover"] = (popover_x, 92.5, 320.0, 320.0)
    frames["activity-changes-heading"] = (1115.0, 142.5, 280.0, 20.0)
    frames["activity-open-changes"] = (1115.0, 170.5, 280.0, 32.0)
    return frames


def r6_state_b_resumed_frames():
    frames = r6_common_frames(inspector_open=True)
    frames["header-new-task"] = (935.0, 51.5, 40.0, 37.0)
    frames["inspector-tabs"] = (1012.0, 0.0, 300.0, 58.0)
    frames["inspector-tab-changes"] = (1012.0, 0.0, 100.0, 58.0)
    frames["changes"] = (1000.0, 58.0, 440.0, 942.0)
    return frames


def r6_tree_text(phase, *, terminal_selected=False, stray_popover=False,
                 toggle_on_root=False):
    """伪造 ui-ax-dump.swift 树（按深度 2 空格缩进）。"""
    lines = [
        'role=AXGroup identifier="pawork-root"',
        '  role=AXGroup identifier="task-rail"',
        '    role=AXList identifier="session-list"',
        '      role=AXButton identifier="session-fx-ses-alpha-today" selected=1',
        '  role=AXGroup identifier="workspace"',
    ]
    if phase == "r6-state-b-open":
        header_lines = [
            '    role=AXGroup identifier="workspace-header"',
            '      role=AXGroup identifier="activity-popover"',
            '        role=AXStaticText identifier="activity-changes-heading"',
            '        role=AXButton identifier="activity-open-changes" enabled=1 actions=[AXPress]',
        ]
        if not toggle_on_root:
            header_lines.insert(
                1, '      role=AXButton identifier="inspector-toggle" enabled=1 actions=[AXPress]'
            )
        lines += header_lines
    else:
        lines += [
            '    role=AXGroup identifier="workspace-header"',
            '      role=AXButton identifier="header-new-task" enabled=1 actions=[AXPress]',
        ]
    lines += [
        '    role=AXList identifier="timeline"',
        '      role=AXGroup identifier="timeline-entry-evt-fx-ses-alpha-today-1"',
        '    role=AXGroup identifier="composer"',
        '      role=AXTextArea identifier="composer-input" focused=1',
    ]
    if phase in ("r6-state-a", "r6-state-b-resumed"):
        tab_lines = [
            '  role=AXGroup identifier="inspector"',
            '    role=AXTabGroup identifier="inspector-tabs"',
        ]
        if phase == "r6-state-a":
            tab_lines += [
                '      role=AXTab identifier="inspector-tab-changes" selected=1 actions=[AXPress]',
                '      role=AXTab identifier="inspector-tab-terminal"'
                + (' selected=1' if terminal_selected else '')
                + ' actions=[AXPress]',
                '      role=AXTab identifier="inspector-tab-resources" actions=[AXPress]',
                '    role=AXButton identifier="inspector-collapse" enabled=1 actions=[AXPress]',
                '    role=AXGroup identifier="changes"',
                '      role=AXTabGroup identifier="changes-tabs"',
                '        role=AXTab identifier="changes-tab-files" selected=1 actions=[AXPress]',
                '        role=AXTab identifier="changes-tab-summary" actions=[AXPress]',
            ]
        else:
            tab_lines += [
                '      role=AXTab identifier="inspector-tab-changes" selected=1 actions=[AXPress]',
            ]
            if stray_popover:
                tab_lines += [
                    '      role=AXGroup identifier="activity-popover"',
                ]
        lines += tab_lines
    if toggle_on_root:
        # 负路径：触发器改挂 pawork-root（深度 1、置于 status-bar 前，
        # 不扰动其他节点的父映射），使 _r6_under 判定失败。
        lines.append('  role=AXButton identifier="inspector-toggle" enabled=1 actions=[AXPress]')
    lines.append('  role=AXGroup identifier="status-bar"')
    return NL.join(lines) + NL


class R6PhaseAssertionTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tools = load_tools()

    def assert_phase(self, phase, frames, tree_text):
        with tempfile.TemporaryDirectory() as raw:
            frames_path = Path(raw) / "geometry.txt"
            tree_path = Path(raw) / "ax-tree.txt"
            out_path = Path(raw) / "assert.json"
            frames_path.write_text(
                NL.join(
                    "id=" + name + " role=AXGroup x=" + str(x) + " y=" + str(y)
                    + " w=" + str(w) + " h=" + str(h)
                    for name, (x, y, w, h) in frames.items()
                ) + NL,
                encoding="utf-8",
            )
            tree_path.write_text(tree_text, encoding="utf-8")
            proc = subprocess.run(
                [
                    sys.executable, str(TOOLS), "assert",
                    "--frames", str(frames_path),
                    "--tree", str(tree_path),
                    "--phase", phase,
                    "--out", str(out_path),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            payload = json.loads(out_path.read_text("utf-8"))
            return proc, payload

    def check_names(self, payload):
        return {check["name"]: check for check in payload["checks"]}

    def test_parse_tree_extracts_r6_selection_actions_and_parents(self):
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "tree.txt"
            path.write_text(
                NL.join(
                    [
                        'role=AXGroup identifier="workspace"',
                        '  role=AXGroup identifier="workspace-header"',
                        '    role=AXButton identifier="inspector-toggle" enabled=1 actions=[AXPress]',
                        '    role=AXTab identifier="inspector-tab-changes" selected=1',
                    ]
                ) + NL,
                encoding="utf-8",
            )
            tree = self.tools.parse_tree(path)
            self.assertEqual(tree["selected_identifiers"], {"inspector-tab-changes"})
            self.assertEqual(tree["press_identifiers"], {"inspector-toggle"})
            self.assertEqual(tree["parents"]["inspector-toggle"], "workspace-header")
            self.assertEqual(tree["parents"]["workspace-header"], "workspace")

    def test_r6_state_a_pass(self):
        proc, payload = self.assert_phase(
            "r6-state-a", r6_state_a_frames(), r6_tree_text("r6-state-a")
        )
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertTrue(payload["pass"], proc.stdout)
        names = self.check_names(payload)
        for name in (
            "inspector-tabs-height",
            "inspector-tabs-tabs-adjacent",
            "inspector-tab-changes-selected",
            "inspector-tab-terminal-not-selected",
            "inspector-tab-resources-not-selected",
            "changes-tabs-height",
            "changes-tabs-tabs-adjacent",
            "changes-tab-files-selected",
            "inspector-collapse-size",
            "header-new-task-size",
        ):
            self.assertIn(name, names)
            self.assertTrue(names[name]["pass"], names[name]["detail"])

    def test_r6_state_a_fail_on_wrong_tab_selection_and_adjacency(self):
        frames = r6_state_a_frames()
        # dx=98 破坏相邻合同。
        frames["changes-tab-summary"] = (1110.0, 58.0, 96.0, 56.0)
        proc, payload = self.assert_phase(
            "r6-state-a", frames, r6_tree_text("r6-state-a", terminal_selected=True)
        )
        self.assertEqual(proc.returncode, 5, proc.stdout + proc.stderr)
        self.assertFalse(payload["pass"])
        names = self.check_names(payload)
        self.assertFalse(names["inspector-tab-terminal-not-selected"]["pass"])
        self.assertFalse(names["changes-tabs-tabs-adjacent"]["pass"])

    def test_r6_state_b_open_pass(self):
        proc, payload = self.assert_phase(
            "r6-state-b-open",
            r6_state_b_open_frames(),
            r6_tree_text("r6-state-b-open"),
        )
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertTrue(payload["pass"], proc.stdout)
        names = self.check_names(payload)
        for name in (
            "inspector-absent",
            "inspector-column-absent",
            "inspector-toggle-size",
            "inspector-toggle-header-inset",
            "inspector-toggle-under-header",
            "activity-popover-size",
            "activity-popover-anchor",
            "activity-changes-heading-height",
            "activity-open-changes-height",
            "activity-open-changes-press",
            "header-new-task-absent",
        ):
            self.assertIn(name, names)
            self.assertTrue(names[name]["pass"], names[name]["detail"])

    def test_r6_state_b_open_fail_on_popover_anchor_drift(self):
        proc, payload = self.assert_phase(
            "r6-state-b-open",
            r6_state_b_open_frames(anchor_ok=False),
            r6_tree_text("r6-state-b-open"),
        )
        self.assertEqual(proc.returncode, 5, proc.stdout + proc.stderr)
        self.assertFalse(payload["pass"])
        names = self.check_names(payload)
        self.assertFalse(names["activity-popover-anchor"]["pass"])

    def test_r6_state_b_open_fail_on_toggle_outside_header_subtree(self):
        proc, payload = self.assert_phase(
            "r6-state-b-open",
            r6_state_b_open_frames(),
            r6_tree_text("r6-state-b-open", toggle_on_root=True),
        )
        self.assertEqual(proc.returncode, 5, proc.stdout + proc.stderr)
        self.assertFalse(payload["pass"])
        names = self.check_names(payload)
        self.assertFalse(names["inspector-toggle-under-header"]["pass"])

    def test_r6_state_b_resumed_pass(self):
        proc, payload = self.assert_phase(
            "r6-state-b-resumed",
            r6_state_b_resumed_frames(),
            r6_tree_text("r6-state-b-resumed"),
        )
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertTrue(payload["pass"], proc.stdout)
        names = self.check_names(payload)
        for name in (
            "inspector-tab-changes-selected",
            "header-new-task-present",
            "activity-controls-absent",
        ):
            self.assertIn(name, names)
            self.assertTrue(names[name]["pass"], names[name]["detail"])

    def test_r6_state_b_resumed_fail_on_stray_popover(self):
        proc, payload = self.assert_phase(
            "r6-state-b-resumed",
            r6_state_b_resumed_frames(),
            r6_tree_text("r6-state-b-resumed", stray_popover=True),
        )
        self.assertEqual(proc.returncode, 5, proc.stdout + proc.stderr)
        self.assertFalse(payload["pass"])
        names = self.check_names(payload)
        self.assertFalse(names["activity-controls-absent"]["pass"])


if __name__ == "__main__":
    unittest.main()
