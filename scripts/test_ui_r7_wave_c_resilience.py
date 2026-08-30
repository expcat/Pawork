#!/usr/bin/env python3
"""R7 Wave C regressions for the derived dataset and new gate contracts."""

from __future__ import annotations

import json
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).parent
R7_TOOLS = SCRIPT_DIR / "ui-r7-wave-c-tools.py"
WAVE_D_TOOLS = SCRIPT_DIR / "ui-wave-d-tools.py"
DRIVER = SCRIPT_DIR / "ui-r7-wave-c-resilience.sh"


def create_fixture_db(path: Path) -> None:
    connection = sqlite3.connect(path)
    connection.executescript(
        """
        CREATE TABLE sessions (
            session_id TEXT PRIMARY KEY,
            title TEXT NOT NULL DEFAULT '',
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            archived INTEGER NOT NULL DEFAULT 0,
            active_branch TEXT NOT NULL DEFAULT 'main'
        );
        CREATE TABLE session_branches (
            session_id TEXT NOT NULL,
            branch_id TEXT NOT NULL,
            parent_branch_id TEXT,
            forked_from_event_id TEXT,
            head_sequence INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(session_id, branch_id)
        );
        CREATE TABLE session_events (
            event_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            branch_id TEXT NOT NULL,
            run_id TEXT,
            parent_event_id TEXT,
            sequence INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            schema_version INTEGER NOT NULL,
            timestamp_ms INTEGER NOT NULL,
            payload_json TEXT NOT NULL,
            UNIQUE(session_id, sequence)
        );
        """
    )
    connection.execute(
        "INSERT INTO sessions "
        "(session_id, title, created_at_ms, updated_at_ms, active_branch) "
        "VALUES ('fx-ses-beta-long', 'stress', 1, 64, 'main')"
    )
    connection.execute(
        "INSERT INTO session_branches (session_id, branch_id, head_sequence) "
        "VALUES ('fx-ses-beta-long', 'main', 64)"
    )
    for sequence in range(1, 65):
        connection.execute(
            "INSERT INTO session_events VALUES (?, 'fx-ses-beta-long', 'main', "
            "'run', NULL, ?, 'message_committed', 1, ?, '{}')",
            (f"base-{sequence}", sequence, sequence),
        )
    connection.commit()
    connection.close()


def frame_lines(
    width: int,
    height: int,
    inspector: bool,
    disconnected: bool = False,
    composer_height: int = 88,
    large_text: bool = False,
) -> str:
    root_x, root_y = 100, 50
    rail = 320 if large_text else 288 if width == 1440 else 240
    inspector_width = 440 if inspector else 0
    workspace_x = root_x + rail
    workspace_width = width - rail - inspector_width
    status_y = root_y + height - 24
    composer_y = status_y - composer_height
    lines = [
        f"id=pawork-root role=AXGroup x={root_x} y={root_y} w={width} h={height}",
        f"id=task-rail role=AXGroup x={root_x} y={root_y} w={rail} h={height}",
        f"id=workspace role=AXGroup x={workspace_x} y={root_y} w={workspace_width} h={height - 24}",
        f"id=timeline role=AXList x={workspace_x} y={root_y + 104} w={workspace_width} h={height - 216}",
        f"id=composer role=AXGroup x={workspace_x} y={composer_y} w={workspace_width} h={composer_height}",
        f"id=status-bar role=AXGroup x={workspace_x} y={status_y} w={workspace_width} h=24",
    ]
    if inspector:
        lines += [
            f"id=inspector role=AXGroup x={root_x + width - 440} y={root_y} w=440 h={height - 24}",
            f"id=inspector-tabs role=AXTabGroup x={root_x + width - 440} y={root_y} w=440 h=58",
        ]
    if disconnected:
        lines += [
            f"id=connection-status role=AXStaticText x={root_x + 20} y={root_y + 306} w=164 h=36",
            f"id=reconnect role=AXButton x={root_x + 20} y={root_y + 350} w=200 h=36",
            f"id=add-task role=AXButton x={root_x + 192} y={root_y + 310} w=28 h=28",
        ]
    return "\n".join(lines) + "\n"


def r7_tree(
    popover: bool = False, disconnected: bool = False, connected: bool = False
) -> str:
    lines = [
        'role=AXGroup identifier="pawork-root"',
        '  role=AXGroup identifier="task-rail"',
        '    role=AXList identifier="session-list"',
        '      role=AXRow identifier="session-fx-ses-alpha-today"',
        '      role=AXRow identifier="session-fx-ses-beta-long" selected=1',
    ]
    if disconnected:
        lines += [
            '    role=AXStaticText value="Disconnected · transport error: ConnectionClosed: '
            'peer closed the connection" identifier="connection-status"',
            '    role=AXButton identifier="reconnect" actions=[AXPress]',
        ]
    if connected:
        lines.append(
            '    role=AXStaticText value="Local · Connected · Up to date · 0" '
            'identifier="connection-status"'
        )
    lines += [
        '  role=AXGroup identifier="workspace"',
        '    role=AXList identifier="timeline"',
        '      role=AXRow value="R7C 千级列表末尾 🐾🧪" '
        'identifier="timeline-entry-evt-r7c-fx-ses-beta-long-1024"',
        '    role=AXGroup identifier="composer"',
        '      role=AXTextArea identifier="composer-input" focused=1',
        '    role=AXButton identifier="inspector-toggle"',
        '  role=AXGroup identifier="status-bar"',
    ]
    if popover:
        lines.append('    role=AXGroup identifier="activity-popover"')
    return "\n".join(lines) + "\n"


class R7WaveCResilienceTest(unittest.TestCase):
    def test_inflate_derives_exact_1024_rows_and_rejects_changed_base(self):
        with tempfile.TemporaryDirectory() as raw:
            db = Path(raw) / "session.db"
            out = Path(raw) / "dataset.json"
            create_fixture_db(db)
            command = [
                sys.executable,
                str(R7_TOOLS),
                "inflate",
                "--db",
                str(db),
                "--session-id",
                "fx-ses-beta-long",
                "--base-rows",
                "64",
                "--target-rows",
                "1024",
                "--out",
                str(out),
            ]
            proc = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            payload = json.loads(out.read_text("utf-8"))
            self.assertEqual(payload["inserted_message_rows"], 960)
            connection = sqlite3.connect(db)
            self.assertEqual(
                connection.execute(
                    "SELECT COUNT(*) FROM session_events WHERE session_id='fx-ses-beta-long'"
                ).fetchone()[0],
                1024,
            )
            self.assertEqual(
                connection.execute(
                    "SELECT head_sequence FROM session_branches "
                    "WHERE session_id='fx-ses-beta-long' AND branch_id='main'"
                ).fetchone()[0],
                1024,
            )
            last_payload = connection.execute(
                "SELECT payload_json FROM session_events "
                "WHERE session_id='fx-ses-beta-long' ORDER BY sequence DESC LIMIT 1"
            ).fetchone()[0]
            connection.close()
            self.assertIn("R7C 千级列表末尾 🐾🧪", last_payload)

            bad = subprocess.run(
                command[:-6]
                + ["--base-rows", "64", "--target-rows", "2048", "--out", str(out)],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(bad.returncode, 2)
            self.assertIn("base logical rows changed", bad.stderr)

    def test_1080x720_and_thousand_slice_contracts(self):
        with tempfile.TemporaryDirectory() as raw:
            raw_path = Path(raw)
            frames = raw_path / "frames.txt"
            tree = raw_path / "tree.txt"
            result = raw_path / "result.json"
            frames.write_text(frame_lines(1080, 720, False), "utf-8")
            tree.write_text(r7_tree(), "utf-8")
            phase = subprocess.run(
                [
                    sys.executable,
                    str(WAVE_D_TOOLS),
                    "assert",
                    "--frames",
                    str(frames),
                    "--tree",
                    str(tree),
                    "--phase",
                    "r7c-narrow",
                    "--out",
                    str(result),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(phase.returncode, 0, phase.stdout + phase.stderr)
            names = {item["name"]: item for item in json.loads(result.read_text("utf-8"))["checks"]}
            self.assertTrue(names["root-1080x720"]["pass"])
            self.assertTrue(names["focus-composer-retained"]["pass"])

            frames.write_text(
                frame_lines(
                    1080,
                    720,
                    False,
                    composer_height=104,
                    large_text=True,
                ),
                "utf-8",
            )
            zoom_phase = subprocess.run(
                [
                    sys.executable,
                    str(WAVE_D_TOOLS),
                    "assert",
                    "--frames",
                    str(frames),
                    "--tree",
                    str(tree),
                    "--phase",
                    "r7c-narrow-zoom",
                    "--out",
                    str(result),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(
                zoom_phase.returncode,
                0,
                zoom_phase.stdout + zoom_phase.stderr,
            )
            names = {
                item["name"]: item
                for item in json.loads(result.read_text("utf-8"))["checks"]
            }
            self.assertTrue(names["composer-height"]["pass"])
            self.assertTrue(names["rail-width"]["pass"])

            frames.write_text(frame_lines(1080, 720, False, disconnected=True), "utf-8")
            tree.write_text(r7_tree(disconnected=True), "utf-8")
            disconnected_phase = subprocess.run(
                [
                    sys.executable,
                    str(WAVE_D_TOOLS),
                    "assert",
                    "--frames",
                    str(frames),
                    "--tree",
                    str(tree),
                    "--phase",
                    "r7c-disconnected",
                    "--out",
                    str(result),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(
                disconnected_phase.returncode,
                0,
                disconnected_phase.stdout + disconnected_phase.stderr,
            )
            names = {
                item["name"]: item
                for item in json.loads(result.read_text("utf-8"))["checks"]
            }
            self.assertTrue(names["connection-status-within-rail"]["pass"])
            self.assertTrue(names["reconnect-within-rail"]["pass"])

            tree.write_text(r7_tree(connected=True), "utf-8")
            reconnected_phase = subprocess.run(
                [
                    sys.executable,
                    str(WAVE_D_TOOLS),
                    "assert",
                    "--frames",
                    str(frames),
                    "--tree",
                    str(tree),
                    "--phase",
                    "r7c-narrow-reconnected",
                    "--out",
                    str(result),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(
                reconnected_phase.returncode,
                0,
                reconnected_phase.stdout + reconnected_phase.stderr,
            )
            names = {
                item["name"]: item
                for item in json.loads(result.read_text("utf-8"))["checks"]
            }
            self.assertTrue(names["reconnect-absent"]["pass"])
            self.assertTrue(names["connection-status-connected"]["pass"])

            (raw_path / "timeline_stable").write_text(
                json.dumps(
                    {"entry_count": 1024, "session_id": "fx-ses-beta-long"}
                ),
                "utf-8",
            )

            state = subprocess.run(
                [
                    sys.executable,
                    str(WAVE_D_TOOLS),
                    "states-assert",
                    "--tree",
                    str(tree),
                    "--phase",
                    "virtualized-thousand",
                    "--logical-entries",
                    "1024",
                    "--barrier",
                    str(raw_path / "timeline_stable"),
                    "--out",
                    str(result),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(state.returncode, 0, state.stdout + state.stderr)
            checks = {item["name"]: item for item in json.loads(result.read_text("utf-8"))["checks"]}
            self.assertTrue(checks["virtualization-thousand-window-slice"]["pass"])
            self.assertTrue(checks["virtualization-cjk-emoji-long-row"]["pass"])

            (raw_path / "timeline_stable").write_text(
                json.dumps({"entry_count": 64, "session_id": "fx-ses-beta-long"}),
                "utf-8",
            )
            bad_barrier = subprocess.run(
                [
                    sys.executable,
                    str(WAVE_D_TOOLS),
                    "states-assert",
                    "--tree",
                    str(tree),
                    "--phase",
                    "virtualized-thousand",
                    "--logical-entries",
                    "1024",
                    "--barrier",
                    str(raw_path / "timeline_stable"),
                    "--out",
                    str(result),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(
                bad_barrier.returncode,
                5,
                bad_barrier.stdout + bad_barrier.stderr,
            )

    def test_driver_keeps_fixture_mutation_temporary_and_records_all_gates(self):
        text = DRIVER.read_text("utf-8")
        self.assertIn("$ROOT/data/session.db", text)
        self.assertNotIn('inflate --db "$REPO_ROOT', text)
        for token in (
            "1080x720",
            "virtualized-thousand",
            "scroll-y",
            "RESIZE_CYCLES",
            "platform-preferences.json",
            "performance-baseline.json",
            "r7c-disconnected",
            "paint-assert",
            "Text size · 150%",
            "r7c-narrow-zoom",
        ):
            self.assertIn(token, text)

    def test_paint_assert_rejects_connection_text_in_add_task_gap(self):
        from PIL import Image

        with tempfile.TemporaryDirectory() as raw:
            raw_path = Path(raw)
            geometry = raw_path / "geometry.txt"
            screenshot = raw_path / "screenshot.png"
            result = raw_path / "result.json"
            geometry.write_text(
                frame_lines(1080, 720, False, disconnected=True), "utf-8"
            )
            image = Image.new("RGB", (1080, 720), (32, 32, 32))
            for x in range(204, 209):
                image.putpixel((x, 324), (220, 220, 220))
            image.save(screenshot)
            command = [
                sys.executable,
                str(R7_TOOLS),
                "paint-assert",
                "--screenshot",
                str(screenshot),
                "--geometry",
                str(geometry),
                "--out",
                str(result),
            ]
            good = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertEqual(good.returncode, 0, good.stdout + good.stderr)
            self.assertTrue(json.loads(result.read_text("utf-8"))["pass"])

            image.putpixel((188, 324), (220, 220, 220))
            image.save(screenshot)
            bad = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertEqual(bad.returncode, 5, bad.stdout + bad.stderr)
            self.assertFalse(json.loads(result.read_text("utf-8"))["pass"])

if __name__ == "__main__":
    unittest.main()
