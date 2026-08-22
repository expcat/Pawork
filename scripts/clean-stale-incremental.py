#!/usr/bin/env python3
"""Delete R1-era stale Cargo incremental dirs. Never wipe target/."""
from __future__ import annotations

import argparse
import shutil
import sys
from pathlib import Path

STALE_PREFIXES = (
    "pawork_net",
    "pawork_mcp",
    "pawork_session",
    "pawork_provider_control",
    "pawork_quota",
    "pawork_sqlite",
    "pawork_resources",
    "pawork_compat",
    "pawork_api",
    "pawork_provider_core",
    "pawork_blob_store",
    "pawork_channels",
    "pawork_config",
    "pawork_diagnostics",
    "pawork_sdk",
    "pawork_review",
    "pawork_memory",
    "pawork_gui_server",
)

KEEP_PREFIXES = (
    "pawork",
    "pawork_app",
    "pawork_auth",
    "pawork_cli",
    "pawork_client",
    "pawork_control_plane",
    "pawork_desktop",
    "pawork_domain",
    "pawork_engine",
    "pawork_exec",
    "pawork_git",
    "pawork_orchestration",
    "pawork_policy",
    "pawork_protocol",
    "pawork_protocol_typegen",
    "pawork_providers",
    "pawork_storage",
    "pawork_testkit",
    "pawork_tools",
    "pawork_transport",
    "pawork_workflow",
    "pawork_workspace",
)


def crate_prefix(name: str) -> str:
    return name.split("-", 1)[0]


def matches_stale(prefix: str) -> bool:
    return prefix in STALE_PREFIXES


def is_keep(prefix: str) -> bool:
    return prefix in KEEP_PREFIXES


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        default=str(Path(__file__).resolve().parents[1] / "target" / "debug" / "incremental"),
        help="incremental directory (default: <repo>/target/debug/incremental)",
    )
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    root = Path(args.root)
    if not root.is_dir():
        print(f"missing {root}", file=sys.stderr)
        return 1

    before = sum(1 for p in root.iterdir() if p.is_dir())
    victims = []
    for path in root.iterdir():
        if not path.is_dir():
            continue
        prefix = crate_prefix(path.name)
        if is_keep(prefix):
            continue
        if matches_stale(prefix):
            victims.append(path)

    print(f"incremental_dirs_before={before}")
    print(f"stale_dirs={len(victims)}")
    if args.dry_run:
        for path in sorted(victims)[:20]:
            print(f"dry-run {path.name}")
        if len(victims) > 20:
            print(f"dry-run ... {len(victims) - 20} more")
        return 0

    deleted = 0
    for path in victims:
        shutil.rmtree(path)
        deleted += 1
    after = sum(1 for p in root.iterdir() if p.is_dir())
    print(f"deleted={deleted}")
    print(f"incremental_dirs_after={after}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
