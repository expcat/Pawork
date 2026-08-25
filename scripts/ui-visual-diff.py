#!/usr/bin/env python3
"""Zoned color SSIM gate for the R1/R8 99% visual contract.

Judgment order (docs/UI_Review.md section 0.1):
  1. structure/geometry checklist veto (human + U1/U2)
  2. every RGB channel in every zone reaches the configured SSIM threshold
  3. human review of raw and zone-aligned overlays

The full-screen score is auxiliary only. Dynamic masks affect SSIM and the
heatmap, but never change the raw overlay. Masked pixels are replaced by the
same neutral value in both inputs before local-window statistics are computed;
this prevents an 11x11 SSIM window from leaking dynamic text differences past
the mask edge. The contract's 0.99 SSIM floor, 35% mask ceiling, and 50%
common-crop floor are hard policy limits: configuration may only make them
stricter.

zones.json supports an optional live-current rectangle and alignment anchor:

  {
    "threshold": 0.99,
    "max_mask_fraction": 0.35,
    "zones": [{
      "id": "taskrail",
      "x": 0, "y": 0, "w": 297, "h": 1024,
      "anchor": "top-left",
      "min_coverage": 0.85,
      "current": {"x": 0, "y": 0, "w": 288, "h": 1024}
    }]
  }

If current is omitted, the reference rectangle is used. Differently sized
rectangles align at anchor and compare their common crop. min_coverage prevents
that crop from silently discarding most of either zone. max_mask_fraction
rejects over-masking before a score can be accepted.

Usage:
  python3 scripts/ui-visual-diff.py \
      --reference docs/ui-review/state-a/reference.png \
      --current docs/ui-review/state-a/current.png \
      --zones docs/ui-review/state-a/zones.json \
      --masks docs/ui-review/state-a/mask.json \
      --out docs/ui-review/state-a

Exit codes: 0 all zones pass; 1 a visual zone fails; 2 invalid input or I/O.
Requires Python 3.8+, Pillow and numpy.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
from pathlib import Path

import numpy as np
from PIL import Image

WIN = 11
L = 255.0
C1 = (0.01 * L) ** 2
C2 = (0.03 * L) ** 2
ANCHORS = {"top-left", "top-right", "bottom-left", "bottom-right"}
RECT_KEYS = ("x", "y", "w", "h")
ID_PATTERN = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")
HARD_MAX_MASK_FRACTION = 0.35
HARD_MIN_COVERAGE = 0.50
HARD_MIN_SSIM = 0.99
ALLOWED_COVER_TOKENS = frozenset(
    {
        "account-name",
        "added",
        "agent-name",
        "agent-status",
        "branch-name",
        "count",
        "deleted",
        "diff-count",
        "diff-stats",
        "diff-text",
        "duration",
        "file-count",
        "filename",
        "geometry-drift",
        "message-content",
        "message-text",
        "model-name",
        "project-name",
        "quota",
        "reasoning",
        "reference-artifact",
        "run-state",
        "session-title",
        "task-name",
        "task-title",
        "time",
        "tokens",
        "tokens-per-second",
        "tool-name",
        "workspace-name",
    }
)
SPECIAL_COVER_TYPES = {"geometry-drift", "reference-artifact"}
GENERATED_ROOT_FILES = (
    "diff-report.json",
    "overlay-50.png",
    "diff-heatmap.png",
    "raw-diff-heatmap.png",
    "masked-overlay-50.png",
)
GENERATED_ZONE_SUFFIXES = ("-overlay-50.png", "-diff-heatmap.png")


class InputError(ValueError):
    """Invalid visual-gate input (reported as exit code 2)."""


def box_mean(values: np.ndarray) -> np.ndarray:
    """Return an edge-clamped WIN x WIN box mean using integral images."""
    pad = WIN // 2
    padded = np.pad(values, pad, mode="edge")
    integral = padded.cumsum(axis=0).cumsum(axis=1)
    integral = np.pad(integral, ((1, 0), (1, 0)), mode="constant")
    total = (
        integral[WIN:, WIN:]
        - integral[:-WIN, WIN:]
        - integral[WIN:, :-WIN]
        + integral[:-WIN, :-WIN]
    )
    return total / float(WIN * WIN)


def ssim_map(first: np.ndarray, second: np.ndarray) -> np.ndarray:
    """Compute a single-channel SSIM map."""
    mean_first = box_mean(first)
    mean_second = box_mean(second)
    mean_first_sq = mean_first * mean_first
    mean_second_sq = mean_second * mean_second
    mean_both = mean_first * mean_second
    variance_first = np.maximum(box_mean(first * first) - mean_first_sq, 0.0)
    variance_second = np.maximum(box_mean(second * second) - mean_second_sq, 0.0)
    covariance = box_mean(first * second) - mean_both
    numerator = (2 * mean_both + C1) * (2 * covariance + C2)
    denominator = (
        (mean_first_sq + mean_second_sq + C1)
        * (variance_first + variance_second + C2)
    )
    return numerator / denominator


def validate_rect(rect: object, label: str, width: int, height: int) -> dict:
    if not isinstance(rect, dict):
        raise InputError(f"{label} must be an object")
    for key in RECT_KEYS:
        value = rect.get(key)
        if isinstance(value, bool) or not isinstance(value, int):
            raise InputError(f"{label}.{key} must be an integer")
    if rect["w"] <= 0 or rect["h"] <= 0:
        raise InputError(f"{label} must have positive width and height")
    if (
        rect["x"] < 0
        or rect["y"] < 0
        or rect["x"] + rect["w"] > width
        or rect["y"] + rect["h"] > height
    ):
        raise InputError(
            f"{label} out of bounds: x={rect['x']} y={rect['y']} "
            f"w={rect['w']} h={rect['h']} for {width}x{height}"
        )
    return rect


def validate_fraction(value: object, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise InputError(f"{label} must be numeric")
    value = float(value)
    if not 0.0 < value <= 1.0:
        raise InputError(f"{label} must be in (0, 1]")
    return value


def validate_inputs(zones_cfg: object, masks_cfg: object, width: int, height: int):
    if not isinstance(zones_cfg, dict):
        raise InputError("zones root must be an object")
    threshold = zones_cfg.get("threshold", 0.99)
    threshold = validate_fraction(threshold, "threshold")
    if threshold < HARD_MIN_SSIM:
        raise InputError(f"threshold must be at least {HARD_MIN_SSIM}")
    max_mask_fraction = validate_fraction(
        zones_cfg.get("max_mask_fraction", HARD_MAX_MASK_FRACTION),
        "max_mask_fraction",
    )
    if max_mask_fraction > HARD_MAX_MASK_FRACTION:
        raise InputError(
            f"max_mask_fraction must not exceed {HARD_MAX_MASK_FRACTION}"
        )

    zones = zones_cfg.get("zones")
    if not isinstance(zones, list) or not zones:
        raise InputError("zones must be a non-empty list")
    zone_ids = set()
    for index, zone in enumerate(zones):
        validate_rect(zone, f"zones[{index}]", width, height)
        zone_id = zone.get("id")
        if not isinstance(zone_id, str) or not zone_id.strip():
            raise InputError(f"zones[{index}].id must be a non-empty string")
        if ID_PATTERN.fullmatch(zone_id) is None:
            raise InputError(
                f"zones[{index}].id must match {ID_PATTERN.pattern}: {zone_id!r}"
            )
        if zone_id in zone_ids:
            raise InputError(f"duplicate zone id: {zone_id}")
        zone_ids.add(zone_id)
        anchor = zone.get("anchor", "top-left")
        if anchor not in ANCHORS:
            raise InputError(f"zone {zone_id} has invalid anchor: {anchor}")
        min_coverage = validate_fraction(
            zone.get("min_coverage", 0.85), f"zone {zone_id} min_coverage"
        )
        if min_coverage < HARD_MIN_COVERAGE:
            raise InputError(
                f"zone {zone_id} min_coverage must be at least "
                f"{HARD_MIN_COVERAGE}"
            )
        zone_max_mask_fraction = validate_fraction(
            zone.get("max_mask_fraction", max_mask_fraction),
            f"zone {zone_id} max_mask_fraction",
        )
        if zone_max_mask_fraction > max_mask_fraction:
            raise InputError(
                f"zone {zone_id} max_mask_fraction cannot exceed global "
                f"max_mask_fraction {max_mask_fraction}"
            )
        if "current" in zone:
            validate_rect(zone["current"], f"zone {zone_id}.current", width, height)

    if not isinstance(masks_cfg, dict):
        raise InputError("masks root must be an object")
    masks = masks_cfg.get("masks", [])
    if not isinstance(masks, list):
        raise InputError("masks must be a list")
    mask_ids = set()
    for index, mask in enumerate(masks):
        validate_rect(mask, f"masks[{index}]", width, height)
        mask_id = mask.get("id")
        if not isinstance(mask_id, str) or not mask_id.strip():
            raise InputError(f"masks[{index}].id must be a non-empty string")
        if ID_PATTERN.fullmatch(mask_id) is None:
            raise InputError(
                f"masks[{index}].id must match {ID_PATTERN.pattern}: {mask_id!r}"
            )
        if mask_id in mask_ids:
            raise InputError(f"duplicate mask id: {mask_id}")
        mask_ids.add(mask_id)
        if not isinstance(mask.get("covers"), str) or not mask["covers"].strip():
            raise InputError(f"mask {mask_id}.covers must be a non-empty string")
        cover_tokens = mask["covers"].split("|")
        invalid_cover_tokens = [
            token for token in cover_tokens if token not in ALLOWED_COVER_TOKENS
        ]
        if invalid_cover_tokens or len(cover_tokens) != len(set(cover_tokens)):
            raise InputError(
                f"mask {mask_id}.covers has invalid or duplicate tokens: "
                f"{mask['covers']}"
            )
        if SPECIAL_COVER_TYPES.intersection(cover_tokens) and len(cover_tokens) != 1:
            raise InputError(
                f"mask {mask_id}.covers special type must not be combined"
            )
        if mask["covers"] == "reference-artifact":
            horizontal_edge = (
                mask["h"] <= 2
                and (mask["y"] == 0 or mask["y"] + mask["h"] == height)
            )
            vertical_edge = (
                mask["w"] <= 2
                and (mask["x"] == 0 or mask["x"] + mask["w"] == width)
            )
            if not (horizontal_edge or vertical_edge):
                raise InputError(
                    f"mask {mask_id} reference-artifact must stay within "
                    "a 2px image edge"
                )
        if not isinstance(mask.get("reason"), str) or not mask["reason"].strip():
            raise InputError(f"mask {mask_id}.reason must be a non-empty string")
    return threshold, max_mask_fraction, zones, masks


def anchor_offset(anchor: str, size: tuple[int, int], target: tuple[int, int]):
    x = size[0] - target[0] if anchor.endswith("right") else 0
    y = size[1] - target[1] if anchor.startswith("bottom") else 0
    return x, y


def rect_mask(
    masks: list[dict],
    origin_x: int,
    origin_y: int,
    width: int,
    height: int,
) -> np.ndarray:
    selected = np.zeros((height, width), dtype=bool)
    for mask in masks:
        left = max(mask["x"], origin_x) - origin_x
        top = max(mask["y"], origin_y) - origin_y
        right = min(mask["x"] + mask["w"], origin_x + width) - origin_x
        bottom = min(mask["y"] + mask["h"], origin_y + height) - origin_y
        if right > left and bottom > top:
            selected[top:bottom, left:right] = True
    return selected


def score_zone(
    zone: dict,
    reference_rgb: np.ndarray,
    current_rgb: np.ndarray,
    masks: list[dict],
    default_max_mask_fraction: float,
) -> tuple[dict, np.ndarray, np.ndarray, np.ndarray]:
    rx, ry, rw, rh = (zone[key] for key in RECT_KEYS)
    current = zone.get("current", zone)
    cx, cy, cw, ch = (current[key] for key in RECT_KEYS)
    anchor = zone.get("anchor", "top-left")
    common_width, common_height = min(rw, cw), min(rh, ch)
    coverage_reference = (common_width * common_height) / (rw * rh)
    coverage_current = (common_width * common_height) / (cw * ch)
    min_coverage = float(zone.get("min_coverage", 0.85))
    coverage = min(coverage_reference, coverage_current)
    if coverage + 1e-12 < min_coverage:
        raise InputError(
            f"zone {zone['id']} common crop coverage {coverage:.4f} "
            f"is below min_coverage {min_coverage:.4f}"
        )

    ref_dx, ref_dy = anchor_offset(anchor, (rw, rh), (common_width, common_height))
    cur_dx, cur_dy = anchor_offset(anchor, (cw, ch), (common_width, common_height))
    ref_x, ref_y = rx + ref_dx, ry + ref_dy
    cur_x, cur_y = cx + cur_dx, cy + cur_dy
    ref_crop = reference_rgb[
        ref_y:ref_y + common_height, ref_x:ref_x + common_width
    ].copy()
    cur_crop = current_rgb[
        cur_y:cur_y + common_height, cur_x:cur_x + common_width
    ].copy()

    masked = rect_mask(masks, ref_x, ref_y, common_width, common_height)
    mask_fraction = float(masked.mean())
    max_mask_fraction = float(
        zone.get("max_mask_fraction", default_max_mask_fraction)
    )
    if mask_fraction > max_mask_fraction + 1e-12:
        raise InputError(
            f"zone {zone['id']} mask fraction {mask_fraction:.4f} "
            f"exceeds max_mask_fraction {max_mask_fraction:.4f}"
        )
    ref_for_score = ref_crop.copy()
    cur_for_score = cur_crop.copy()
    ref_for_score[masked] = 0.0
    cur_for_score[masked] = 0.0
    weight = (~masked).astype(np.float64)
    evaluated = int(weight.sum())
    if evaluated == 0:
        raise InputError(f"zone {zone['id']} is fully masked")

    channel_scores = []
    for channel in range(3):
        channel_map = ssim_map(
            ref_for_score[..., channel], cur_for_score[..., channel]
        )
        channel_scores.append(float((channel_map * weight).sum() / evaluated))
    score = min(channel_scores)
    if not all(math.isfinite(value) for value in channel_scores):
        raise InputError(f"zone {zone['id']} produced a non-finite SSIM score")

    details = {
        "id": zone["id"],
        "anchor": anchor,
        "reference": {key: zone[key] for key in RECT_KEYS},
        "current": {key: current[key] for key in RECT_KEYS},
        "common_size": [common_width, common_height],
        "aligned_reference_origin": [ref_x, ref_y],
        "aligned_current_origin": [cur_x, cur_y],
        "coverage_reference": coverage_reference,
        "coverage_current": coverage_current,
        "evaluated_pixels": evaluated,
        "masked_pixels": int(masked.sum()),
        "mask_fraction": mask_fraction,
        "max_mask_fraction": max_mask_fraction,
        "ssim_channels": {
            "red": channel_scores[0],
            "green": channel_scores[1],
            "blue": channel_scores[2],
        },
        "ssim": score,
    }
    return details, ref_crop, cur_crop, masked


def safe_id(value: str) -> str:
    cleaned = re.sub(r"[^a-zA-Z0-9._-]+", "-", value).strip("-")
    return cleaned or "zone"


def save_overlay(path: Path, reference: np.ndarray, current: np.ndarray) -> None:
    overlay = (0.5 * reference + 0.5 * current).astype(np.uint8)
    Image.fromarray(overlay).save(path)


def save_heatmap(
    path: Path,
    reference: np.ndarray,
    current: np.ndarray,
    masked: np.ndarray,
) -> None:
    difference = np.abs(reference - current).mean(axis=2)
    difference[masked] = 0.0
    heat = np.clip(difference * 4.0, 0, 255).astype(np.uint8)
    heatmap = np.zeros((*heat.shape, 3), dtype=np.uint8)
    heatmap[..., 0] = heat
    heatmap[..., 1] = heat // 3
    Image.fromarray(heatmap).save(path)


def save_evidence(
    output_dir: Path,
    name: str,
    reference: np.ndarray,
    current: np.ndarray,
    masked: np.ndarray,
) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    save_overlay(output_dir / f"{name}-overlay-50.png", reference, current)
    save_heatmap(
        output_dir / f"{name}-diff-heatmap.png", reference, current, masked
    )


def cleanup_generated(output: Path) -> None:
    for name in GENERATED_ROOT_FILES:
        (output / name).unlink(missing_ok=True)
    zone_output = output / "zone-evidence"
    for suffix in GENERATED_ZONE_SUFFIXES:
        for stale in zone_output.glob(f"*{suffix}"):
            stale.unlink()


def load_untagged_rgb(path: str, label: str) -> np.ndarray:
    with Image.open(path) as image:
        if image.mode != "RGB":
            raise InputError(f"{label} must be RGB, got {image.mode}")
        if image.info.get("icc_profile"):
            raise InputError(
                f"{label} has an ICC profile; normalize it explicitly to "
                "untagged sRGB before comparison"
            )
        image.load()
        return np.asarray(image, dtype=np.float64)


def run(args: argparse.Namespace) -> int:
    output = Path(args.out)
    output.mkdir(parents=True, exist_ok=True)
    report_path = output / "diff-report.json"
    zone_output = output / "zone-evidence"
    zone_output.mkdir(parents=True, exist_ok=True)
    # Any failed/incomplete invocation must not leave a prior PASS report.
    # Only this script's exact root names and zone PNG suffixes are eligible.
    cleanup_generated(output)

    reference_rgb = load_untagged_rgb(args.reference, "reference")
    current_rgb = load_untagged_rgb(args.current, "current")
    if reference_rgb.shape != current_rgb.shape:
        raise InputError(
            f"size mismatch: reference {reference_rgb.shape[1::-1]} "
            f"vs current {current_rgb.shape[1::-1]}"
        )
    height, width = reference_rgb.shape[:2]

    zones_cfg = json.loads(Path(args.zones).read_text("utf-8"))
    masks_cfg = json.loads(Path(args.masks).read_text("utf-8"))
    threshold, max_mask_fraction, zones, masks = validate_inputs(
        zones_cfg, masks_cfg, width, height
    )

    save_overlay(output / "overlay-50.png", reference_rgb, current_rgb)
    full_mask = rect_mask(masks, 0, 0, width, height)
    heatmap_mask = full_mask.copy()

    report = {
        "method": {
            "ssim": "11x11 box-window, per RGB channel",
            "zone_score": "minimum of red/green/blue channel scores",
            "color_inputs": "untagged RGB bytes interpreted as sRGB",
            "masked_pixels": "neutralized in both inputs before SSIM and excluded from mean",
            "heatmap_mask": "union of reference masks and their zone-aligned current positions",
        },
        "reference": str(Path(args.reference)),
        "current": str(Path(args.current)),
        "size": [width, height],
        "threshold": threshold,
        "max_mask_fraction": max_mask_fraction,
        "mask_count": len(masks),
        "heatmap_masked_pixels": None,
        "evidence": {
            "raw_overlay": "overlay-50.png",
            "masked_heatmap": "diff-heatmap.png",
            "zone_directory": "zone-evidence",
        },
        "zones": [],
        "global": None,
        "pass": None,
    }
    passed = True
    for zone in zones:
        details, ref_crop, cur_crop, masked = score_zone(
            zone, reference_rgb, current_rgb, masks, max_mask_fraction
        )
        details["pass"] = details["ssim"] >= threshold
        passed = passed and details["pass"]
        report["zones"].append(details)
        current_x, current_y = details["aligned_current_origin"]
        common_width, common_height = details["common_size"]
        current_mask_view = heatmap_mask[
            current_y:current_y + common_height,
            current_x:current_x + common_width,
        ]
        current_mask_view |= masked
        save_evidence(
            zone_output, safe_id(zone["id"]), ref_crop, cur_crop, masked
        )
        print(
            f"zone {zone['id']}: ssim={details['ssim']:.6f} "
            f"rgb=({details['ssim_channels']['red']:.6f},"
            f"{details['ssim_channels']['green']:.6f},"
            f"{details['ssim_channels']['blue']:.6f}) "
            f"coverage={min(details['coverage_reference'], details['coverage_current']):.4f} "
            f"threshold={threshold} {'PASS' if details['pass'] else 'FAIL'}"
        )

    global_zone = {
        "id": "global",
        "x": 0,
        "y": 0,
        "w": width,
        "h": height,
        "max_mask_fraction": max_mask_fraction,
    }
    global_details, _, _, _ = score_zone(
        global_zone, reference_rgb, current_rgb, masks, max_mask_fraction
    )
    heatmap_mask_fraction = float(heatmap_mask.mean())
    if heatmap_mask_fraction > max_mask_fraction + 1e-12:
        raise InputError(
            f"mapped heatmap mask fraction {heatmap_mask_fraction:.4f} "
            f"exceeds max_mask_fraction {max_mask_fraction:.4f}"
        )
    report["global"] = global_details
    report["heatmap_masked_pixels"] = int(heatmap_mask.sum())
    report["heatmap_mask_fraction"] = heatmap_mask_fraction
    report["pass"] = passed
    save_heatmap(
        output / "diff-heatmap.png",
        reference_rgb,
        current_rgb,
        heatmap_mask,
    )
    print(f"global (auxiliary only): {global_details['ssim']:.6f}")
    report_path.write_text(
        json.dumps(report, indent=2) + chr(10), "utf-8"
    )
    return 0 if passed else 1


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--reference", required=True)
    parser.add_argument("--current", required=True)
    parser.add_argument("--zones", required=True)
    parser.add_argument("--masks", required=True)
    parser.add_argument("--out", required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        return run(args)
    except (
        InputError,
        OSError,
        UnicodeError,
        json.JSONDecodeError,
        KeyError,
        TypeError,
    ) as error:
        try:
            cleanup_generated(Path(args.out))
        except OSError as cleanup_error:
            print(f"ui-visual-diff cleanup: {cleanup_error}", file=sys.stderr)
        print(f"ui-visual-diff: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
