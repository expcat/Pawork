#!/usr/bin/env python3
"""R4 Wave B WS-2/WS-3b regressions: states assertions + driver guards."""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from PIL import Image  # noqa: F401  # tools module import contract

TOOLS = Path(__file__).with_name("ui-wave-d-tools.py")
DRIVER = Path(__file__).with_name("ui-r4-wave-b-states.sh")


def load_tools():
    spec = importlib.util.spec_from_file_location("ui_wave_d_tools_r4b", TOOLS)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def session_row(identifier, selected=False, help_text="Session"):
    line = 'role=AXRow identifier="' + identifier + '" help="' + help_text + '" enabled=1'
    if selected:
        line += " selected=1"
    return line


def timeline_row(identifier, value):
    return (
        'role=AXRow value="' + value + '" identifier="' + identifier
        + '" description="Run" help="2h" enabled=1'
    )


def summary_card(identifier, label, help_text=""):
    return (
        'role=AXStaticText identifier="' + identifier + '" description="'
        + label + '" help="' + help_text + '" enabled=1'
    )


def static(identifier, label):
    return 'role=AXStaticText identifier="' + identifier + '" description="' + label + '" enabled=1'


def tool_row(identifier, value, help_text):
    return (
        'role=AXRow value="' + value + '" identifier="' + identifier
        + '" description="Tool · run_command" help="' + help_text + '" enabled=1'
    )


def button(identifier, enabled):
    return 'role=AXButton identifier="' + identifier + '" enabled=' + str(enabled) + ' actions=[AXPress]'


SHELL = [
    'role=AXGroup identifier="pawork-root"',
    'role=AXGroup identifier="task-rail"',
    'role=AXGroup identifier="timeline"',
    'role=AXGroup identifier="composer"',
]


class StatesAssertTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tools = load_tools()

    def assert_tree(self, lines, phase, logical=None, extra_args=()):
        """Write tree, run states-assert subprocess, return (proc, payload)."""
        with tempfile.TemporaryDirectory() as raw:
            tree = Path(raw) / "tree.txt"
            out = Path(raw) / "assert.json"
            tree.write_text("\n".join(lines) + "\n", encoding="utf-8")
            cmd = [
                sys.executable, str(TOOLS), "states-assert",
                "--tree", str(tree), "--phase", phase, "--out", str(out),
            ]
            if logical is not None:
                cmd += ["--logical-entries", str(logical)]
            cmd += list(extra_args)
            proc = subprocess.run(cmd, check=False, capture_output=True, text=True)
            payload = json.loads(out.read_text("utf-8"))
            return proc, payload

    def failed_checks(self, payload):
        return [check["name"] for check in payload["checks"] if not check["pass"]]

    def test_approval_visible_positive_and_negative(self):
        lines = SHELL + [
            session_row("session-fx-ses-beta-pending", selected=True, help_text="Needs input"),
            timeline_row("timeline-entry-evt-fx-ses-beta-pending-9", "approval requested · tool · write_file"),
            'role=AXGroup identifier="approval-card" value="write_file · 等待审批"',
            button("approve-once", 1),
            button("approve-for-run", 1),
            button("approve-deny", 1),
        ]
        proc, payload = self.assert_tree(lines, "approval-visible")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(payload["pass"])
        # 负例：卡消失（按钮仍在）必须失败。
        negative = [line for line in lines if 'identifier="approval-card"' not in line]
        proc, payload = self.assert_tree(negative, "approval-visible")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("approval-card-present", self.failed_checks(payload))
        # 负例：按钮 disabled。
        disabled = [
            line.replace('identifier="approve-once" enabled=1', 'identifier="approve-once" enabled=0')
            for line in lines
        ]
        proc, payload = self.assert_tree(disabled, "approval-visible")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("approval-buttons-enabled", self.failed_checks(payload))

    def test_approval_resolved_positive_and_negatives(self):
        # live wire 契约：决策行不即时出现（快照重放才推），正例不含
        # 决策行即通过；Resolved 调用诚实显示 failed。
        base = SHELL + [
            session_row("session-fx-ses-beta-pending", selected=True),
            timeline_row("timeline-entry-evt-fx-ses-beta-pending-9", "approval requested · tool · write_file"),
            tool_row("tool-row-evt-fx-ses-beta-pending-10", "failed", "write_file 失败"),
            'role=AXTextArea identifier="composer-input" focused=1',
        ]
        proc, payload = self.assert_tree(base, "approval-resolved")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(payload["pass"])
        no_focus = [line.replace(" focused=1", "") for line in base]
        proc, payload = self.assert_tree(no_focus, "approval-resolved")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("approval-focus-composer", self.failed_checks(payload))
        stray = base + ['role=AXGroup identifier="approval-card" value="write_file"']
        proc, payload = self.assert_tree(stray, "approval-resolved")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("approval-card-absent", self.failed_checks(payload))
        still_needs = [
            line.replace('help="Session"', 'help="Needs input"') if "beta-pending" in line else line
            for line in base
        ]
        proc, payload = self.assert_tree(still_needs, "approval-resolved")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("rail-needs-input-cleared", self.failed_checks(payload))
        succeeded = [
            line.replace('value="failed"', 'value="Completed"') for line in base
        ]
        proc, payload = self.assert_tree(succeeded, "approval-resolved")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("approval-tool-row-failed-value", self.failed_checks(payload))
        # 负例：他 session 的 failed tool-row 不得满足当前会话检查。
        cross_session = base[:5] + [
            tool_row("tool-row-evt-fx-ses-beta-toolfailed-2", "failed", "fixture tool failure"),
        ]
        proc, payload = self.assert_tree(cross_session, "approval-resolved")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("approval-tool-row-failed-value", self.failed_checks(payload))

    def test_approval_replayed_positive_and_negatives(self):
        # 重放契约：重选回 beta-pending 后决策行必须出现、卡不复活。
        base = SHELL + [
            session_row("session-fx-ses-beta-pending", selected=True),
            timeline_row("timeline-entry-evt-fx-ses-beta-pending-9", "approval requested · tool · write_file"),
            tool_row("tool-row-evt-fx-ses-beta-pending-10", "failed", "write_file 失败"),
            timeline_row("timeline-entry-evt-fx-ses-beta-pending-11", "approval approve_once"),
        ]
        proc, payload = self.assert_tree(base, "approval-replayed")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(payload["pass"])
        no_decision = base[:7]
        proc, payload = self.assert_tree(no_decision, "approval-replayed")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("timeline-approval-decision", self.failed_checks(payload))
        stray_card = base + ['role=AXGroup identifier="approval-card" value="write_file"']
        proc, payload = self.assert_tree(stray_card, "approval-replayed")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("approval-card-absent", self.failed_checks(payload))
        wrong_selected = [
            line.replace("beta-pending", "alpha-today") if "beta-pending" in line else line
            for line in base
        ]
        proc, payload = self.assert_tree(wrong_selected, "approval-replayed")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("selected-session-fx-ses-beta-pending", self.failed_checks(payload))

    def test_failed_summary_requires_real_seed_reason(self):
        base = SHELL + [
            session_row("session-fx-ses-alpha-yesterday", selected=True),
            summary_card(
                "run-summary-card-evt-fx-ses-alpha-yesterday-4",
                "Run failed",
                "fixture scripted provider failure",
            ),
            static("run-footer-evt-fx-ses-alpha-yesterday-4", "Run failed · 27h"),
        ]
        proc, payload = self.assert_tree(base, "failed-summary")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        generic = [
            line.replace(
                'help="fixture scripted provider failure"',
                'help="The run failed. See the error details above."',
            )
            for line in base
        ]
        proc, payload = self.assert_tree(generic, "failed-summary")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("run-summary-help-contains", self.failed_checks(payload))

    def test_live_failed_honest_fallback_and_replayed_reason(self):
        # live RunChanged{Failed} 不带原因（wire 契约）：摘要卡只能兜底。
        live = SHELL + [
            session_row("session-fx-ses-alpha-today", selected=True),
            summary_card("run-summary-card-app-evt-8", "Run failed", "The run failed."),
            static("run-footer-app-evt-8", "Run failed · now"),
        ]
        proc, payload = self.assert_tree(live, "live-failed")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        # wire 若演进为 live 带原因，help-exact 钉住契约失败，提醒同步改断言。
        evolved = [
            line.replace(
                'help="The run failed."',
                'help="fixture scripted provider failure"',
            )
            for line in live
        ]
        proc, payload = self.assert_tree(evolved, "live-failed")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("run-summary-help-exact", self.failed_checks(payload))
        # 重放（重选）后快照路径可见真实原因。
        proc, payload = self.assert_tree(evolved, "failed-replayed")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        proc, payload = self.assert_tree(live, "failed-replayed")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("run-summary-help-contains", self.failed_checks(payload))

    def test_driver_s7_includes_failed_replayed_phase(self):
        text = DRIVER.read_text("utf-8")
        self.assertIn("failed-replayed", text)
        self.assertIn("S7 reselect away", text)
        self.assertIn("S7 reselect back", text)

    def test_cancelled_summary_and_tool_failed(self):
        cancelled = SHELL + [
            session_row("session-fx-ses-beta-cancelled", selected=True),
            summary_card("run-summary-card-evt-fx-ses-beta-cancelled-3", "Run cancelled"),
            static("run-footer-evt-fx-ses-beta-cancelled-3", "Run cancelled · 10d"),
        ]
        proc, payload = self.assert_tree(cancelled, "cancelled-summary")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        wrong_session = [
            line.replace("session-fx-ses-beta-cancelled", "session-fx-ses-beta-long")
            for line in cancelled
        ]
        proc, payload = self.assert_tree(wrong_session, "cancelled-summary")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("selected-session-fx-ses-beta-cancelled", self.failed_checks(payload))

        tool_failed = SHELL + [
            session_row("session-fx-ses-beta-toolfailed", selected=True),
            tool_row(
                "tool-row-evt-fx-ses-beta-toolfailed-2",
                "failed",
                "fixture tool failure: build quota exceeded",
            ),
        ]
        proc, payload = self.assert_tree(tool_failed, "tool-failed")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        succeeded = [
            line.replace('value="failed"', 'value="Completed"') for line in tool_failed
        ]
        proc, payload = self.assert_tree(succeeded, "tool-failed")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("tool-row-failed-value", self.failed_checks(payload))

    def test_virtualized_requires_window_slice_below_logical_rows(self):
        lines = SHELL + [session_row("session-fx-ses-beta-long", selected=True)]
        lines += [
            timeline_row("timeline-entry-evt-fx-ses-beta-long-" + str(ix), "第 {} 轮".format(ix))
            for ix in range(12)
        ]
        proc, payload = self.assert_tree(lines, "virtualized", logical=64)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(payload["pass"])
        proc, payload = self.assert_tree(lines, "virtualized", logical=12)
        self.assertEqual(proc.returncode, 5)
        self.assertIn("virtualization-window-slice", self.failed_checks(payload))
        proc, payload = self.assert_tree(lines, "virtualized", logical=63)
        self.assertEqual(proc.returncode, 5)
        self.assertIn("virtualization-logical-rows", self.failed_checks(payload))

    def test_streamed_summary_requires_cleared_composer_and_terminal_row(self):
        base = SHELL + [
            session_row("session-fx-ses-alpha-today", selected=True),
            summary_card("run-summary-card-evt-fx-ses-alpha-today-90", "Ready for review"),
            timeline_row("timeline-entry-evt-fx-ses-alpha-today-90", "Run completed"),
            'role=AXTextArea identifier="composer-input" description="Message" enabled=1',
        ]
        proc, payload = self.assert_tree(base, "streamed-summary")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        dirty = base[:-1] + [
            'role=AXTextArea identifier="composer-input" value="残留草稿" description="Message" enabled=1',
        ]
        proc, payload = self.assert_tree(dirty, "streamed-summary")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("composer-cleared", self.failed_checks(payload))

    def test_hang_phases_assert_composer_button_states(self):
        running = SHELL + [
            session_row("session-fx-ses-alpha-today", selected=True),
            button("cancel", 1),
        ]
        proc, payload = self.assert_tree(running, "hang-cancelable")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        idle_slot = SHELL + [
            session_row("session-fx-ses-alpha-today", selected=True),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(idle_slot, "hang-cancelable")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("composer-cancel-enabled-1", self.failed_checks(payload))
        # 负例：旧双节点形态（send+cancel 同存）必须失败。
        dual = SHELL + [
            session_row("session-fx-ses-alpha-today", selected=True),
            button("cancel", 1),
            button("send", 0),
        ]
        proc, payload = self.assert_tree(dual, "hang-cancelable")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("composer-send-absent", self.failed_checks(payload))

        cancelled = SHELL + [
            session_row("session-fx-ses-alpha-today", selected=True),
            summary_card("run-summary-card-evt-fx-ses-alpha-today-96", "Run cancelled"),
            static("run-footer-evt-fx-ses-alpha-today-96", "Run cancelled · 1m"),
            button("send", 0),
            'role=AXTextArea identifier="composer-input" description="Message" enabled=1',
        ]
        proc, payload = self.assert_tree(cancelled, "hang-cancelled")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        stuck = [line.replace('identifier="send" enabled=0', 'identifier="send" enabled=1') for line in cancelled]
        proc, payload = self.assert_tree(stuck, "hang-cancelled")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("composer-send-enabled-0", self.failed_checks(payload))

    def test_connection_phases_and_negatives(self):
        disconnected = SHELL + [
            session_row("session-fx-ses-alpha-today", selected=True),
            button("reconnect", 1),
            'role=AXStaticText identifier="connection-status" value="Disconnected · 连接已断开"',
            timeline_row("timeline-entry-evt-fx-ses-alpha-today-90", "Run completed"),
        ]
        proc, payload = self.assert_tree(disconnected, "disconnected-retained")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        connecting = [
            line.replace('value="Disconnected · 连接已断开"', 'value="Connecting…"')
            for line in disconnected
        ]
        proc, payload = self.assert_tree(connecting, "disconnected-retained")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("connection-status-disconnected", self.failed_checks(payload))

        reconnected = SHELL + [
            session_row("session-fx-ses-alpha-today", selected=True),
            'role=AXStaticText identifier="connection-status" value="Local · Connected"',
            timeline_row("timeline-entry-evt-fx-ses-alpha-today-90", "Run completed"),
        ]
        proc, payload = self.assert_tree(reconnected, "reconnected-replay")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        stray = reconnected + [button("reconnect", 1)]
        proc, payload = self.assert_tree(stray, "reconnected-replay")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("reconnect-absent", self.failed_checks(payload))

    def test_states_assert_rejects_unknown_phase(self):
        with tempfile.TemporaryDirectory() as raw:
            tree = Path(raw) / "tree.txt"
            tree.write_text("\n".join(SHELL) + "\n", encoding="utf-8")
            proc = subprocess.run(
                [
                    sys.executable, str(TOOLS), "states-assert",
                    "--tree", str(tree), "--phase", "not-a-phase",
                    "--out", str(Path(raw) / "out.json"),
                ],
                check=False, capture_output=True, text=True,
            )
            self.assertEqual(proc.returncode, 2)


class ApprovalReadTest(unittest.TestCase):
    def test_approval_read_outputs_tool_and_run_id(self):
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "approval_visible"
            path.write_text(
                json.dumps({"tool": "write_file", "run_id": "run-9", "at_ms": 1}),
                encoding="utf-8",
            )
            proc = subprocess.run(
                [sys.executable, str(TOOLS), "approval-read", "--file", str(path)],
                check=False, capture_output=True, text=True,
            )
            self.assertEqual(proc.returncode, 0)
            self.assertEqual(proc.stdout.strip(), "write_file run-9")

    def test_approval_read_rejects_missing_or_incomplete(self):
        with tempfile.TemporaryDirectory() as raw:
            missing = Path(raw) / "missing"
            proc = subprocess.run(
                [sys.executable, str(TOOLS), "approval-read", "--file", str(missing)],
                check=False, capture_output=True, text=True,
            )
            self.assertEqual(proc.returncode, 1)
            incomplete = Path(raw) / "approval_visible"
            incomplete.write_text(json.dumps({"tool": "write_file"}), encoding="utf-8")
            proc = subprocess.run(
                [sys.executable, str(TOOLS), "approval-read", "--file", str(incomplete)],
                check=False, capture_output=True, text=True,
            )
            self.assertEqual(proc.returncode, 1)


class EntryCompareTest(unittest.TestCase):
    def tree_with_rows(self, raw, name, rows):
        """rows: (identifier 本体, value) 列表；identifier 加 timeline-entry- 前缀。"""
        lines = SHELL + [
            timeline_row("timeline-entry-" + identifier, value)
            for identifier, value in rows
        ]
        path = Path(raw) / ("tree-" + name + ".txt")
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return path

    def tree_with_entries(self, raw, identifiers):
        return self.tree_with_rows(
            raw,
            str(len(identifiers)) + "-" + str(identifiers[-1]),
            [
                ("evt-fx-ses-alpha-today-" + str(ix), "row " + str(ix))
                for ix in identifiers
            ],
        )

    def run_compare(self, tree_a, tree_b, entries_a, entries_b):
        with tempfile.TemporaryDirectory() as raw:
            out = Path(raw) / "compare.json"
            proc = subprocess.run(
                [
                    sys.executable, str(TOOLS), "entry-compare",
                    "--tree-a", str(tree_a), "--tree-b", str(tree_b),
                    "--entries-a", str(entries_a), "--entries-b", str(entries_b),
                    "--out", str(out),
                ],
                check=False, capture_output=True, text=True,
            )
            payload = json.loads(out.read_text("utf-8"))
            failed = [check["name"] for check in payload["checks"] if not check["pass"]]
            return proc, failed

    def test_identical_replay_passes(self):
        with tempfile.TemporaryDirectory() as raw:
            tree_a = self.tree_with_entries(raw, range(20))
            tree_b = self.tree_with_entries(raw, range(20))
            proc, failed = self.run_compare(tree_a, tree_b, 20, 20)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertEqual(failed, [])

    def test_identifier_set_mismatch_fails(self):
        with tempfile.TemporaryDirectory() as raw:
            tree_a = self.tree_with_entries(raw, range(20))
            tree_b = self.tree_with_entries(raw, list(range(19)) + [99])
            proc, failed = self.run_compare(tree_a, tree_b, 20, 20)
            self.assertEqual(proc.returncode, 6)
            self.assertIn("timeline-seed-identifier-sets-identical", failed)

    def test_barrier_entry_count_mismatch_fails(self):
        with tempfile.TemporaryDirectory() as raw:
            tree_a = self.tree_with_entries(raw, range(20))
            tree_b = self.tree_with_entries(raw, range(20))
            proc, failed = self.run_compare(tree_a, tree_b, 20, 21)
            self.assertEqual(proc.returncode, 6)
            self.assertIn("barrier-entry-count-equal", failed)

    def test_live_replay_mixed_identifier_classes_pass(self):
        """live 树（app-evt-* + 种子）对重放树（持久化 evt-* + 种子）通过：
        种子集合一致 + live value 多重集一致，id 形态差异不参与判定。"""
        seeds = [
            ("evt-fx-ses-alpha-today-" + str(ix), "row " + str(ix))
            for ix in range(5)
        ]
        with tempfile.TemporaryDirectory() as raw:
            tree_a = self.tree_with_rows(raw, "live", seeds + [
                ("app-evt-31", "run started"),
                ("local-echo-r-2", "assistant hi"),
            ])
            tree_b = self.tree_with_rows(raw, "replay", seeds + [
                ("evt-ses-alpha-today-31", "run started"),
                ("local-echo-r-9", "assistant hi"),
            ])
            proc, failed = self.run_compare(tree_a, tree_b, 7, 7)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertEqual(failed, [])

    def test_live_value_degradation_pair_passes(self):
        """run failed · reason（live）对 run failed（重放）归一后相等。"""
        seeds = [("evt-fx-ses-alpha-today-3", "row 3")]
        with tempfile.TemporaryDirectory() as raw:
            tree_a = self.tree_with_rows(raw, "live-failed", seeds + [
                ("app-evt-9", "run failed · fixture scripted provider failure"),
            ])
            tree_b = self.tree_with_rows(raw, "replay-failed", seeds + [
                ("evt-ses-alpha-today-9", "run failed"),
            ])
            proc, failed = self.run_compare(tree_a, tree_b, 2, 2)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertEqual(failed, [])

    def test_live_value_extra_or_missing_fails(self):
        seeds = [("evt-fx-ses-alpha-today-3", "row 3")]
        with tempfile.TemporaryDirectory() as raw:
            tree_with_live = self.tree_with_rows(raw, "with-live", seeds + [
                ("app-evt-9", "run started"),
            ])
            tree_seed_only = self.tree_with_rows(raw, "seed-only", seeds)
            proc, failed = self.run_compare(tree_with_live, tree_seed_only, 2, 1)
            self.assertEqual(proc.returncode, 6)
            self.assertIn("timeline-live-value-multisets-equal", failed)
            proc, failed = self.run_compare(tree_seed_only, tree_with_live, 1, 2)
            self.assertEqual(proc.returncode, 6)
            self.assertIn("timeline-live-value-multisets-equal", failed)


class DriverGuardTest(unittest.TestCase):
    def test_driver_s1_includes_reselect_replay_phase(self):
        text = DRIVER.read_text("utf-8")
        self.assertIn("approval-replayed", text)
        self.assertIn("reselect away", text)
        self.assertIn("reselect back", text)

    def test_driver_rejects_nonempty_output_to_prevent_stale_evidence(self):
        with tempfile.TemporaryDirectory() as raw:
            out = Path(raw) / "evidence"
            out.mkdir()
            (out / "stale.json").write_text("{}\n", encoding="utf-8")
            env = dict(os.environ)
            env["PAWORK_WAVE_D_PYTHON"] = sys.executable
            proc = subprocess.run(
                ["bash", str(DRIVER), "run", "--out", str(out)],
                check=False, capture_output=True, text=True, env=env,
            )
            self.assertEqual(proc.returncode, 3, proc.stdout + proc.stderr)
            self.assertIn("must be new or empty", proc.stderr)
            self.assertTrue((out / "stale.json").exists())


if __name__ == "__main__":
    unittest.main()
