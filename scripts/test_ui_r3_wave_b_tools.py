#!/usr/bin/env python3
"""R3 Wave B Slice 3 regressions: nav phase assertions + driver guards."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).with_name("ui-r3-wave-b-tools.py")
DRIVER = Path(__file__).with_name("ui-r3-wave-b-nav.sh")


def load_tools():
    spec = importlib.util.spec_from_file_location("ui_r3_wave_b_tools", TOOLS)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def row(identifier, help_text="Session", selected=False, focused=False):
    line = 'role=AXRow identifier="' + identifier + '"'
    if help_text is not None:
        line += ' help="' + help_text + '"'
    if focused:
        line += " focused=1"
    if selected:
        line += " selected=1"
    return line + " enabled=1 actions=[AXPress]"


def button(identifier, focused=False, help_text=None):
    line = 'role=AXButton identifier="' + identifier + '"'
    if help_text is not None:
        line += ' help="' + help_text + '"'
    if focused:
        line += " focused=1"
    return line


def write_tree(path, **overrides):
    """Synthetic r3b tree; overrides replace keyed lines."""
    selected = overrides.get("selected", "session-fx-ses-beta-pending")
    focused_button = overrides.get("focused_button")
    rows = overrides.get("rows")
    if rows is None:
        hide_alpha = overrides.get("alpha_collapsed", False)
        rows = [
            *(
                []
                if hide_alpha
                else [
                    row("session-fx-ses-alpha-today"),
                    row("session-fx-ses-alpha-yesterday"),
                    row("session-fx-ses-alpha-longtitle"),
                ]
            ),
            row(
                "session-fx-ses-beta-pending",
                help_text=overrides.get("pending_help", "Needs input"),
                selected=selected == "session-fx-ses-beta-pending",
            ),
            row(
                "session-fx-ses-beta-toolfailed",
                selected=selected == "session-fx-ses-beta-toolfailed",
            ),
            row("session-fx-ses-beta-long", selected=selected == "session-fx-ses-beta-long"),
            row(
                "session-fx-ses-beta-cancelled",
                selected=selected == "session-fx-ses-beta-cancelled",
            ),
        ]
    lines = [
        'role=AXGroup identifier="pawork-root"',
        'role=AXGroup identifier="task-rail"',
        'role=AXButton value="'
        + overrides.get("grouping", "Timeline")
        + '" identifier="task-rail-grouping"'
        + (" focused=1" if focused_button == "task-rail-grouping" else ""),
        'role=AXButton identifier="add-task"',
        'role=AXGroup identifier="session-list"',
    ]
    lines.extend(rows)
    lines.append('role=AXGroup identifier="workspace"')
    if overrides.get("empty_hint"):
        lines.append('role=AXStaticText identifier="workspace-empty-hint"')
    for index in range(overrides.get("entries", 2)):
        lines.append(
            'role=AXRow identifier="timeline-entry-evt-'
            + overrides.get("entry_session", "fx-ses-beta-pending")
            + "-"
            + str(index)
            + '"'
        )
    lines.extend(
        [
            'role=AXTextArea identifier="composer-input"'
            + (" focused=1" if overrides.get("composer_focused", False) else ""),
            'role=AXStaticText value="'
            + overrides.get("connection", "Local · Connected")
            + '" identifier="connection-status"',
        ]
    )
    if overrides.get("reconnect_button"):
        lines.append('role=AXButton identifier="reconnect"')
    menu_focused = overrides.get("menu_focused")
    if overrides.get("menu_open"):
        lines.extend(
            [
                'role=AXGroup identifier="grouping-menu"',
                button("group-timeline", focused=menu_focused == "group-timeline"),
                button("group-projects", focused=menu_focused == "group-projects"),
            ]
        )
    if overrides.get("scope_menu"):
        lines.extend(
            [
                'role=AXGroup identifier="scope-menu"',
                button("scope-all", focused=menu_focused == "scope-all"),
                button("scope-fx-alpha-app", focused=menu_focused == "scope-fx-alpha-app"),
                button("scope-fx-beta-lib", focused=menu_focused == "scope-fx-beta-lib"),
            ]
        )
    if overrides.get("workspace_confirm"):
        lines.extend(
            [
                'role=AXGroup identifier="workspace-confirm"',
                button(
                    "workspace-confirm-fx-alpha-app",
                    focused=menu_focused == "workspace-confirm-fx-alpha-app",
                ),
            ]
        )
    if focused_button and focused_button != "task-rail-grouping":
        lines.append(button(focused_button, focused=True))
    if overrides.get("alpha_collapsed"):
        lines.append(button("project-Earlier_3afx-alpha-app", help_text="Collapsed"))
        lines.append(button("project-add-Earlier_3afx-alpha-app"))
    else:
        lines.append(button("project-Earlier_3afx-alpha-app", help_text="Expanded"))
        lines.append(button("project-add-Earlier_3afx-alpha-app"))
    lines.append(button("project-Earlier_3afx-beta-lib", help_text="Expanded"))
    lines.append(button("project-add-Earlier_3afx-beta-lib"))
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def run_assert(raw, phase, **overrides):
    tree = Path(raw) / ("ax-tree-" + phase + ".txt")
    out = Path(raw) / ("assert-" + phase + ".json")
    write_tree(tree, **overrides)
    proc = subprocess.run(
        [
            sys.executable, str(TOOLS), "assert",
            "--tree", str(tree),
            "--phase", phase,
            "--out", str(out),
        ],
        check=False, capture_output=True, text=True,
    )
    payload = json.loads(out.read_text(encoding="utf-8"))
    return proc, payload


class R3WaveBToolsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tools = load_tools()

    def test_parse_tree_extracts_rows_focus_and_connection(self):
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "tree.txt"
            write_tree(path, composer_focused=True, selected="session-fx-ses-beta-long")
            tree = self.tools.parse_tree(path)
            self.assertEqual(tree["focused"], ["composer-input"])
            self.assertEqual(
                tree["rows"]["session-fx-ses-beta-pending"]["help"], "Needs input"
            )
            self.assertEqual(tree["rows"]["session-fx-ses-beta-long"]["selected"], True)
            self.assertEqual(tree["grouping_value"], "Timeline")
            self.assertIn("Connected", tree["connection_status"])
            self.assertEqual(tree["timeline_counts"]["fx-ses-beta-pending"], 2)

    def test_next_needs_attention_passes_and_fails_without_help_word(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(raw, "next-needs-attention", composer_focused=True)
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            self.assertTrue(payload["pass"])
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "next-needs-attention",
                pending_help="Session",
                composer_focused=True,
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["needs-input-help"]["pass"])

    def test_tab_traverse_phases_pin_focused_identifier(self):
        cases = {
            "tab-traverse-scope": "project-scope",
            "tab-traverse-grouping": "task-rail-grouping",
            "tab-traverse-add-task": "add-task",
            "tab-reverse-grouping": "task-rail-grouping",
            "tab-reverse-scope": "project-scope",
        }
        for phase, target in cases.items():
            with tempfile.TemporaryDirectory() as raw:
                proc, payload = run_assert(raw, phase, focused_button=target)
                self.assertEqual(proc.returncode, 0, phase + ": " + proc.stdout)
                self.assertTrue(payload["pass"], phase)
            with tempfile.TemporaryDirectory() as raw:
                proc, payload = run_assert(raw, phase)
                self.assertEqual(proc.returncode, 5, phase)

    def test_rail_focus_phases_pin_focused_identifier(self):
        cases = {
            "rail-focus-alpha-header": "project-Earlier_3afx-alpha-app",
            "rail-focus-alpha-add": "project-add-Earlier_3afx-alpha-app",
            "rail-focus-beta-header": "project-Earlier_3afx-beta-lib",
            "rail-focus-beta-add": "project-add-Earlier_3afx-beta-lib",
            "rail-focus-task": "session-fx-ses-beta-pending",
        }
        for phase, target in cases.items():
            with tempfile.TemporaryDirectory() as raw:
                proc, payload = run_assert(
                    raw, phase, focused_button=target, alpha_collapsed=True
                )
                self.assertEqual(proc.returncode, 0, phase + ": " + proc.stdout)
                self.assertTrue(payload["pass"], phase)
            with tempfile.TemporaryDirectory() as raw:
                proc, _ = run_assert(raw, phase)
                self.assertEqual(proc.returncode, 5, phase)

    def test_rail_click_add_task_hands_focus_to_popover_highlight(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "rail-focus-add-task",
                workspace_confirm=True,
                menu_focused="workspace-confirm-fx-alpha-app",
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            self.assertTrue(payload["pass"])
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "rail-focus-add-task",
                workspace_confirm=True,
                focused_button="add-task",
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["rail-click-add-task-focus-handover"]["pass"])

    def test_button_enter_phases_require_only_menu_highlight_focus_and_open_menu(self):
        cases = {
            "button-enter-grouping-menu": (
                "task-rail-grouping",
                "grouping-menu",
                "group-timeline",
                "menu_open",
            ),
            "button-enter-add-task-popover": (
                "add-task",
                "workspace-confirm",
                "workspace-confirm-fx-alpha-app",
                "workspace_confirm",
            ),
            "button-enter-scope-menu": (
                "project-scope",
                "scope-menu",
                "scope-all",
                "scope_menu",
            ),
        }
        for phase, (target, menu_id, highlighted_id, override) in cases.items():
            with tempfile.TemporaryDirectory() as raw:
                proc, payload = run_assert(
                    raw, phase, menu_focused=highlighted_id, **{override: True}
                )
                self.assertEqual(proc.returncode, 0, phase + ": " + proc.stdout)
                self.assertTrue(payload["pass"], phase)
            with tempfile.TemporaryDirectory() as raw:
                proc, payload = run_assert(raw, phase, **{override: True})
                self.assertEqual(proc.returncode, 5, phase)
                names = {check["name"]: check for check in payload["checks"]}
                self.assertTrue(names["menu-open-" + target]["pass"], menu_id)
                self.assertFalse(names["menu-focus-" + highlighted_id]["pass"])
            with tempfile.TemporaryDirectory() as raw:
                proc, payload = run_assert(raw, phase, menu_focused=highlighted_id)
                self.assertEqual(proc.returncode, 5, phase)
                names = {check["name"]: check for check in payload["checks"]}
                self.assertFalse(names["menu-open-" + target]["pass"])
            with tempfile.TemporaryDirectory() as raw:
                proc, payload = run_assert(
                    raw,
                    phase,
                    focused_button=target,
                    menu_focused=highlighted_id,
                    **{override: True},
                )
                self.assertEqual(proc.returncode, 5, phase)
                names = {check["name"]: check for check in payload["checks"]}
                self.assertFalse(names["menu-focus-" + highlighted_id]["pass"])
                self.assertIn(
                    "trigger " + target,
                    names["menu-focus-" + highlighted_id]["detail"],
                )

    def test_key_open_task_asserts_active_and_timeline(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "key-open-task",
                selected="session-fx-ses-beta-toolfailed",
                entry_session="fx-ses-beta-toolfailed",
                composer_focused=True,
            )
            self.assertEqual(proc.returncode, 0)
            self.assertTrue(payload["pass"])
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(raw, "key-open-task", entries=0)
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["timeline-fx-ses-beta-toolfailed"]["pass"])

    def test_ax_current_task_closes_menu_and_focuses_composer(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "ax-current-task-menu",
                selected="session-fx-ses-beta-toolfailed",
                entry_session="fx-ses-beta-toolfailed",
                composer_focused=True,
            )
            self.assertEqual(proc.returncode, 0)
            self.assertTrue(payload["pass"])
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "ax-current-task-menu",
                selected="session-fx-ses-beta-toolfailed",
                entry_session="fx-ses-beta-toolfailed",
                menu_open=True,
                menu_focused="group-timeline",
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["ax-current-task-menu-closed"]["pass"])
            self.assertFalse(names["composer-focus-ax-current-task-menu"]["pass"])

    def test_grouping_keyboard_round_trip_pins_value_menu_and_selection(self):
        for phase, grouping in (
            ("grouping-projects-keyboard", "Projects"),
            ("grouping-timeline-keyboard", "Timeline"),
        ):
            with tempfile.TemporaryDirectory() as raw:
                proc, payload = run_assert(
                    raw,
                    phase,
                    grouping=grouping,
                    selected="session-fx-ses-beta-toolfailed",
                    entry_session="fx-ses-beta-toolfailed",
                    focused_button="task-rail-grouping",
                )
                self.assertEqual(proc.returncode, 0, phase)
                self.assertTrue(payload["pass"])
            with tempfile.TemporaryDirectory() as raw:
                proc, payload = run_assert(raw, phase, menu_open=True)
                self.assertEqual(proc.returncode, 5)
                names = {check["name"]: check for check in payload["checks"]}
                self.assertFalse(names["grouping-menu-closed"]["pass"])

    def test_cycle_phases_pin_selected_row(self):
        cases = (
            ("cycle-down-1", "session-fx-ses-beta-long", "fx-ses-beta-long"),
            ("cycle-down-2", "session-fx-ses-beta-cancelled", "fx-ses-beta-cancelled"),
            ("cycle-up", "session-fx-ses-beta-long", "fx-ses-beta-long"),
        )
        for phase, selected, timeline_session in cases:
            with tempfile.TemporaryDirectory() as raw:
                proc, payload = run_assert(
                    raw,
                    phase,
                    selected=selected,
                    entry_session=timeline_session,
                    composer_focused=True,
                )
                self.assertEqual(proc.returncode, 0, phase)
                self.assertTrue(payload["pass"], phase)

        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "cycle-down-1",
                selected="session-fx-ses-beta-long",
                entry_session="fx-ses-beta-long",
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["composer-focus-cycle-down-1"]["pass"])

    def test_single_visible_cycle_requires_focus_handoff_without_switching(self):
        only_row = [row("session-ses-only", selected=True)]
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "single-visible-before-cycle",
                rows=only_row,
                focused_button="project-scope",
            )
            self.assertEqual(proc.returncode, 0)
            self.assertTrue(payload["pass"])
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "single-visible-cycle",
                rows=only_row,
                composer_focused=True,
            )
            self.assertEqual(proc.returncode, 0)
            self.assertTrue(payload["pass"])
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "single-visible-cycle",
                rows=[
                    row("session-ses-only", selected=True),
                    row("session-ses-extra"),
                ],
                composer_focused=True,
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["single-visible-active-single-visible-cycle"]["pass"])

    def test_disconnect_phases_require_connection_marker_and_selection(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "disconnected-kept",
                connection="Local · Disconnected · dropped",
                selected="session-fx-ses-beta-long",
                entry_session="fx-ses-beta-long",
                reconnect_button=True,
            )
            self.assertEqual(proc.returncode, 0)
            self.assertTrue(payload["pass"])
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "reconnected-kept",
                connection="Local · Connected",
                selected="session-fx-ses-beta-long",
                entry_session="fx-ses-beta-long",
            )
            self.assertEqual(proc.returncode, 0)
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(raw, "disconnected-kept", reconnect_button=False)
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["reconnect-button-present"]["pass"])

    def test_blocked_live_requires_new_session_row_with_word(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "blocked-live",
                rows=[row("session-ses-1", help_text="Blocked", selected=True)],
            )
            self.assertEqual(proc.returncode, 0)
            self.assertTrue(payload["pass"])
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "blocked-live",
                rows=[
                    row("session-ses-1", help_text="Blocked", selected=False),
                    row("session-fx-ses-beta-pending", selected=True),
                ],
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["blocked-row-active"]["pass"])
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "blocked-live",
                rows=[row("session-fx-ses-beta-long", help_text="Blocked", selected=True)],
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["blocked-row-is-new-session"]["pass"])

    def test_unread_live_requires_background_row_with_word(self):
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "unread-live",
                rows=[
                    row("session-self-check-1", help_text="Session · Unread"),
                    row("session-ses-2", help_text="Blocked", selected=True),
                ],
            )
            self.assertEqual(proc.returncode, 0)
            self.assertTrue(payload["pass"])
        with tempfile.TemporaryDirectory() as raw:
            proc, payload = run_assert(
                raw,
                "unread-live",
                rows=[row("session-self-check-1", help_text="Session · Unread", selected=True)],
                connection="Local · Connected",
            )
            self.assertEqual(proc.returncode, 5)
            names = {check["name"]: check for check in payload["checks"]}
            self.assertFalse(names["unread-row-not-active"]["pass"])

    def test_unknown_phase_is_rejected(self):
        with tempfile.TemporaryDirectory() as raw:
            tree = Path(raw) / "tree.txt"
            write_tree(tree)
            with self.assertRaises(ValueError):
                self.tools.phase_checks(self.tools.parse_tree(tree), "bogus-phase")

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
