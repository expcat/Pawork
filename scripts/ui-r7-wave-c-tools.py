#!/usr/bin/env python3
"""R7 Wave C helpers: derive a 1k-row temp fixture and summarize timings."""

from __future__ import annotations

import argparse
import json
import sqlite3
import statistics
import sys
from datetime import datetime, timezone
from pathlib import Path

NL = "\n"
PROJECTABLE_EVENT_TYPES = ("message_committed", "run_started", "run_completed")
SENTINEL = "R7C 千级列表末尾 🐾🧪"
REQUIRED_METRICS = {
    "desktop_ready",
    "timeline_1024_load",
    "timeline_scroll_up",
    "timeline_scroll_bottom",
    "composer_input",
    "resize_narrow",
    "resize_wide",
    "screenshot",
}


def now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def write_json(path: str, payload: dict) -> None:
    Path(path).write_text(
        json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True) + NL,
        encoding="utf-8",
    )


def message_envelope(
    session_id: str,
    sequence: int,
    timestamp_ms: int,
    text: str,
) -> tuple[str, str]:
    event_id = f"evt-r7c-{session_id}-{sequence}"
    payload = {
        "schema_version": 1,
        "event_id": event_id,
        "session_id": session_id,
        "run_id": f"run-r7c-{session_id}",
        "sequence": sequence,
        "timestamp": timestamp_ms,
        "payload": {
            "type": "message_committed",
            "data": {
                "message": {
                    "id": f"msg-r7c-{session_id}-{sequence}",
                    "role": "user",
                    "content": [{"type": "text", "data": {"text": text}}],
                    "metadata": {"incomplete": False},
                }
            },
        },
    }
    return event_id, json.dumps(payload, ensure_ascii=False, separators=(",", ":"))


def cmd_inflate(args: argparse.Namespace) -> int:
    db_path = Path(args.db)
    if not db_path.is_file():
        raise ValueError(f"database does not exist: {db_path}")
    if args.target_rows < 1000:
        raise ValueError("--target-rows must be at least 1000")
    connection = sqlite3.connect(db_path)
    try:
        connection.execute("PRAGMA foreign_keys = ON")
        connection.execute("BEGIN IMMEDIATE")
        session = connection.execute(
            "SELECT active_branch, updated_at_ms FROM sessions WHERE session_id = ?",
            (args.session_id,),
        ).fetchone()
        if session is None:
            raise ValueError(f"session not found: {args.session_id}")
        branch_id, updated_at_ms = session
        head = connection.execute(
            "SELECT head_sequence FROM session_branches "
            "WHERE session_id = ? AND branch_id = ?",
            (args.session_id, branch_id),
        ).fetchone()
        if head is None:
            raise ValueError(f"active branch not found: {args.session_id}/{branch_id}")
        before_rows = connection.execute(
            "SELECT COUNT(*) FROM session_events WHERE session_id = ? "
            "AND event_type IN (?, ?, ?)",
            (args.session_id, *PROJECTABLE_EVENT_TYPES),
        ).fetchone()[0]
        if before_rows != args.base_rows:
            raise ValueError(
                f"base logical rows changed: got {before_rows}, expected {args.base_rows}"
            )
        insert_count = args.target_rows - before_rows
        sequence = int(head[0])
        latest_timestamp = connection.execute(
            "SELECT COALESCE(MAX(timestamp_ms), ?) FROM session_events WHERE session_id = ?",
            (updated_at_ms, args.session_id),
        ).fetchone()[0]
        first_sequence = sequence + 1
        for index in range(insert_count):
            sequence += 1
            latest_timestamp += 1
            if index == insert_count - 1:
                text = SENTINEL + " · CJK/emoji/超长行 · " + ("边界文本" * 48)
            else:
                text = f"R7C 千级列表边界行 {index + 1:04d} · 中文 mixed text 🐾"
            event_id, payload_json = message_envelope(
                args.session_id, sequence, latest_timestamp, text
            )
            connection.execute(
                "INSERT INTO session_events "
                "(event_id, session_id, branch_id, run_id, parent_event_id, sequence, "
                "event_type, schema_version, timestamp_ms, payload_json) "
                "VALUES (?, ?, ?, ?, NULL, ?, 'message_committed', 1, ?, ?)",
                (
                    event_id,
                    args.session_id,
                    branch_id,
                    f"run-r7c-{args.session_id}",
                    sequence,
                    latest_timestamp,
                    payload_json,
                ),
            )
        connection.execute(
            "UPDATE session_branches SET head_sequence = ? "
            "WHERE session_id = ? AND branch_id = ?",
            (sequence, args.session_id, branch_id),
        )
        connection.execute(
            "UPDATE sessions SET updated_at_ms = ? WHERE session_id = ?",
            (latest_timestamp, args.session_id),
        )
        after_rows = connection.execute(
            "SELECT COUNT(*) FROM session_events WHERE session_id = ? "
            "AND event_type IN (?, ?, ?)",
            (args.session_id, *PROJECTABLE_EVENT_TYPES),
        ).fetchone()[0]
        if after_rows != args.target_rows:
            raise ValueError(
                f"derived logical rows mismatch: got {after_rows}, expected {args.target_rows}"
            )
        connection.commit()
    except Exception:
        connection.rollback()
        raise
    finally:
        connection.close()
    write_json(
        args.out,
        {
            "generated_at": now_iso(),
            "scope": "temporary_fixture_database_only",
            "session_id": args.session_id,
            "branch_id": branch_id,
            "base_logical_rows": before_rows,
            "target_logical_rows": after_rows,
            "inserted_message_rows": insert_count,
            "first_inserted_sequence": first_sequence,
            "last_inserted_sequence": sequence,
            "sentinel": SENTINEL,
        },
    )
    return 0


def read_samples(path: str) -> list[dict]:
    samples = []
    for line_number, line in enumerate(Path(path).read_text("utf-8").splitlines(), 1):
        if not line.strip():
            continue
        parts = line.split("\t", 2)
        if len(parts) != 3:
            raise ValueError(f"invalid timing sample line {line_number}")
        metric, raw_value, detail = parts
        value_ms = int(raw_value)
        if value_ms < 0:
            raise ValueError(f"negative timing sample line {line_number}")
        samples.append({"metric": metric, "value_ms": value_ms, "detail": detail})
    return samples


def read_geometry_frame(path: str, identifier: str) -> dict:
    for line in Path(path).read_text("utf-8").splitlines():
        if not line.startswith("id=" + identifier + " "):
            continue
        fields = {}
        for part in line.split():
            if "=" in part:
                key, value = part.split("=", 1)
                fields[key] = value
        try:
            return {
                "x": float(fields["x"]),
                "y": float(fields["y"]),
                "w": float(fields["w"]),
                "h": float(fields["h"]),
            }
        except (KeyError, ValueError) as error:
            raise ValueError("invalid frame for " + identifier) from error
    raise ValueError("frame missing: " + identifier)


def cmd_paint_assert(args: argparse.Namespace) -> int:
    from PIL import Image

    root = read_geometry_frame(args.geometry, "pawork-root")
    status = read_geometry_frame(args.geometry, "connection-status")
    add_task = read_geometry_frame(args.geometry, "add-task")
    image = Image.open(args.screenshot).convert("RGB")
    expected_size = (int(root["w"]), int(root["h"]))
    if image.size != expected_size:
        raise ValueError(
            "screenshot size mismatch: got " + str(image.size)
            + " expected " + str(expected_size)
        )

    def relative(frame: dict) -> tuple[int, int, int, int]:
        return (
            int(frame["x"] - root["x"]),
            int(frame["y"] - root["y"]),
            int(frame["w"]),
            int(frame["h"]),
        )

    status_x, status_y, status_w, status_h = relative(status)
    add_x, add_y, add_w, add_h = relative(add_task)
    gap_x0 = int(status_x + status_w) + 1
    gap_x1 = add_x - 1
    gap_y0 = status_y + 6
    gap_y1 = status_y + status_h - 6
    lit_gap = [
        (x, y)
        for y in range(gap_y0, gap_y1 + 1)
        for x in range(gap_x0, gap_x1 + 1)
        if max(image.getpixel((x, y))) > 100
    ]
    plus_pixels = [
        (x, y)
        for y in range(add_y + 4, add_y + add_h - 4)
        for x in range(add_x + 4, add_x + add_w - 4)
        if max(image.getpixel((x, y))) > 100
    ]
    passed = gap_x0 <= gap_x1 and not lit_gap and len(plus_pixels) >= 5
    write_json(
        args.out,
        {
            "generated_at": now_iso(),
            "screenshot": args.screenshot,
            "geometry": args.geometry,
            "gap_rect": [gap_x0, gap_y0, gap_x1, gap_y1],
            "lit_gap_pixels": lit_gap,
            "plus_pixel_count": len(plus_pixels),
            "pass": passed,
        },
    )
    print(
        ("PASS" if passed else "FAIL")
        + " disconnected-rail-paint-gap - lit=" + str(len(lit_gap))
        + " plus_pixels=" + str(len(plus_pixels))
    )
    return 0 if passed else 5


def cmd_report(args: argparse.Namespace) -> int:
    samples = read_samples(args.samples)
    metrics = {sample["metric"] for sample in samples}
    missing = sorted(REQUIRED_METRICS - metrics)
    if missing:
        raise ValueError("missing timing metrics: " + ", ".join(missing))
    grouped = {}
    for metric in sorted(metrics):
        values = [sample["value_ms"] for sample in samples if sample["metric"] == metric]
        grouped[metric] = {
            "count": len(values),
            "min_ms": min(values),
            "median_ms": statistics.median(values),
            "max_ms": max(values),
        }
    platform = json.loads(Path(args.platform).read_text("utf-8"))
    dataset = json.loads(Path(args.dataset).read_text("utf-8"))
    write_json(
        args.out,
        {
            "generated_at": now_iso(),
            "classification": "baseline_only",
            "thresholds": None,
            "threshold_note": (
                "First R7 Wave C real-window sample; no regression threshold is "
                "claimed until repeated clean-machine samples are reviewed."
            ),
            "dataset": dataset,
            "platform": platform,
            "summary": grouped,
            "samples": samples,
        },
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    inflate = sub.add_parser("inflate")
    inflate.add_argument("--db", required=True)
    inflate.add_argument("--session-id", required=True)
    inflate.add_argument("--base-rows", type=int, required=True)
    inflate.add_argument("--target-rows", type=int, required=True)
    inflate.add_argument("--out", required=True)
    inflate.set_defaults(func=cmd_inflate)
    report = sub.add_parser("report")
    report.add_argument("--samples", required=True)
    report.add_argument("--platform", required=True)
    report.add_argument("--dataset", required=True)
    report.add_argument("--out", required=True)
    report.set_defaults(func=cmd_report)
    paint = sub.add_parser("paint-assert")
    paint.add_argument("--screenshot", required=True)
    paint.add_argument("--geometry", required=True)
    paint.add_argument("--out", required=True)
    paint.set_defaults(func=cmd_paint_assert)
    args = parser.parse_args()
    try:
        return args.func(args)
    except (OSError, ValueError, sqlite3.Error, json.JSONDecodeError) as error:
        print(f"ui-r7-wave-c-tools: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
