#!/usr/bin/env python3
"""Normalize the three v3 design PNGs to the 1440x1024 acceptance viewport.

R1 Wave A visual-contract tooling. For each state we resize the full design
canvas (including the inline macOS traffic-light strip, which the immersive
dark titlebar plan treats as content) to exactly 1440x1024 with LANCZOS.
No cropping and no color-profile conversion: the source PNGs are untagged RGB,
treated as sRGB, and resampled as-is. The 1px per-image edge variance
(1486x1059 vs 1487x1058) is absorbed by the resize; per-image scale factors
are printed for the normalization record in docs/ui-review/README.md.

Usage:
  python3 scripts/ui-normalize-reference.py

Requires Python 3 and Pillow.
"""

import io
import json
import tempfile
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
TARGET_W = 1440
TARGET_H = 1024

STATES = {
    "state-a": "design/desktop-shell-timeline-v3.png",
    "state-b": "design/desktop-shell-timeline-collapsed-v3.png",
    "state-c": "design/desktop-shell-projects-v3.png",
}


def replace_if_changed(path: Path, payload: bytes) -> bool:
    """Atomically replace path only when its bytes actually changed."""
    try:
        if path.read_bytes() == payload:
            return False
    except FileNotFoundError:
        pass

    path.parent.mkdir(parents=True, exist_ok=True)
    temporary_path = None
    try:
        with tempfile.NamedTemporaryFile(
            prefix=f".{path.name}.", suffix=".tmp", dir=path.parent, delete=False
        ) as temporary:
            temporary.write(payload)
            temporary_path = Path(temporary.name)
        temporary_path.replace(path)
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)
    return True


def main() -> int:
    report = {}
    for state, rel in STATES.items():
        src_path = ROOT / rel
        out_dir = ROOT / "docs" / "ui-review" / state
        out_dir.mkdir(parents=True, exist_ok=True)
        out_path = out_dir / "reference.png"

        with Image.open(src_path) as src:
            if src.mode != "RGB":
                raise ValueError(f"{src_path}: expected RGB source, got {src.mode}")
            if src.info.get("icc_profile"):
                raise ValueError(
                    f"{src_path}: source now has an ICC profile; "
                    "update the normalization contract before converting it"
                )
            sw, sh = src.size
            resized = src.resize((TARGET_W, TARGET_H), Image.LANCZOS)
            encoded = io.BytesIO()
            resized.save(encoded, "PNG")
            changed = replace_if_changed(out_path, encoded.getvalue())

        report[state] = {
            "source": rel,
            "source_size": [sw, sh],
            "source_mode": "RGB",
            "source_profile": "untagged",
            "target_size": [TARGET_W, TARGET_H],
            "scale_x": round(TARGET_W / sw, 6),
            "scale_y": round(TARGET_H / sh, 6),
            "crop": "none (full canvas resize)",
            "color": "untagged RGB treated as sRGB; no profile conversion",
            "titlebar": "inline traffic-light strip kept inside content viewport",
            "output": str(out_path.relative_to(ROOT)),
        }
        print(
            f"{state}: {sw}x{sh} -> {TARGET_W}x{TARGET_H} "
            f"(sx={report[state]['scale_x']}, sy={report[state]['scale_y']}; "
            f"{'updated' if changed else 'unchanged'})"
        )

    report_path = ROOT / "docs" / "ui-review" / "normalization-report.json"
    report_payload = (json.dumps(report, indent=2) + chr(10)).encode("utf-8")
    replace_if_changed(report_path, report_payload)
    print(f"report: {report_path.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
