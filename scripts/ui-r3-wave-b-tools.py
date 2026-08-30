#!/usr/bin/env python3
"""R3 Wave B helpers: AX tree parsing + keyboard-navigation phase assertions."""

from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

IDENTIFIER_RE = re.compile(r'identifier="([^"]*)"')
HELP_RE = re.compile(r'help="([^"]*)"')
VALUE_RE = re.compile(r'value="([^"]*)"')
NL = chr(10)

SEED_ROWS = [
    "session-fx-ses-alpha-today",
    "session-fx-ses-alpha-yesterday",
    "session-fx-ses-alpha-longtitle",
    "session-fx-ses-beta-pending",
    "session-fx-ses-beta-toolfailed",
    "session-fx-ses-beta-long",
    "session-fx-ses-beta-cancelled",
]


def now_iso():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_tree(path):
    """Parse a ui-ax-dump tree into rows / focus / selection / rail facts."""
    lines = [
        line
        for line in Path(path).read_text("utf-8").splitlines()
        if not line.lstrip().startswith("#")
    ]
    identifiers = set()
    focused = []
    rows = {}
    grouping_value = ""
    connection_status = ""
    for line in lines:
        ids = IDENTIFIER_RE.findall(line)
        if not ids:
            continue
        identifier = ids[0]
        identifiers.add(identifier)
        if "focused=1" in line:
            focused.append(identifier)
        helps = HELP_RE.findall(line)
        values = VALUE_RE.findall(line)
        if identifier.startswith("session-") and identifier != "session-list":
            rows[identifier] = {
                "help": helps[0] if helps else "",
                "value": values[0] if values else "",
                "selected": "selected=1" in line,
            }
        if identifier == "task-rail-grouping":
            grouping_value = values[0] if values else ""
        if identifier == "connection-status":
            connection_status = values[0] if values else ""
    timeline_counts = {}
    for identifier in identifiers:
        match = re.match(r"^timeline-entry-evt-(fx-ses-[a-z-]+)-", identifier)
        if match:
            key = match.group(1)
            timeline_counts[key] = timeline_counts.get(key, 0) + 1
    return {
        "identifiers": identifiers,
        "focused": focused,
        "rows": rows,
        "grouping_value": grouping_value,
        "connection_status": connection_status,
        "timeline_counts": timeline_counts,
    }


def check(name, ok, detail):
    return {"name": name, "pass": bool(ok), "detail": detail}


def selected_rows(tree):
    return [row for row, facts in tree["rows"].items() if facts["selected"]]


def assert_single_selected(tree, phase, expected):
    selected = selected_rows(tree)
    return check(
        "selected-" + phase,
        selected == [expected],
        "selected rows: " + (",".join(selected) or "none") + " (expect " + expected + ")",
    )


def assert_timeline_loaded(tree, session):
    count = tree["timeline_counts"].get(session, 0)
    return check(
        "timeline-" + session,
        count >= 1,
        "timeline-entry-evt-" + session + "-* count=" + str(count),
    )


def assert_composer_focused(tree, phase):
    focused = tree["focused"]
    return check(
        "composer-focus-" + phase,
        focused == ["composer-input"],
        "focused=" + (",".join(focused) or "none") + " (expect composer-input)",
    )


def assert_focused(tree, expected):
    return check(
        "focus-" + expected,
        tree["focused"] == [expected],
        "focused=" + (",".join(tree["focused"]) or "none") + " (expect " + expected + ")",
    )


def assert_single_visible_active(tree, phase):
    rows = list(tree["rows"])
    selected = selected_rows(tree)
    return check(
        "single-visible-active-" + phase,
        len(rows) == 1 and selected == rows,
        "visible rows="
        + (",".join(rows) or "none")
        + "; selected="
        + (",".join(selected) or "none"),
    )


PHASES = {
    "next-needs-attention",
    "tab-traverse-scope",
    "tab-traverse-grouping",
    "tab-traverse-add-task",
    "tab-reverse-grouping",
    "tab-reverse-scope",
    "button-enter-scope-menu",
    "button-enter-grouping-menu",
    "button-enter-add-task-popover",
    "rail-focus-add-task",
    "rail-focus-alpha-header",
    "rail-focus-alpha-add",
    "rail-focus-beta-header",
    "rail-expand-beta",
    "rail-focus-beta-add",
    "rail-focus-task",
    "key-open-task",
    "ax-current-task-menu",
    "grouping-projects-keyboard",
    "grouping-timeline-keyboard",
    "cycle-down-1",
    "cycle-down-2",
    "cycle-up",
    "single-visible-before-cycle",
    "single-visible-cycle",
    "disconnected-kept",
    "reconnected-kept",
    "blocked-live",
    "unread-live",
}


def phase_checks(tree, phase):
    checks = []
    if phase == "next-needs-attention":
        target = "session-fx-ses-beta-pending"
        checks.append(assert_single_selected(tree, phase, target))
        help_text = tree["rows"].get(target, {}).get("help", "")
        checks.append(
            check(
                "needs-input-help",
                "Needs input" in help_text,
                target + " help=" + (help_text or "absent"),
            )
        )
        checks.append(assert_timeline_loaded(tree, "fx-ses-beta-pending"))
        checks.append(assert_composer_focused(tree, phase))
    elif phase in (
        "tab-traverse-scope",
        "tab-traverse-grouping",
        "tab-traverse-add-task",
        "tab-reverse-grouping",
        "tab-reverse-scope",
    ):
        target = {
            "tab-traverse-scope": "project-scope",
            "tab-traverse-grouping": "task-rail-grouping",
            "tab-traverse-add-task": "add-task",
            "tab-reverse-grouping": "task-rail-grouping",
            "tab-reverse-scope": "project-scope",
        }[phase]
        checks.append(
            check(
                "tab-focus-" + target,
                tree["focused"] == [target],
                "focused=" + (",".join(tree["focused"]) or "none")
                + " (expect "
                + target
                + "; Tab/Shift-Tab 经根节点映射 focus_next/focus_prev)",
            )
        )
    elif phase in (
        "button-enter-scope-menu",
        "button-enter-grouping-menu",
        "button-enter-add-task-popover",
    ):
        # Slice 5 P2b：聚焦 rail 触发器（Tab 到达）后裸 Enter 行级激活——与
        # click 同一激活路径、不用 click_id，断言目标浮层已开（keyup 合成
        # click 由衔接标记吞掉，菜单不被闪关）。
        # R7 Wave A 焦点口径：菜单打开后 AX 焦点移交给菜单当前高亮项（树内
        # 唯一焦点）。若触发器仍发布 focused=1，会形成双 focused 的错误 AX
        # 树，必须 fail-closed；不能仅因浮层存在就误报通过。
        trigger_id, menu_id, highlighted_id = {
            "button-enter-scope-menu": ("project-scope", "scope-menu", "scope-all"),
            "button-enter-grouping-menu": (
                "task-rail-grouping",
                "grouping-menu",
                "group-timeline",
            ),
            "button-enter-add-task-popover": (
                "add-task",
                "workspace-confirm",
                "workspace-confirm-fx-alpha-app",
            ),
        }[phase]
        checks.append(
            check(
                "menu-open-" + trigger_id,
                menu_id in tree["identifiers"],
                menu_id
                + (
                    " present (Enter opened it)"
                    if menu_id in tree["identifiers"]
                    else " absent after bare Enter"
                ),
            )
        )
        checks.append(
            check(
                "menu-focus-" + highlighted_id,
                tree["focused"] == [highlighted_id],
                "focused="
                + (",".join(tree["focused"]) or "none")
                + " (expect only "
                + highlighted_id
                + " while "
                + menu_id
                + " is open; trigger "
                + trigger_id
                + " must not remain AX focused)",
            )
        )
    elif phase == "rail-focus-alpha-header":
        checks.append(assert_focused(tree, "project-Earlier_3afx-alpha-app"))
        header = tree["identifiers"]
        checks.append(
            check(
                "alpha-collapsed-after-click",
                "project-add-Earlier_3afx-alpha-app" in header
                and "session-fx-ses-alpha-today" not in header,
                "alpha rows hidden after header click (collapsed)",
            )
        )
    elif phase == "rail-focus-add-task":
        # R7 Wave A 焦点口径：click add-task 打开 workspace-confirm 后，AX
        # 焦点同样移交给弹层高亮项（与裸 Enter / AXPress 同一可观察终态）。
        # 触发器仍发布 focused=1 属双焦点错误树，fail-closed。
        checks.append(
            check(
                "rail-click-add-task-popover-open",
                "workspace-confirm" in tree["identifiers"],
                "workspace-confirm "
                + (
                    "present (click opened it)"
                    if "workspace-confirm" in tree["identifiers"]
                    else "absent after click"
                ),
            )
        )
        checks.append(
            check(
                "rail-click-add-task-focus-handover",
                tree["focused"] == ["workspace-confirm-fx-alpha-app"],
                "focused="
                + (",".join(tree["focused"]) or "none")
                + " (expect only workspace-confirm-fx-alpha-app while popover open)",
            )
        )
    elif phase == "rail-focus-alpha-add":
        checks.append(assert_focused(tree, "project-add-Earlier_3afx-alpha-app"))
    elif phase == "rail-focus-beta-header":
        checks.append(assert_focused(tree, "project-Earlier_3afx-beta-lib"))
    elif phase == "rail-expand-beta":
        checks.append(assert_focused(tree, "project-Earlier_3afx-beta-lib"))
        checks.append(
            check(
                "beta-rows-visible",
                "session-fx-ses-beta-pending" in tree["identifiers"],
                "session-fx-ses-beta-pending "
                + ("visible" if "session-fx-ses-beta-pending" in tree["identifiers"] else "hidden"),
            )
        )
    elif phase == "rail-focus-beta-add":
        checks.append(assert_focused(tree, "project-add-Earlier_3afx-beta-lib"))
    elif phase == "rail-focus-task":
        checks.append(assert_focused(tree, "session-fx-ses-beta-pending"))
    elif phase == "key-open-task":
        checks.append(assert_single_selected(tree, phase, "session-fx-ses-beta-toolfailed"))
        checks.append(assert_timeline_loaded(tree, "fx-ses-beta-toolfailed"))
        checks.append(assert_composer_focused(tree, phase))
    elif phase == "ax-current-task-menu":
        selected = selected_rows(tree)
        checks.append(
            check(
                "selected-" + phase,
                len(selected) == 1,
                "selected rows: " + (",".join(selected) or "none") + " (expect one)",
            )
        )
        if len(selected) == 1:
            checks.append(assert_timeline_loaded(tree, selected[0].removeprefix("session-")))
        else:
            checks.append(check("timeline-selected-session", False, "no unique selected session"))
        checks.append(
            check(
                "ax-current-task-menu-closed",
                "grouping-menu" not in tree["identifiers"],
                "grouping-menu "
                + (
                    "stray present after AXPress current task"
                    if "grouping-menu" in tree["identifiers"]
                    else "absent"
                ),
            )
        )
        checks.append(assert_composer_focused(tree, phase))
    elif phase in ("grouping-projects-keyboard", "grouping-timeline-keyboard"):
        expected = "Projects" if phase.endswith("projects-keyboard") else "Timeline"
        checks.append(
            check(
                "grouping-" + expected.lower(),
                tree["grouping_value"] == expected,
                "task-rail-grouping value=" + (tree["grouping_value"] or "absent"),
            )
        )
        checks.append(
            check(
                "grouping-menu-closed",
                "grouping-menu" not in tree["identifiers"],
                "grouping-menu "
                + ("stray present" if "grouping-menu" in tree["identifiers"] else "absent"),
            )
        )
        checks.append(
            assert_single_selected(tree, phase, "session-fx-ses-beta-toolfailed")
        )
        checks.append(assert_timeline_loaded(tree, "fx-ses-beta-toolfailed"))
        checks.append(assert_focused(tree, "task-rail-grouping"))
    elif phase in ("cycle-down-1", "cycle-down-2", "cycle-up"):
        expected = {
            "cycle-down-1": "session-fx-ses-beta-long",
            "cycle-down-2": "session-fx-ses-beta-cancelled",
            "cycle-up": "session-fx-ses-beta-long",
        }[phase]
        timeline_session = {
            "cycle-down-1": "fx-ses-beta-long",
            "cycle-down-2": "fx-ses-beta-cancelled",
            "cycle-up": "fx-ses-beta-long",
        }[phase]
        checks.append(assert_single_selected(tree, phase, expected))
        checks.append(assert_timeline_loaded(tree, timeline_session))
        checks.append(assert_composer_focused(tree, phase))
    elif phase == "single-visible-before-cycle":
        checks.append(assert_single_visible_active(tree, phase))
        checks.append(assert_focused(tree, "project-scope"))
    elif phase == "single-visible-cycle":
        checks.append(assert_single_visible_active(tree, phase))
        checks.append(assert_composer_focused(tree, phase))
    elif phase in ("disconnected-kept", "reconnected-kept"):
        marker = "Disconnected" if phase.startswith("disconnected") else "Connected"
        status = tree["connection_status"]
        checks.append(
            check(
                "connection-" + marker.lower(),
                marker in status,
                "connection-status=" + (status or "absent"),
            )
        )
        if phase.startswith("disconnected"):
            checks.append(
                check(
                    "reconnect-button-present",
                    "reconnect" in tree["identifiers"],
                    "reconnect button " + ("present" if "reconnect" in tree["identifiers"] else "absent"),
                )
            )
        checks.append(assert_single_selected(tree, phase, "session-fx-ses-beta-long"))
        checks.append(assert_timeline_loaded(tree, "fx-ses-beta-long"))
    elif phase == "blocked-live":
        blocked_rows = [
            row
            for row, facts in tree["rows"].items()
            if "Blocked" in facts["help"]
        ]
        checks.append(
            check(
                "blocked-row-present",
                bool(blocked_rows),
                "rows with Blocked help: " + (",".join(blocked_rows) or "none"),
            )
        )
        checks.append(
            check(
                "blocked-row-active",
                any(tree["rows"][row]["selected"] for row in blocked_rows),
                "blocked rows selected: "
                + (
                    ",".join(
                        row for row in blocked_rows if tree["rows"][row]["selected"]
                    )
                    or "none"
                ),
            )
        )
        checks.append(
            check(
                "blocked-row-is-new-session",
                all(row not in SEED_ROWS for row in blocked_rows),
                "blocked rows outside seed set: " + (",".join(blocked_rows) or "none"),
            )
        )
    elif phase == "unread-live":
        unread_rows = [
            row
            for row, facts in tree["rows"].items()
            if "Unread" in facts["help"]
        ]
        checks.append(
            check(
                "unread-row-present",
                bool(unread_rows),
                "rows with Unread help: " + (",".join(unread_rows) or "none"),
            )
        )
        checks.append(
            check(
                "unread-row-not-active",
                all(not tree["rows"][row]["selected"] for row in unread_rows),
                "unread rows must not be selected: "
                + (
                    ",".join(row for row in unread_rows if tree["rows"][row]["selected"])
                    or "none"
                ),
            )
        )
        checks.append(
            check(
                "connection-connected",
                "Connected" in tree["connection_status"],
                "connection-status=" + (tree["connection_status"] or "absent"),
            )
        )
    else:
        raise ValueError("unknown phase: " + str(phase))
    return checks


def cmd_assert(args):
    tree = parse_tree(args.tree)
    checks = phase_checks(tree, args.phase)
    payload = {
        "phase": args.phase,
        "generated_at": now_iso(),
        "checks": checks,
        "pass": all(entry["pass"] for entry in checks),
    }
    Path(args.out).write_text(json.dumps(payload, indent=2) + NL, "utf-8")
    for entry in checks:
        prefix = "PASS " if entry["pass"] else "FAIL "
        print(prefix + entry["name"] + " - " + entry["detail"])
    return 0 if payload["pass"] else 5


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    assertion = sub.add_parser("assert", help="phase-aware AX tree assertion")
    assertion.add_argument("--tree", required=True)
    assertion.add_argument("--phase", required=True)
    assertion.add_argument("--out", required=True)
    assertion.set_defaults(func=cmd_assert)
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
