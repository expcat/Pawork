#!/usr/bin/env python3
"""R5 Wave B U2 composer-state assertions + driver guards.

The driver (ui-r5-wave-b-states.sh) calls this module as a CLI for phase
assertions so Wave B stays inside the scripts write-set and does not extend
ui-wave-d-tools.py STATES_PHASES.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

DRIVER = Path(__file__).with_name("ui-r5-wave-b-states.sh")
IDENTIFIER_RE = re.compile(r'identifier="([^"]*)"')
VALUE_RE = re.compile(r'value="([^"]*)"')
DESCRIPTION_RE = re.compile(r'description="([^"]*)"')
ENABLED_RE = re.compile(r"enabled=([01])")
NL = chr(10)
AX_VALUE_LIMIT = 240

PLACEHOLDER_SESSION = "Message Pawork… (Enter to send, Shift+Enter for newline)"
PLACEHOLDER_NO_SESSION = "Open a session to send messages."
PLACEHOLDER_DISCONNECTED = "Disconnected — click Reconnect before sending."
SESSION_A = "session-fx-ses-alpha-today"
SESSION_B = "session-fx-ses-alpha-yesterday"
MODEL_MENU = "model-menu"
MODEL_PICKER = "model-picker"

SCENES = (
    ("r5-1", "empty-input", "empty composer: send disabled, cancel absent, empty AX value"),
    ("r5-2", "send-cancel", "set-value -> send enabled -> running cancel -> send restored + multiline echo"),
    ("r5-3", "disconnected-draft", "drop-socket keeps draft, send disabled; restart-host + reconnect keeps text"),
    ("r5-4", "draft-isolation", "task switch isolates draft-A / draft-B and restores draft-A"),
    ("r5-5", "paste-large", "pbcopy+HID cmd-v >=8KB CJK paste, send, timeline echo"),
    ("r5-6", "hang-visual", "fixture:hang running Cancel slot screenshot"),
    ("r5-7", "model-menu", "model-picker menu; select other model or degrade to open/close"),
    ("r5-8", "narrow-window", "resize 1080x720, composer/send reachable, no overflow"),
    ("r5-9", "keyboard-path", "HID type+Return send, cmd-. cancel, shift-return newline"),
)

PHASES = (
    "empty-input",
    "send-enabled",
    "running-cancelable",
    "send-restored",
    "draft-set",
    "disconnected-draft",
    "reconnected-draft",
    "draft-a",
    "draft-b-empty",
    "draft-b",
    "draft-a-restored",
    "paste-complete",
    "paste-echoed",
    "multiline-echoed",
    "hang-cancelable",
    "model-menu-open",
    "model-menu-closed",
    "model-trigger-changed",
    "narrow-reachable",
    "keyboard-typed",
    "keyboard-newline",
    "keyboard-sent",
    "keyboard-cancelled",
)


def now_iso():
    from datetime import datetime, timezone
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_composer_tree(path):
    lines = [
        line
        for line in Path(path).read_text("utf-8").splitlines()
        if not line.lstrip().startswith("#")
    ]
    identifiers = set()
    nodes = {}
    connection_status = ""
    for line in lines:
        ids = IDENTIFIER_RE.findall(line)
        if not ids:
            continue
        identifier = ids[0]
        identifiers.add(identifier)
        values = VALUE_RE.findall(line)
        descriptions = DESCRIPTION_RE.findall(line)
        enabled = ENABLED_RE.findall(line)
        nodes[identifier] = {
            "value": values[0] if values else "",
            "label": descriptions[0] if descriptions else "",
            "enabled": enabled[0] if enabled else None,
            "selected": "selected=1" in line,
            "focused": "focused=1" in line,
            "line": line,
        }
        if identifier == "connection-status":
            connection_status = values[0] if values else ""
    timeline_values = {
        identifier: facts["value"]
        for identifier, facts in nodes.items()
        if identifier.startswith("timeline-entry-")
    }
    return {
        "identifiers": identifiers,
        "nodes": nodes,
        "connection_status": connection_status,
        "timeline_values": timeline_values,
    }


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
                "x": float(fields["x"]),
                "y": float(fields["y"]),
                "w": float(fields["w"]),
                "h": float(fields["h"]),
            }
        except (KeyError, ValueError):
            frames[fields["id"]] = {"error": True}
    return frames


def check(name, ok, detail):
    return {"name": name, "pass": bool(ok), "detail": detail}


def node(tree, identifier):
    return tree["nodes"].get(identifier)


def selected_session(tree):
    return [
        identifier
        for identifier, facts in tree["nodes"].items()
        if identifier.startswith("session-") and facts["selected"]
    ]


def composer_value(tree):
    item = node(tree, "composer-input")
    return None if item is None else item["value"]


def button_state(tree, identifier):
    item = node(tree, identifier)
    if item is None:
        return "absent"
    return "enabled=" + (item["enabled"] if item["enabled"] is not None else "?")


def expect_selected(tree, expected):
    selected = selected_session(tree)
    return check(
        "selected-" + expected,
        selected == [expected],
        "selected rows: " + (",".join(selected) or "none") + " (expect " + expected + ")",
    )


def expect_send(tree, enabled):
    item = node(tree, "send")
    if enabled is None:
        return check(
            "composer-send-absent",
            item is None,
            "send " + ("present " + button_state(tree, "send") if item is not None else "absent"),
        )
    wanted = "1" if enabled else "0"
    return check(
        "composer-send-enabled-" + wanted,
        item is not None and item["enabled"] == wanted,
        "send " + button_state(tree, "send") + " (expect enabled=" + wanted + ")",
    )


def expect_cancel(tree, enabled):
    item = node(tree, "cancel")
    if enabled is None:
        return check(
            "composer-cancel-absent",
            item is None,
            "cancel " + ("present " + button_state(tree, "cancel") if item is not None else "absent"),
        )
    wanted = "1" if enabled else "0"
    return check(
        "composer-cancel-enabled-" + wanted,
        item is not None and item["enabled"] == wanted,
        "cancel " + button_state(tree, "cancel") + " (expect enabled=" + wanted + ")",
    )


def ax_escape(value):
    escaped = value.replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t")
    if len(escaped) > AX_VALUE_LIMIT:
        return escaped[:237] + "..."
    return escaped


def ax_unescape(value):
    if value is None:
        return None
    core = value[:-3] if value.endswith("...") else value
    return core.replace("\\n", "\n").replace("\\r", "\r").replace("\\t", "\t")


def values_match(actual, expected):
    if actual is None:
        return False
    if actual == expected:
        return True
    escaped = ax_escape(expected)
    if actual == escaped:
        return True
    unescaped = ax_unescape(actual)
    if unescaped == expected:
        return True
    if expected.startswith(unescaped) and actual.endswith("..."):
        return True
    if escaped.endswith("..." ) and expected.startswith(ax_unescape(escaped[:-3])):
        return actual == escaped
    return False


def expect_composer_value(tree, expected, name="composer-value"):
    actual = composer_value(tree)
    return check(
        name,
        values_match(actual, expected),
        "composer-input value=" + (repr(actual) if actual is not None else "absent")
        + " (expect " + repr(expected) + ")",
    )


def expect_composer_contains(tree, needle, name="composer-contains"):
    actual = composer_value(tree)
    ok = actual is not None and (
        needle in actual or needle in ax_unescape(actual) or ax_escape(needle) in actual
    )
    return check(
        name,
        ok,
        "composer-input value=" + (repr(actual) if actual is not None else "absent")
        + " (expect substring " + repr(needle) + ")",
    )


def expect_timeline_value(tree, needle, name="timeline-echo"):
    hits = [
        identifier
        for identifier, value in tree["timeline_values"].items()
        if needle in value or needle in ax_unescape(value) or ax_escape(needle) in value
    ]
    return check(
        name,
        bool(hits),
        "timeline-entry values containing " + repr(needle) + ": "
        + (",".join(hits[:4]) or "none"),
    )


def model_option_ids(tree):
    return sorted(
        identifier
        for identifier in tree["identifiers"]
        if identifier.startswith("model-") and identifier != MODEL_MENU
        and identifier != MODEL_PICKER
    )


def rect_within(inner, outer, slop=1.0):
    if inner is None or outer is None or inner.get("error") or outer.get("error"):
        return False
    return (
        inner["x"] + slop >= outer["x"]
        and inner["y"] + slop >= outer["y"]
        and inner["x"] + inner["w"] <= outer["x"] + outer["w"] + slop
        and inner["y"] + inner["h"] <= outer["y"] + outer["h"] + slop
    )


def near(value, target, tol):
    return abs(value - target) <= tol


def phase_checks(tree, phase, expect_value=None, expect_contains=None, frames=None):
    if phase not in PHASES:
        raise ValueError("unknown composer phase: " + str(phase))
    checks = []
    if phase == "empty-input":
        checks.append(expect_selected(tree, SESSION_A))
        checks.append(expect_send(tree, False))
        checks.append(expect_cancel(tree, None))
        wanted = "" if expect_value is None else expect_value
        checks.append(expect_composer_value(tree, wanted, "composer-empty-value"))
    elif phase == "send-enabled":
        checks.append(expect_selected(tree, SESSION_A))
        checks.append(expect_send(tree, True))
        checks.append(expect_cancel(tree, None))
        if expect_value is not None:
            checks.append(expect_composer_value(tree, expect_value))
    elif phase == "running-cancelable":
        checks.append(expect_selected(tree, SESSION_A))
        checks.append(expect_cancel(tree, True))
        checks.append(expect_send(tree, None))
    elif phase == "send-restored":
        checks.append(expect_selected(tree, SESSION_A))
        # F2: send clears the visible composer; cancel does not restore the
        # draft, so the send slot returns disabled on the empty composer.
        checks.append(expect_send(tree, False))
        checks.append(expect_cancel(tree, None))
    elif phase == "draft-set":
        checks.append(expect_send(tree, True))
        checks.append(expect_cancel(tree, None))
        checks.append(expect_composer_value(tree, expect_value or ""))
    elif phase == "disconnected-draft":
        checks.append(expect_send(tree, False))
        status = tree["connection_status"]
        checks.append(check(
            "connection-status-disconnected",
            bool(status) and status.startswith("Disconnected ·"),
            "connection-status value=" + (status or "absent"),
        ))
        checks.append(expect_composer_value(tree, expect_value or "", "draft-retained"))
        placeholder = composer_value(tree)
        checks.append(check(
            "draft-not-placeholder",
            placeholder not in (PLACEHOLDER_SESSION, PLACEHOLDER_NO_SESSION, PLACEHOLDER_DISCONNECTED),
            "composer-input value=" + repr(placeholder),
        ))
    elif phase == "reconnected-draft":
        status = tree["connection_status"]
        checks.append(check(
            "connection-status-connected",
            bool(status) and "Connected" in status and not status.startswith("Disconnected"),
            "connection-status value=" + (status or "absent"),
        ))
        checks.append(expect_composer_value(tree, expect_value or "", "draft-survived-reconnect"))
        checks.append(expect_send(tree, True))
    elif phase == "draft-a":
        checks.append(expect_selected(tree, SESSION_A))
        checks.append(expect_composer_value(tree, expect_value or "draft-A", "draft-a"))
    elif phase == "draft-b-empty":
        checks.append(expect_selected(tree, SESSION_B))
        actual = composer_value(tree)
        checks.append(check(
            "draft-b-empty",
            actual in ("", PLACEHOLDER_SESSION),
            "composer-input value=" + repr(actual) + " (expect empty or placeholder)",
        ))
    elif phase == "draft-b":
        checks.append(expect_selected(tree, SESSION_B))
        checks.append(expect_composer_value(tree, expect_value or "draft-B", "draft-b"))
    elif phase == "draft-a-restored":
        checks.append(expect_selected(tree, SESSION_A))
        checks.append(expect_composer_value(tree, expect_value or "draft-A", "draft-a-restored"))
    elif phase == "paste-complete":
        checks.append(expect_send(tree, True))
        expected = expect_value or ""
        actual = composer_value(tree) or ""
        checks.append(check(
            "paste-complete",
            values_match(actual, expected),
            "composer-input value=" + repr(actual)
            + " (expect prefix/full of " + str(len(expected.encode("utf-8"))) + " bytes)",
        ))
        reconstructed = ax_unescape(actual)
        checks.append(check(
            "paste-min-prefix",
            expected.startswith(reconstructed) and len(reconstructed) >= 80,
            "reconstructed prefix chars=" + str(len(reconstructed))
            + " expected bytes=" + str(len(expected.encode("utf-8"))),
        ))
    elif phase == "paste-echoed":
        needle = expect_contains or expect_value or ""
        checks.append(expect_timeline_value(tree, needle, "paste-timeline-echo"))
    elif phase == "multiline-echoed":
        needle = expect_contains or expect_value or ""
        checks.append(expect_timeline_value(tree, needle, "multiline-timeline-echo"))
    elif phase == "hang-cancelable":
        checks.append(expect_selected(tree, SESSION_A))
        checks.append(expect_cancel(tree, True))
        checks.append(expect_send(tree, None))
    elif phase == "model-menu-open":
        checks.append(check(
            "model-menu-present",
            MODEL_MENU in tree["identifiers"],
            "model-menu " + ("present" if MODEL_MENU in tree["identifiers"] else "missing"),
        ))
        options = model_option_ids(tree)
        checks.append(check(
            "model-menu-has-rows",
            True,
            "model options=" + (",".join(options) or "none")
            + " count=" + str(len(options)),
        ))
    elif phase == "model-menu-closed":
        checks.append(check(
            "model-menu-absent",
            MODEL_MENU not in tree["identifiers"],
            "model-menu " + ("present" if MODEL_MENU in tree["identifiers"] else "absent"),
        ))
        picker = node(tree, MODEL_PICKER)
        checks.append(check(
            "model-picker-present",
            picker is not None,
            "model-picker " + ("present value=" + picker["value"] if picker else "missing"),
        ))
    elif phase == "model-trigger-changed":
        picker = node(tree, MODEL_PICKER)
        actual = picker["value"] if picker else None
        checks.append(check(
            "model-trigger-changed",
            expect_value is not None and actual == expect_value,
            "model-picker value=" + repr(actual) + " (expect " + repr(expect_value) + ")",
        ))
        checks.append(check(
            "model-menu-absent",
            MODEL_MENU not in tree["identifiers"],
            "model-menu " + ("present" if MODEL_MENU in tree["identifiers"] else "absent"),
        ))
    elif phase == "narrow-reachable":
        if frames is None:
            raise ValueError("narrow-reachable requires frames")
        root = frames.get("pawork-root")
        composer = frames.get("composer")
        send = frames.get("send") or frames.get("cancel")
        checks.append(check(
            "root-1080x720",
            root is not None and not root.get("error")
            and near(root["w"], 1080.0, 8.0) and near(root["h"], 720.0, 8.0),
            "pawork-root "
            + (
                "w=" + str(root.get("w")) + " h=" + str(root.get("h"))
                if root else "missing"
            ),
        ))
        checks.append(check(
            "composer-present",
            composer is not None and not composer.get("error")
            and composer.get("w", 0) > 0 and composer.get("h", 0) > 0,
            "composer frame=" + str(composer),
        ))
        checks.append(check(
            "send-or-cancel-present",
            send is not None and not send.get("error")
            and send.get("w", 0) > 0 and send.get("h", 0) > 0,
            "action slot frame=" + str(send),
        ))
        checks.append(check(
            "composer-within-root",
            rect_within(composer, root),
            "composer=" + str(composer) + " root=" + str(root),
        ))
        checks.append(check(
            "action-within-root",
            rect_within(send, root),
            "action=" + str(send) + " root=" + str(root),
        ))
        checks.append(check(
            "action-within-composer",
            rect_within(send, composer, slop=2.0),
            "action=" + str(send) + " composer=" + str(composer),
        ))
    elif phase == "keyboard-typed":
        checks.append(expect_send(tree, True))
        checks.append(expect_cancel(tree, None))
        if expect_value is not None:
            checks.append(expect_composer_value(tree, expect_value, "keyboard-typed"))
        elif expect_contains is not None:
            checks.append(expect_composer_contains(tree, expect_contains, "keyboard-typed"))
    elif phase == "keyboard-newline":
        checks.append(expect_send(tree, True))
        checks.append(expect_cancel(tree, None))
        actual = composer_value(tree) or ""
        checks.append(check(
            "shift-return-newline",
            ("\n" in actual) or ("\\n" in actual),
            "composer-input value=" + repr(actual),
        ))
        if expect_value is not None:
            checks.append(expect_composer_value(tree, expect_value, "keyboard-newline-value"))
    elif phase == "keyboard-sent":
        checks.append(expect_timeline_value(
            tree, expect_contains or expect_value or "", "keyboard-timeline-echo",
        ))
    elif phase == "keyboard-cancelled":
        checks.append(expect_send(tree, False))
        checks.append(expect_cancel(tree, None))
    return checks


def write_assert_payload(out_path, phase, checks):
    payload = {
        "phase": phase,
        "generated_at": now_iso(),
        "checks": checks,
        "pass": all(item["pass"] for item in checks),
    }
    Path(out_path).write_text(json.dumps(payload, indent=2) + NL, "utf-8")
    for item in checks:
        prefix = "PASS " if item["pass"] else "FAIL "
        print(prefix + item["name"] + " - " + item["detail"])
    return 0 if payload["pass"] else 5


def cmd_assert(args):
    expect_value = args.expect_value
    if args.expect_value_file:
        expect_value = Path(args.expect_value_file).read_text("utf-8")
    frames = parse_frames(args.frames) if args.frames else None
    tree = parse_composer_tree(args.tree)
    try:
        checks = phase_checks(
            tree,
            args.phase,
            expect_value=expect_value,
            expect_contains=args.expect_contains,
            frames=frames,
        )
    except ValueError as error:
        print("error: " + str(error), file=sys.stderr)
        return 2
    return write_assert_payload(args.out, args.phase, checks)


def cmd_scenes(_args):
    print(json.dumps(
        [{"id": sid, "phase": phase, "summary": summary} for sid, phase, summary in SCENES],
        indent=2,
    ))
    return 0


def build_parser():
    parser = argparse.ArgumentParser(prog="test_ui_r5_wave_b_states.py")
    sub = parser.add_subparsers(dest="cmd", required=True)
    asrt = sub.add_parser("assert")
    asrt.add_argument("--tree", required=True)
    asrt.add_argument("--phase", required=True)
    asrt.add_argument("--out", required=True)
    asrt.add_argument("--expect-value")
    asrt.add_argument("--expect-value-file")
    asrt.add_argument("--expect-contains")
    asrt.add_argument("--frames")
    asrt.set_defaults(func=cmd_assert)
    scenes = sub.add_parser("scenes")
    scenes.set_defaults(func=cmd_scenes)
    return parser


def main(argv=None):
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


# --------------------------------------------------------------------------- tests

SHELL = [
    'role=AXGroup identifier="pawork-root"',
    'role=AXGroup identifier="task-rail"',
    'role=AXGroup identifier="timeline"',
    'role=AXGroup identifier="composer"',
]


def session_row(identifier, selected=False):
    line = 'role=AXRow identifier="' + identifier + '" help="Session" enabled=1'
    if selected:
        line += " selected=1"
    return line


def button(identifier, enabled, value=None):
    line = 'role=AXButton identifier="' + identifier + '" enabled=' + str(enabled)
    if value is not None:
        line = (
            'role=AXButton value="' + value + '" identifier="' + identifier
            + '" enabled=' + str(enabled)
        )
    if enabled:
        line += " actions=[AXPress]"
    return line


def textarea(value, enabled=1):
    prefix = "role=AXTextArea"
    if value:
        prefix += ' value="' + value + '"'
    return prefix + ' identifier="composer-input" description="Message" enabled=' + str(enabled)


def timeline_row(identifier, value):
    return (
        'role=AXRow value="' + value + '" identifier="' + identifier
        + '" description="You" enabled=1'
    )


def status(value):
    return 'role=AXStaticText value="' + value + '" identifier="connection-status" enabled=1'


class ComposerAssertTest(unittest.TestCase):
    def assert_tree(self, lines, phase, extra=None, frames_text=None):
        with tempfile.TemporaryDirectory() as raw:
            tree = Path(raw) / "tree.txt"
            out = Path(raw) / "assert.json"
            tree.write_text("\n".join(lines) + "\n", encoding="utf-8")
            cmd = [
                sys.executable, str(Path(__file__)), "assert",
                "--tree", str(tree), "--phase", phase, "--out", str(out),
            ]
            extra = extra or []
            cmd += extra
            if frames_text is not None:
                frames = Path(raw) / "frames.txt"
                frames.write_text(frames_text, encoding="utf-8")
                cmd += ["--frames", str(frames)]
            proc = subprocess.run(cmd, check=False, capture_output=True, text=True)
            payload = json.loads(out.read_text("utf-8")) if out.exists() else {}
            return proc, payload

    def failed(self, payload):
        return [item["name"] for item in payload.get("checks", []) if not item["pass"]]

    def test_empty_input_positive_and_negatives(self):
        lines = SHELL + [
            session_row(SESSION_A, selected=True),
            textarea(""),
            button("send", 0),
        ]
        proc, payload = self.assert_tree(lines, "empty-input")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(payload["pass"])
        enabled = [
            line.replace(
                'identifier="send" enabled=0',
                'identifier="send" enabled=1 actions=[AXPress]',
            )
            for line in lines
        ]
        proc, payload = self.assert_tree(enabled, "empty-input")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("composer-send-enabled-0", self.failed(payload))
        with_cancel = lines + [button("cancel", 0)]
        proc, payload = self.assert_tree(with_cancel, "empty-input")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("composer-cancel-absent", self.failed(payload))
        leftover = [
            line.replace('identifier="composer-input"', 'value="leftover" identifier="composer-input"')
            if "composer-input" in line else line
            for line in lines
        ]
        proc, payload = self.assert_tree(leftover, "empty-input")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("composer-empty-value", self.failed(payload))

    def test_send_enabled_and_running_swap(self):
        ready = SHELL + [
            session_row(SESSION_A, selected=True),
            textarea("hello"),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(ready, "send-enabled", ["--expect-value", "hello"])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        running = SHELL + [
            session_row(SESSION_A, selected=True),
            textarea("Run in progress — sending is disabled. Cancel remains available."),
            button("cancel", 1),
        ]
        proc, payload = self.assert_tree(running, "running-cancelable")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        stray_send = running + [button("send", 0)]
        proc, payload = self.assert_tree(stray_send, "running-cancelable")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("composer-send-absent", self.failed(payload))
        restored = SHELL + [
            session_row(SESSION_A, selected=True),
            textarea(PLACEHOLDER_SESSION),
            button("send", 0),
        ]
        proc, payload = self.assert_tree(restored, "send-restored")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        draft_back = SHELL + [
            session_row(SESSION_A, selected=True),
            textarea("draft-came-back"),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(draft_back, "send-restored")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("composer-send-enabled-0", self.failed(payload))

    def test_disconnected_draft_retains_text_and_disables_send(self):
        lines = SHELL + [
            session_row(SESSION_A, selected=True),
            status("Disconnected · socket dropped"),
            textarea("keep-me"),
            button("send", 0),
            button("reconnect", 1),
        ]
        proc, payload = self.assert_tree(lines, "disconnected-draft", ["--expect-value", "keep-me"])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        lost = [
            line.replace('value="keep-me"', 'value="' + PLACEHOLDER_DISCONNECTED + '"')
            if "composer-input" in line else line
            for line in lines
        ]
        proc, payload = self.assert_tree(lost, "disconnected-draft", ["--expect-value", "keep-me"])
        self.assertEqual(proc.returncode, 5)
        failed = set(self.failed(payload))
        self.assertTrue("draft-retained" in failed or "draft-not-placeholder" in failed)

    def test_reconnected_draft_enables_send(self):
        lines = SHELL + [
            session_row(SESSION_A, selected=True),
            status("Local · Connected"),
            textarea("keep-me"),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(lines, "reconnected-draft", ["--expect-value", "keep-me"])
        self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_draft_isolation_empty_and_restore(self):
        draft_a = SHELL + [
            session_row(SESSION_A, selected=True),
            session_row(SESSION_B),
            textarea("draft-A"),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(draft_a, "draft-a", ["--expect-value", "draft-A"])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        empty_b = SHELL + [
            session_row(SESSION_A),
            session_row(SESSION_B, selected=True),
            textarea(PLACEHOLDER_SESSION),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(empty_b, "draft-b-empty")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        leaked = SHELL + [
            session_row(SESSION_A),
            session_row(SESSION_B, selected=True),
            textarea("draft-A"),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(leaked, "draft-b-empty")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("draft-b-empty", self.failed(payload))
        restored = SHELL + [
            session_row(SESSION_A, selected=True),
            session_row(SESSION_B),
            textarea("draft-A"),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(restored, "draft-a-restored", ["--expect-value", "draft-A"])
        self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_paste_complete_requires_8kb_and_echo(self):
        payload_text = ("中文" * 2000) + ("日本語" * 500) + ("\nlatin " * 200)
        self.assertGreaterEqual(len(payload_text.encode("utf-8")), 8192)
        with tempfile.TemporaryDirectory() as raw:
            value_file = Path(raw) / "paste.txt"
            value_file.write_text(payload_text, encoding="utf-8")
            dumped = ax_escape(payload_text)
            lines = SHELL + [
                session_row(SESSION_A, selected=True),
                textarea(dumped),
                button("send", 1),
            ]
            proc, payload = self.assert_tree(
                lines, "paste-complete", ["--expect-value-file", str(value_file)],
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertTrue(payload["pass"])
        echoed = SHELL + [
            session_row(SESSION_A, selected=True),
            timeline_row("timeline-entry-local-echo-r-1", payload_text[:80]),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(
            echoed, "paste-echoed", ["--expect-contains", payload_text[:40]],
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        missing = SHELL + [
            session_row(SESSION_A, selected=True),
            timeline_row("timeline-entry-local-echo-r-1", "unrelated"),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(
            missing, "paste-echoed", ["--expect-contains", payload_text[:40]],
        )
        self.assertEqual(proc.returncode, 5)
        self.assertIn("paste-timeline-echo", self.failed(payload))


    def test_model_menu_open_close_and_trigger_change(self):
        opened = SHELL + [
            session_row(SESSION_A, selected=True),
            button(MODEL_PICKER, 1, "mock / fixture-model"),
            'role=AXGroup identifier="model-menu" description="Models"',
            button("model-mock_3afixture-model", 1, "mock / fixture-model"),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(opened, "model-menu-open")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        closed = SHELL + [
            session_row(SESSION_A, selected=True),
            button(MODEL_PICKER, 1, "mock / fixture-model"),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(closed, "model-menu-closed")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        still_open = opened
        proc, payload = self.assert_tree(still_open, "model-menu-closed")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("model-menu-absent", self.failed(payload))
        changed = SHELL + [
            session_row(SESSION_A, selected=True),
            button(MODEL_PICKER, 1, "mock / other-model"),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(
            changed, "model-trigger-changed", ["--expect-value", "mock / other-model"],
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_narrow_frames_require_1080x720_and_no_overflow(self):
        lines = SHELL + [
            session_row(SESSION_A, selected=True),
            textarea(PLACEHOLDER_SESSION),
            button("send", 1),
        ]
        good = (
            "id=pawork-root role=AXGroup x=0 y=0 w=1080.0 h=720.0\n"
            "id=composer role=AXGroup x=240 y=620 w=840.0 h=76.0\n"
            "id=send role=AXButton x=1040 y=656 w=32.0 h=32.0\n"
        )
        proc, payload = self.assert_tree(lines, "narrow-reachable", frames_text=good)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        overflow = (
            "id=pawork-root role=AXGroup x=0 y=0 w=1080.0 h=720.0\n"
            "id=composer role=AXGroup x=240 y=620 w=840.0 h=76.0\n"
            "id=send role=AXButton x=1070 y=656 w=40.0 h=32.0\n"
        )
        proc, payload = self.assert_tree(lines, "narrow-reachable", frames_text=overflow)
        self.assertEqual(proc.returncode, 5)
        failed = set(self.failed(payload))
        self.assertTrue("action-within-root" in failed or "action-within-composer" in failed)
        wrong_size = (
            "id=pawork-root role=AXGroup x=0 y=0 w=1440.0 h=1024.0\n"
            "id=composer role=AXGroup x=288 y=900 w=712.0 h=88.0\n"
            "id=send role=AXButton x=960 y=948 w=32.0 h=32.0\n"
        )
        proc, payload = self.assert_tree(lines, "narrow-reachable", frames_text=wrong_size)
        self.assertEqual(proc.returncode, 5)
        self.assertIn("root-1080x720", self.failed(payload))

    def test_keyboard_newline_and_cancel_restore(self):
        newline = SHELL + [
            session_row(SESSION_A, selected=True),
            textarea(ax_escape("ab\n")),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(newline, "keyboard-newline", ["--expect-value", "ab\n"])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        no_nl = SHELL + [
            session_row(SESSION_A, selected=True),
            textarea("ab"),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(no_nl, "keyboard-newline")
        self.assertEqual(proc.returncode, 5)
        self.assertIn("shift-return-newline", self.failed(payload))
        cancelled = SHELL + [
            session_row(SESSION_A, selected=True),
            textarea(PLACEHOLDER_SESSION),
            button("send", 0),
        ]
        proc, payload = self.assert_tree(cancelled, "keyboard-cancelled")
        self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_unknown_phase_is_usage_error(self):
        proc, payload = self.assert_tree(SHELL, "not-a-phase")
        self.assertEqual(proc.returncode, 2)
        self.assertIn("unknown composer phase", proc.stderr)

    def test_scenes_table_lists_r5_1_through_r5_9(self):
        proc = subprocess.run(
            [sys.executable, str(Path(__file__)), "scenes"],
            check=False, capture_output=True, text=True,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        rows = json.loads(proc.stdout)
        ids = [row["id"] for row in rows]
        self.assertEqual(
            ids,
            ["r5-1", "r5-2", "r5-3", "r5-4", "r5-5", "r5-6", "r5-7", "r5-8", "r5-9"],
        )
        self.assertEqual(len(SCENES), 9)
        self.assertEqual(len(PHASES), 23)

    def test_hang_cancelable_requires_cancel_and_hides_send(self):
        ok = SHELL + [
            session_row(SESSION_A, selected=True),
            button("cancel", 1),
        ]
        proc, payload = self.assert_tree(ok, "hang-cancelable")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        idle = SHELL + [
            session_row(SESSION_A, selected=True),
            button("send", 1),
        ]
        proc, payload = self.assert_tree(idle, "hang-cancelable")
        self.assertEqual(proc.returncode, 5)
        failed = set(self.failed(payload))
        self.assertTrue("composer-cancel-enabled-1" in failed)


class DriverGuardTest(unittest.TestCase):
    def test_driver_declares_nine_scenes_and_composer_phases(self):
        text = DRIVER.read_text("utf-8")
        for scene in ("r5-1", "r5-2", "r5-3", "r5-4", "r5-5", "r5-6", "r5-7", "r5-8", "r5-9"):
            self.assertIn(scene, text)
        for phase in (
            "empty-input", "running-cancelable", "disconnected-draft",
            "draft-a-restored", "paste-complete", "hang-cancelable",
            "model-menu-open", "narrow-reachable", "keyboard-newline",
        ):
            self.assertIn(phase, text)
        self.assertIn("drop-socket", text)
        self.assertIn("pbcopy", text)
        self.assertIn("cmd,v", text)
        self.assertIn("1080x720", text)
        self.assertIn("shot-hang-cancelable.png", text)
        self.assertIn("ui-key-event", text)
        self.assertIn("shift", text)
        self.assertIn("cmd,.", text)
        self.assertNotRegex(text, r"^\s*sleep\s+[1-9]", re.M)
        self.assertIn("--label", text)
        self.assertIn("action-trace.txt", text)
        self.assertIn("run-manifest.json", text)

    def test_driver_rejects_nonempty_output_to_prevent_stale_evidence(self):
        with tempfile.TemporaryDirectory() as raw:
            out = Path(raw) / "evidence"
            out.mkdir()
            (out / "stale.json").write_text("{}\n", encoding="utf-8")
            proc = subprocess.run(
                ["bash", str(DRIVER), "run", "--out", str(out)],
                check=False, capture_output=True, text=True,
            )
            self.assertEqual(proc.returncode, 3, proc.stdout + proc.stderr)
            self.assertIn("must be new or empty", proc.stderr)
            self.assertTrue((out / "stale.json").exists())

    def test_driver_usage_requires_run_and_out(self):
        proc = subprocess.run(
            ["bash", str(DRIVER)],
            check=False, capture_output=True, text=True,
        )
        self.assertEqual(proc.returncode, 2)
        proc = subprocess.run(
            ["bash", str(DRIVER), "run"],
            check=False, capture_output=True, text=True,
        )
        self.assertEqual(proc.returncode, 2)
        proc = subprocess.run(
            ["bash", str(DRIVER), "run", "--out", "/tmp/x", "--unknown"],
            check=False, capture_output=True, text=True,
        )
        self.assertEqual(proc.returncode, 2)

    def test_cli_parse_unknown_phase_and_scenes_json(self):
        parser = build_parser()
        args = parser.parse_args(["scenes"])
        self.assertEqual(args.cmd, "scenes")
        with self.assertRaises(SystemExit):
            parser.parse_args(["assert"])
        args = parser.parse_args([
            "assert", "--tree", "t.txt", "--phase", "empty-input", "--out", "o.json",
        ])
        self.assertEqual(args.phase, "empty-input")

    def test_driver_is_idempotent_run_out_contract(self):
        text = DRIVER.read_text("utf-8")
        self.assertIn('run) MODE="$1"', text)
        self.assertIn("--out)", text)
        self.assertIn("must be new or empty", text)
        self.assertIn("fixture_teardown", text)
        self.assertIn("copy_runtime_evidence", text)

    def test_driver_is_bash_3_safe_and_isolates_keyboard_model_state(self):
        text = DRIVER.read_text("utf-8")
        self.assertNotIn("mapfile", text)
        self.assertIn("--pin-ascii-input-source", text)
        self.assertIn("--restore-input-source", text)
        self.assertIn("MULTILINE=$'fixture:hang", text)
        self.assertIn('composer_set_value "" "$OUT/action-set-value-clear-paste.txt"', text)
        calls = text[text.index("scene_r5_1\n") :]
        self.assertLess(calls.index("scene_r5_9\n"), calls.index("scene_r5_7\n"))


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] in {"assert", "scenes"}:
        sys.exit(main())
    unittest.main()
