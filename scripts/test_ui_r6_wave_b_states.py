#!/usr/bin/env python3
"""R6 Wave B U1/U2 AX contracts and driver guards.

The fixed UI fixture plus explicit dev-only Host profiles prove the complete
Changes/Terminal/Resources/Inspector lifecycle without changing production
Policy or GUI wire contracts.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

DRIVER = Path(__file__).with_name("ui-r6-wave-b-states.sh")
IDENTIFIER_RE = re.compile(r'identifier="([^"]*)"')
VALUE_RE = re.compile(r'value="([^"]*)"')
DESCRIPTION_RE = re.compile(r'description="([^"]*)"')
HELP_RE = re.compile(r'help="([^"]*)"')

SCENES = {
    "c1": "real four-file Changes list from the existing alpha fixture",
    "c2": "clean beta task produces a real empty Changes result",
    "c3": "Changes Files/Summary and file-row keyboard path",
    "t1": "Terminal create/write/output/resize contract",
    "i1": "Inspector collapse/Activity restore and focus contract",
    "s1": "latest-session Changes scope remains honest after task switch",
    "d1": "disconnect/reconnect keeps terminal state honest and disables writes",
    "r1": "Resources empty plus connected/failed MCP Host profile",
    "t2": "read_only Host profile rejects terminal create fail-closed",
}
PHASES = (
    "c1-files", "c2-empty", "c3-summary", "c3-file-focus", "t1-idle", "t1-ready",
    "t1-resized", "t1-output", "i1-collapsed", "i1-restored",
    "s1-latest-scope", "d1-disconnected", "d1-reconnected",
    "r1-empty", "r1-matrix", "profile-disconnected", "t2-denied",
)


def now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_tree(path: Path) -> dict:
    nodes = {}
    for line in path.read_text("utf-8").splitlines():
        if line.lstrip().startswith("#"):
            continue
        ids = IDENTIFIER_RE.findall(line)
        if not ids:
            continue
        values = VALUE_RE.findall(line)
        descriptions = DESCRIPTION_RE.findall(line)
        help_texts = HELP_RE.findall(line)
        nodes[ids[0]] = {
            "value": values[0] if values else "",
            "description": descriptions[0] if descriptions else "",
            "help": help_texts[0] if help_texts else "",
            "selected": "selected=1" in line,
            "focused": "focused=1" in line,
            "enabled": None if "enabled=" not in line else "enabled=1" in line,
            "line": line,
        }
    return nodes


def check(name: str, passed: bool, detail: str) -> dict:
    return {"name": name, "pass": bool(passed), "detail": detail}


def present(nodes: dict, identifier: str) -> dict:
    return check(identifier + "-present", identifier in nodes,
                 identifier + (" present" if identifier in nodes else " missing"))


def selected(nodes: dict, identifier: str) -> dict:
    node = nodes.get(identifier)
    return check(identifier + "-selected", bool(node and node["selected"]),
                 identifier + " selected=" + str(node["selected"] if node else None))


def focused(nodes: dict, identifier: str) -> dict:
    node = nodes.get(identifier)
    return check(identifier + "-focused", bool(node and node["focused"]),
                 identifier + " focused=" + str(node["focused"] if node else None))


def phase_checks(nodes: dict, phase: str) -> list[dict]:
    checks: list[dict] = []
    if phase == "c1-files":
        checks += [selected(nodes, "inspector-tab-changes"), selected(nodes, "changes-tab-files")]
        expected = {
            "changes-file-src_2fmain.rs": "modified",
            "changes-file-docs_2freport.md": "modified",
            "changes-file-src_2fnew_5ffeature.rs": "untracked",
            "changes-file-legacy_2fold.txt": "deleted",
        }
        for identifier, status in expected.items():
            node = nodes.get(identifier)
            value = node["value"] if node else ""
            canonical = re.fullmatch(
                re.escape(status) + r" · \+[0-9]+ / −[0-9]+", value,
            ) is not None
            checks.append(check(identifier + "-canonical", canonical,
                                repr(value) + " expect canonical " + status + " counters"))
        rows = [key for key in nodes if key.startswith("changes-file-") and key != "changes-file-list"]
        checks.append(check("changes-exactly-four", len(rows) == 4, "rows=" + repr(sorted(rows))))
        for forbidden in ("stage", "unstage", "hunk", "add-tool"):
            checks.append(check("no-fake-" + forbidden, forbidden not in nodes, forbidden + " absent"))
    elif phase == "c2-empty":
        checks += [selected(nodes, "inspector-tab-changes"), selected(nodes, "changes-tab-files"),
                   present(nodes, "changes-file-list")]
        fetch_help = nodes.get("changes", {}).get("help", "")
        rows = [key for key in nodes if key.startswith("changes-file-") and key != "changes-file-list"]
        checks.append(check("changes-ready-empty", "ready · 0 files" in fetch_help,
                            repr(fetch_help)))
        checks.append(check("changes-no-file-rows", not rows, "rows=" + repr(sorted(rows))))
    elif phase == "c3-summary":
        checks += [selected(nodes, "inspector-tab-changes"), selected(nodes, "changes-tab-summary"),
                   focused(nodes, "changes-tab-summary")]
    elif phase == "c3-file-focus":
        checks += [selected(nodes, "changes-tab-files"), focused(nodes, "changes-file-src_2fnew_5ffeature.rs"),
                   selected(nodes, "changes-file-src_2fnew_5ffeature.rs")]
    elif phase == "t1-idle":
        checks += [selected(nodes, "inspector-tab-terminal"), present(nodes, "terminal"),
                   present(nodes, "terminal-input"), present(nodes, "terminal-start")]
        node = nodes.get("terminal-start")
        checks.append(check("terminal-live-enabled", bool(node and node["enabled"] is True),
                            "terminal-start enabled=" + str(node["enabled"] if node else None)))
        for forbidden in ("terminal-stop", "terminal-close", "terminal-kill"):
            checks.append(check("no-fake-" + forbidden, forbidden not in nodes, forbidden + " absent"))
    elif phase == "t1-ready":
        checks += [selected(nodes, "inspector-tab-terminal"), present(nodes, "terminal-output")]
        start = nodes.get("terminal-start")
        resize = nodes.get("terminal-resize")
        checks.append(check("terminal-created", bool(start and "Apply terminal size" in start["description"]),
                            repr(start["description"] if start else None)))
        checks.append(check("terminal-resize-enabled", bool(resize and resize["enabled"] is True),
                            "terminal-resize enabled=" + str(resize["enabled"] if resize else None)))
        description = nodes.get("terminal-output", {}).get("description", "")
        checks.append(check("terminal-resize-not-yet-confirmed",
                            "resize confirmed" not in description, repr(description)))
    elif phase == "t1-resized":
        checks += [selected(nodes, "inspector-tab-terminal"), present(nodes, "terminal-output")]
        terminal_help = nodes.get("terminal-output", {}).get("help", "")
        checks.append(check("terminal-resize-receipt", "resize confirmed" in terminal_help,
                            repr(terminal_help)))
    elif phase == "t1-output":
        checks += [selected(nodes, "inspector-tab-terminal"), present(nodes, "terminal-output")]
        output_line = nodes.get("terminal-output", {}).get("line", "")
        checks.append(check("terminal-output-marker", "pawork-r6b-t1" in output_line,
                            repr(output_line[-240:])))
        terminal_input = nodes.get("terminal-input", {}).get("value", "")
        checks.append(check("terminal-input-cleared-after-write-receipt",
                            terminal_input == "", repr(terminal_input)))
        start = nodes.get("terminal-start")
        checks.append(check("terminal-created", bool(start and "Apply terminal size" in start["description"]),
                            repr(start["description"] if start else None)))
    elif phase == "i1-collapsed":
        checks.append(check("inspector-absent", "inspector" not in nodes, "inspector absent"))
        checks += [present(nodes, "inspector-toggle"), focused(nodes, "inspector-toggle")]
    elif phase == "i1-restored":
        checks += [present(nodes, "inspector"), selected(nodes, "inspector-tab-changes")]
    elif phase == "s1-latest-scope":
        checks += [selected(nodes, "session-fx-ses-alpha-yesterday"), selected(nodes, "inspector-tab-changes")]
        rows = [key for key in nodes if key.startswith("changes-file-") and key != "changes-file-list"]
        checks.append(check("latest-scope-four-alpha-files", len(rows) == 4, "rows=" + repr(sorted(rows))))
    elif phase == "d1-disconnected":
        status = nodes.get("connection-status", {}).get("value", "")
        checks.append(check("disconnected-status", "Disconnected" in status, repr(status)))
        start = nodes.get("terminal-start")
        checks.append(check("terminal-write-disabled", bool(start and start["enabled"] is False),
                            "terminal-start enabled=" + str(start["enabled"] if start else None)))
        terminal_input = nodes.get("terminal-input", {}).get("value", "")
        output = nodes.get("terminal-output", {}).get("value", "")
        checks.append(check("terminal-draft-kept-while-disconnected",
                            terminal_input == "pawork-r6b-d1", repr(terminal_input)))
        checks.append(check("terminal-disconnected-write-not-sent",
                            "pawork-r6b-d1" not in output, repr(output[-240:])))
    elif phase == "d1-reconnected":
        status = nodes.get("connection-status", {}).get("value", "")
        checks.append(check("connected-status", "Connected" in status, repr(status)))
        checks += [selected(nodes, "inspector-tab-terminal"), present(nodes, "terminal-output")]
        terminal_help = nodes.get("terminal-output", {}).get("help", "")
        checks.append(check("terminal-reconnect-state-honest",
                            "workspace " in terminal_help and " · running" in terminal_help
                            and "stale" not in terminal_help and "failed" not in terminal_help,
                            repr(terminal_help)))
    elif phase == "r1-empty":
        checks += [selected(nodes, "inspector-tab-resources"), present(nodes, "resources-refresh"),
                   present(nodes, "mcp-server-list")]
        fetch_help = nodes.get("resources", {}).get("help", "")
        rows = [key for key in nodes if key.startswith("mcp-server-") and key != "mcp-server-list"]
        checks.append(check("resources-ready-empty", "ready · 0 servers" in fetch_help,
                            repr(fetch_help)))
        checks.append(check("resources-no-server-rows", not rows, "rows=" + repr(sorted(rows))))
    elif phase == "r1-matrix":
        checks += [selected(nodes, "inspector-tab-resources"), present(nodes, "resources-refresh")]
        fetch_help = nodes.get("resources", {}).get("help", "")
        ready = nodes.get("mcp-server-fixture-files")
        failed = nodes.get("mcp-server-fixture-broken")
        checks.append(check("resources-ready-two", "ready · 2 servers" in fetch_help,
                            repr(fetch_help)))
        checks.append(check("resources-connected-server",
                            bool(ready and "connected · stdio · 2 tools" in ready["value"]),
                            repr(ready)))
        checks.append(check("resources-failed-server",
                            bool(failed and "failed · stdio · 0 tools" in failed["value"]
                                 and "fixture scripted MCP startup failure" in failed["help"]),
                            repr(failed)))
    elif phase == "profile-disconnected":
        status = nodes.get("connection-status", {}).get("value", "")
        checks.append(check("profile-disconnected-status", "Disconnected" in status, repr(status)))
        checks.append(present(nodes, "reconnect"))
    elif phase == "t2-denied":
        checks += [selected(nodes, "inspector-tab-terminal"), present(nodes, "terminal-output")]
        terminal_line = nodes.get("terminal-output", {}).get("line", "")
        start = nodes.get("terminal-start")
        checks.append(check("terminal-read-only-failed",
                            "failed" in terminal_line and "read_only" in terminal_line
                            and "fail-closed" in terminal_line,
                            repr(terminal_line)))
        checks.append(check("terminal-retry-remains-honest",
                            bool(start and start["description"] == "Start terminal"
                                 and start["enabled"] is True), repr(start)))
        for forbidden in ("terminal-stop", "terminal-close", "terminal-kill"):
            checks.append(check("no-fake-" + forbidden, forbidden not in nodes, forbidden + " absent"))
    else:
        raise ValueError("unknown phase " + phase)
    return checks


def assert_phase(tree: Path, phase: str, out: Path) -> int:
    checks = phase_checks(parse_tree(tree), phase)
    payload = {"phase": phase, "generated_at": now_iso(), "checks": checks,
               "pass": all(item["pass"] for item in checks)}
    out.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", "utf-8")
    return 0 if payload["pass"] else 5


def fixture_profiles() -> dict:
    return {
        "c2": "available: clean beta task",
        "r1": "available: r6-resources",
        "t2": "available: r6-read-only",
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    command = sub.add_parser("assert")
    command.add_argument("--tree", type=Path, required=True)
    command.add_argument("--phase", choices=PHASES, required=True)
    command.add_argument("--out", type=Path, required=True)
    sub.add_parser("matrix")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.command == "matrix":
        print(json.dumps({"scenes": SCENES, "profiles": fixture_profiles()}, ensure_ascii=False, indent=2))
        return 0
    return assert_phase(args.tree, args.phase, args.out)


def line(identifier: str, *, value="", description="", help="", selected=False,
         focused=False, enabled=None) -> str:
    fields = [f'identifier="{identifier}"', f'value="{value}"', f'description="{description}"']
    if help:
        fields.append(f'help="{help}"')
    if selected:
        fields.append("selected=1")
    if focused:
        fields.append("focused=1")
    if enabled is not None:
        fields.append("enabled=" + ("1" if enabled else "0"))
    return " ".join(fields)


class ContractTest(unittest.TestCase):
    def run_phase(self, phase: str, lines: list[str]):
        with tempfile.TemporaryDirectory() as raw:
            tree, out = Path(raw) / "tree.txt", Path(raw) / "out.json"
            tree.write_text("\n".join(lines) + "\n", "utf-8")
            rc = assert_phase(tree, phase, out)
            return rc, json.loads(out.read_text("utf-8"))

    def test_c1_requires_canonical_four_files_and_no_fake_actions(self):
        rows = [
            line("inspector-tab-changes", selected=True), line("changes-tab-files", selected=True),
            line("changes-file-src_2fmain.rs", value="modified · +2 / −3"),
            line("changes-file-docs_2freport.md", value="modified · +3 / −4"),
            line("changes-file-src_2fnew_5ffeature.rs", value="untracked · +3 / −0"),
            line("changes-file-legacy_2fold.txt", value="deleted · +0 / −5"),
        ]
        self.assertEqual(self.run_phase("c1-files", rows)[0], 0)
        self.assertEqual(self.run_phase("c1-files", rows[:-1])[0], 5)

    def test_terminal_contract_refuses_fake_stop_and_requires_honest_reconnect_state(self):
        idle = [line("inspector-tab-terminal", selected=True), line("terminal"),
                line("terminal-input"), line("terminal-start", enabled=True)]
        self.assertEqual(self.run_phase("t1-idle", idle)[0], 0)
        self.assertEqual(self.run_phase("t1-idle", idle + [line("terminal-stop")])[0], 5)
        ready = [line("inspector-tab-terminal", selected=True), line("terminal-output"),
                 line("terminal-start", description="Apply terminal size"),
                 line("terminal-resize", enabled=True)]
        self.assertEqual(self.run_phase("t1-ready", ready)[0], 0)
        resized = [line("inspector-tab-terminal", selected=True),
                   line("terminal-output", help="workspace ws-1 · . · 80×24 · running · resize confirmed")]
        self.assertEqual(self.run_phase("t1-resized", resized)[0], 0)
        output = [line("inspector-tab-terminal", selected=True),
                  line("terminal-output", value="hello pawork-r6b-t1"),
                  line("terminal-input", value=""),
                  line("terminal-start", description="Apply terminal size")]
        self.assertEqual(self.run_phase("t1-output", output)[0], 0)
        output[1] = line("terminal-output", value="hello pawork-r6b-t1")
        output[2] = line("terminal-input", value="printf pawork-r6b-t1")
        output.append(line("terminal-start", description="Apply terminal size"))
        self.assertEqual(self.run_phase("t1-output", output)[0], 5)
        disconnected = [line("connection-status", value="Disconnected"),
                        line("terminal-start", enabled=False),
                        line("terminal-input", value="pawork-r6b-d1"),
                        line("terminal-output", value="old output")]
        self.assertEqual(self.run_phase("d1-disconnected", disconnected)[0], 0)
        reconnected = [line("inspector-tab-terminal", selected=True),
                       line("terminal-output", help="workspace ws-1 · . · 80×24 · running"),
                       line("connection-status", value="Connected")]
        self.assertEqual(self.run_phase("d1-reconnected", reconnected)[0], 0)

    def test_empty_changes_resources_and_read_only_contracts(self):
        empty_changes = [line("inspector-tab-changes", selected=True),
                         line("changes-tab-files", selected=True),
                         line("changes", help="scope · ready · 0 files"),
                         line("changes-file-list")]
        self.assertEqual(self.run_phase("c2-empty", empty_changes)[0], 0)

        resources_empty = [line("inspector-tab-resources", selected=True),
                           line("resources", help="Host MCP servers · ready · 0 servers"),
                           line("resources-refresh"), line("mcp-server-list")]
        self.assertEqual(self.run_phase("r1-empty", resources_empty)[0], 0)
        resources_matrix = [line("inspector-tab-resources", selected=True),
                            line("resources", help="Host MCP servers · ready · 2 servers"),
                            line("resources-refresh"),
                            line("mcp-server-fixture-files", value="connected · stdio · 2 tools"),
                            line("mcp-server-fixture-broken", value="failed · stdio · 0 tools",
                                 help="fixture scripted MCP startup failure")]
        self.assertEqual(self.run_phase("r1-matrix", resources_matrix)[0], 0)

        denied = [line("inspector-tab-terminal", selected=True),
                  line("terminal-output", description="Terminal output",
                       value="", help="workspace fx-beta-lib · failed · read_only · fail-closed"),
                  line("terminal-start", description="Start terminal", enabled=True)]
        self.assertEqual(self.run_phase("t2-denied", denied)[0], 0)

        self.assertEqual(
            fixture_profiles(),
            {
                "c2": "available: clean beta task",
                "r1": "available: r6-resources",
                "t2": "available: r6-read-only",
            },
        )


class DriverGuardTest(unittest.TestCase):
    def test_driver_has_matrix_barriers_ax_fallback_and_bounded_restart(self):
        text = DRIVER.read_text("utf-8")
        for scene in SCENES:
            self.assertIn(scene, text)
        for phase in PHASES:
            self.assertIn(phase, text)
        self.assertIn('local phase="$1"', text)
        self.assertIn("subprocess.Popen", text)
        self.assertIn("TimeoutExpired", text)
        self.assertIn("killpg", text)
        self.assertIn("focus_by_keys", text)
        self.assertIn("recover_ax_for_keyboard_focus", text)
        self.assertIn("AX_RECOVERY_ALLOWED=0", text)
        self.assertIn("timeline_stable", text)
        self.assertIn("pick_python", text)
        self.assertIn("PAWORK_WAVE_D_PYTHON", text)
        self.assertIn("ax-fallback=axwindows", text)
        self.assertIn("desktop-restart", text)
        self.assertIn("r6-terminal", text)
        self.assertIn("r6-resources", text)
        self.assertIn("r6-read-only", text)
        self.assertIn("project-add-Earlier_3afx-beta-lib", text)
        self.assertIn("screencapture", text)
        self.assertIn("3", text)
        self.assertNotRegex(text, r"^\s*sleep\s+[1-9]", re.M)
        self.assertIn("exit 4", text)

    def test_nonempty_output_is_rejected_without_deletion(self):
        with tempfile.TemporaryDirectory() as raw:
            out = Path(raw) / "evidence"
            out.mkdir()
            marker = out / "keep"
            marker.write_text("user", "utf-8")
            proc = subprocess.run(["bash", str(DRIVER), "run", "--out", str(out)],
                                  text=True, capture_output=True, check=False)
            self.assertEqual(proc.returncode, 3, proc.stdout + proc.stderr)
            self.assertTrue(marker.exists())

    def test_usage_is_fail_closed(self):
        self.assertEqual(subprocess.run(["bash", str(DRIVER)], check=False).returncode, 2)
        self.assertEqual(subprocess.run(["bash", str(DRIVER), "run"], check=False).returncode, 2)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] in {"assert", "matrix"}:
        raise SystemExit(main())
    unittest.main()
