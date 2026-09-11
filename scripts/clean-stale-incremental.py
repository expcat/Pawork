#!/usr/bin/env python3
"""Prune dead Cargo build generations under target/. Never wipe target/.

Cargo never deletes old build generations: every fingerprint change leaves
orphaned incremental session dirs and stale deps/build artifacts behind.

Rules (all deletions also require mtime older than --days, default 7;
or older than --older-than FILE's mtime — e.g. Cargo.toml after a
profile/toolchain change, to collect generations the change killed):

- incremental/<name>-<hash>/  Session dirs are write-once per build config;
  a dir untouched for --days is either dead or cold (worst case: one clean
  recompile of that crate). Registry deps have no incremental dirs.
- deps/ and examples/         Artifacts are named <base>-<hash>[.ext].
  Within each <base> group, keep the newest hash generation (by mtime);
  delete older generations. Sole generations are never deleted, so a
  still-current artifact is never removed no matter how old it is.
- build/<name>-<hash>/        Same generation rule as deps.

Use --dry-run to inspect before deleting.
"""
from __future__ import annotations

import argparse
import os
import re
import shutil
import sys
import time
from pathlib import Path

# deps/examples/build artifact names: <base>-<metadata>[.<tail>]
# metadata is a cargo unit hash: legacy 16-hex or base36-ish (12-17 chars).
ARTIFACT_RE = re.compile(r"^(?P<base>.+)-(?P<hash>[0-9a-z]{12,17})(?P<tail>[.].*)?$")


def split_artifact(name: str) -> tuple[str, str] | None:
    m = ARTIFACT_RE.match(name)
    if not m:
        return None
    return m.group("base"), m.group("hash")


def dir_size(path: Path) -> int:
    total = 0
    for root, _dirs, files in os.walk(path):
        for f in files:
            try:
                total += (Path(root) / f).stat().st_size
            except OSError:
                pass
    return total


def prune_incremental(root: Path, cutoff: float, dry_run: bool) -> tuple[int, int]:
    """Age-based pruning of incremental session dirs. Returns (count, bytes)."""
    if not root.is_dir():
        return 0, 0
    victims: list[Path] = []
    for entry in os.scandir(root):
        if not entry.is_dir(follow_symlinks=False):
            continue
        try:
            if entry.stat().st_mtime > cutoff:
                continue
        except OSError:
            continue
        victims.append(Path(entry.path))
    return _remove(victims, dry_run)


def prune_generations(root: Path, cutoff: float, dry_run: bool) -> tuple[int, int]:
    """Within each base-name group keep the newest hash generation."""
    if not root.is_dir():
        return 0, 0
    groups: dict[str, dict[str, list[tuple[Path, float]]]] = {}
    for entry in os.scandir(root):
        if entry.is_symlink():
            continue
        parsed = split_artifact(entry.name)
        if not parsed:
            continue
        base, hash_ = parsed
        try:
            mtime = entry.stat().st_mtime
        except OSError:
            continue
        groups.setdefault(base, {}).setdefault(hash_, []).append(
            (Path(entry.path), mtime)
        )
    victims: list[Path] = []
    for _base, hashes in groups.items():
        if len(hashes) < 2:
            continue  # sole generation: never touch
        cluster_mtime = {
            h: max(m for _, m in items) for h, items in hashes.items()
        }
        newest = max(cluster_mtime, key=cluster_mtime.get)
        for h, items in hashes.items():
            if h == newest:
                continue
            if cluster_mtime[h] > cutoff:
                continue  # young sibling generation, likely still live
            victims.extend(p for p, _ in items)
    return _remove(victims, dry_run)


def _remove(paths: list[Path], dry_run: bool) -> tuple[int, int]:
    count = 0
    bytes_ = 0
    for p in paths:
        if p.is_dir() and not p.is_symlink():
            size = dir_size(p)
            if not dry_run:
                shutil.rmtree(p, ignore_errors=True)
        else:
            try:
                size = p.stat().st_size
            except OSError:
                size = 0
            if not dry_run:
                try:
                    p.unlink()
                except OSError:
                    pass
        count += 1
        bytes_ += size
    return count, bytes_


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        default=str(Path(__file__).resolve().parents[1] / "target" / "debug"),
        help="profile dir (default: <repo>/target/debug)",
    )
    parser.add_argument("--days", type=float, default=7.0,
                        help="only delete entries untouched for this many days (default: 7)")
    parser.add_argument("--older-than", metavar="FILE",
                        help="use FILE's mtime as the cutoff instead of --days")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    root = Path(args.root).resolve()
    if not root.is_dir():
        print(f"missing {root}", file=sys.stderr)
        return 1
    if root.name in ("debug", "release") and root.parent.name == "target":
        pass
    else:
        print(f"refusing non-profile root {root}", file=sys.stderr)
        return 1

    if args.older_than:
        marker = Path(args.older_than)
        if not marker.is_file():
            print(f"missing --older-than marker {marker}", file=sys.stderr)
            return 1
        cutoff = marker.stat().st_mtime
    else:
        cutoff = time.time() - args.days * 86400
    total_n = total_b = 0
    for area, fn in (
        ("incremental", prune_incremental),
        ("deps", prune_generations),
        ("examples", prune_generations),
        ("build", prune_generations),
    ):
        n, b = fn(root / area, cutoff, args.dry_run)
        tag = "would_free" if args.dry_run else "freed"
        print(f"{area}: {tag}={b / 2**30:.1f}G entries={n}")
        total_n += n
        total_b += b
    tag = "would_free" if args.dry_run else "freed"
    print(f"total: {tag}={total_b / 2**30:.1f}G entries={total_n}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
