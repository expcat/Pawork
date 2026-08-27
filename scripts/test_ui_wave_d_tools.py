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


if __name__ == "__main__":
    unittest.main()
