#!/usr/bin/env python3
"""Focused regressions for scripts/ui-wave-d-tools.py."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).with_name("ui-wave-d-tools.py")
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
                        "id=task-rail role=AXGroup x=100.0 y=200.0 w=288.0 h=1024.0",
                        "id=workspace role=AXGroup x=388.0 y=200.0 w=712.0 h=1000.0",
                        "id=inspector role=AXGroup x=1100.0 y=200.0 w=440.0 h=1000.0",
                        "id=composer role=AXGroup x=388.0 y=1100.0 w=712.0 h=92.0",
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


if __name__ == "__main__":
    unittest.main()
