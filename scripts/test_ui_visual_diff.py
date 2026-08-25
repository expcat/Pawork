#!/usr/bin/env python3
"""Focused regressions for ui-visual-diff.py."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np
from PIL import Image


SCRIPT = Path(__file__).with_name("ui-visual-diff.py")


class VisualDiffTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name)

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def run_gate(
        self,
        reference: np.ndarray,
        current: np.ndarray,
        zones: dict,
        masks: dict | None = None,
        reference_mode: str = "RGB",
        reference_icc: bytes | None = None,
    ) -> subprocess.CompletedProcess[str]:
        reference_path = self.root / "reference.png"
        current_path = self.root / "current.png"
        zones_path = self.root / "zones.json"
        masks_path = self.root / "masks.json"
        reference_image = Image.fromarray(
            reference.astype(np.uint8), reference_mode
        )
        reference_save_options = (
            {"icc_profile": reference_icc} if reference_icc is not None else {}
        )
        reference_image.save(reference_path, **reference_save_options)
        Image.fromarray(current.astype(np.uint8), "RGB").save(current_path)
        zones_path.write_text(json.dumps(zones), "utf-8")
        masks_path.write_text(json.dumps(masks or {"masks": []}), "utf-8")
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--reference",
                str(reference_path),
                "--current",
                str(current_path),
                "--zones",
                str(zones_path),
                "--masks",
                str(masks_path),
                "--out",
                str(self.root / "out"),
            ],
            check=False,
            capture_output=True,
            text=True,
        )

    @staticmethod
    def full_zone(width: int, height: int) -> dict:
        return {
            "threshold": 0.99,
            "zones": [
                {
                    "id": "full",
                    "x": 0,
                    "y": 0,
                    "w": width,
                    "h": height,
                    "anchor": "top-left",
                    "min_coverage": 1.0,
                }
            ],
        }

    def test_identical_images_pass_with_rgb_report_and_clean_stale_evidence(self) -> None:
        image = np.full((32, 32, 3), [7, 18, 26], dtype=np.uint8)
        stale_dir = self.root / "out" / "zone-evidence"
        stale_dir.mkdir(parents=True)
        (stale_dir / "removed-overlay-50.png").write_bytes(b"stale")
        (stale_dir / "removed-diff-heatmap.png").write_bytes(b"stale")

        result = self.run_gate(image, image, self.full_zone(32, 32))

        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads((self.root / "out" / "diff-report.json").read_text())
        self.assertTrue(report["pass"])
        self.assertEqual(report["zones"][0]["ssim"], 1.0)
        self.assertEqual(
            set(report["zones"][0]["ssim_channels"]), {"red", "green", "blue"}
        )
        self.assertEqual(
            report["method"]["color_inputs"],
            "untagged RGB bytes interpreted as sRGB",
        )
        self.assertFalse((stale_dir / "removed-overlay-50.png").exists())
        self.assertFalse((stale_dir / "removed-diff-heatmap.png").exists())

    def test_equal_luma_hue_change_fails(self) -> None:
        # Rec.601 luma is almost equal (red 76.245, green 76.31), but hue is not.
        reference = np.full((32, 32, 3), [255, 0, 0], dtype=np.uint8)
        current = np.full((32, 32, 3), [0, 130, 0], dtype=np.uint8)

        result = self.run_gate(reference, current, self.full_zone(32, 32))

        self.assertEqual(result.returncode, 1, result.stderr)
        report = json.loads((self.root / "out" / "diff-report.json").read_text())
        self.assertFalse(report["pass"])
        self.assertLess(report["zones"][0]["ssim"], 0.99)

    def test_masked_change_does_not_leak_through_ssim_window(self) -> None:
        reference = np.full((32, 32, 3), 10, dtype=np.uint8)
        current = reference.copy()
        current[12:20, 12:20] = 240
        masks = {
            "masks": [
                {
                    "id": "dynamic-value",
                    "x": 12,
                    "y": 12,
                    "w": 8,
                    "h": 8,
                    "covers": "time",
                    "reason": "synthetic dynamic value",
                }
            ]
        }

        result = self.run_gate(
            reference, current, self.full_zone(32, 32), masks=masks
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads((self.root / "out" / "diff-report.json").read_text())
        self.assertAlmostEqual(report["zones"][0]["ssim"], 1.0)
        heatmap = np.asarray(Image.open(self.root / "out" / "diff-heatmap.png"))
        self.assertEqual(int(heatmap.max()), 0)

    def test_mask_neutralization_does_not_hide_an_adjacent_change(self) -> None:
        reference = np.full((64, 64, 3), 20, dtype=np.uint8)
        current = reference.copy()
        current[24:40, 40:41] = 240
        masks = {
            "masks": [
                {
                    "id": "dynamic-value",
                    "x": 24,
                    "y": 24,
                    "w": 16,
                    "h": 16,
                    "covers": "time",
                    "reason": "synthetic dynamic value",
                }
            ]
        }

        result = self.run_gate(
            reference, current, self.full_zone(64, 64), masks=masks
        )

        self.assertEqual(result.returncode, 1, result.stderr)
        report = json.loads((self.root / "out" / "diff-report.json").read_text())
        self.assertFalse(report["pass"])

    def test_right_anchor_compares_the_common_contract_content(self) -> None:
        reference = np.full((24, 40, 3), [2, 3, 4], dtype=np.uint8)
        current = reference.copy()
        pattern = np.zeros((20, 20, 3), dtype=np.uint8)
        pattern[..., 0] = np.arange(20, dtype=np.uint8)[None, :] * 9
        pattern[..., 1] = np.arange(20, dtype=np.uint8)[:, None] * 7
        pattern[..., 2] = 90
        reference[0:20, 4:24] = pattern
        current[0:20, 10:30] = pattern
        zones = {
            "threshold": 0.99,
            "zones": [
                {
                    "id": "right",
                    "x": 0,
                    "y": 0,
                    "w": 24,
                    "h": 20,
                    "anchor": "top-right",
                    "min_coverage": 0.8,
                    "current": {"x": 10, "y": 0, "w": 20, "h": 20},
                }
            ],
        }

        result = self.run_gate(reference, current, zones)

        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads((self.root / "out" / "diff-report.json").read_text())
        self.assertAlmostEqual(report["zones"][0]["ssim"], 1.0)
        self.assertAlmostEqual(report["zones"][0]["coverage_reference"], 5 / 6)

    def test_heatmap_masks_the_aligned_current_value(self) -> None:
        reference = np.full((32, 32, 3), 10, dtype=np.uint8)
        current = reference.copy()
        reference[5:9, 2:6] = 200
        current[5:9, 10:14] = 240
        zones = {
            "threshold": 0.99,
            "zones": [
                {
                    "id": "shifted",
                    "x": 0,
                    "y": 0,
                    "w": 20,
                    "h": 20,
                    "anchor": "top-left",
                    "min_coverage": 1.0,
                    "current": {"x": 8, "y": 0, "w": 20, "h": 20},
                }
            ],
        }
        masks = {
            "masks": [
                {
                    "id": "moving-time",
                    "x": 2,
                    "y": 5,
                    "w": 4,
                    "h": 4,
                    "covers": "time",
                    "reason": "synthetic aligned dynamic value",
                }
            ]
        }

        result = self.run_gate(reference, current, zones, masks=masks)

        self.assertEqual(result.returncode, 0, result.stderr)
        heatmap = np.asarray(Image.open(self.root / "out" / "diff-heatmap.png"))
        self.assertEqual(int(heatmap.max()), 0)

    def test_minimum_coverage_rejects_tiny_common_crop_and_invalidates_report(self) -> None:
        image = np.full((32, 32, 3), 20, dtype=np.uint8)
        report_path = self.root / "out" / "diff-report.json"
        report_path.parent.mkdir(parents=True)
        report_path.write_text('{"pass": true}', "utf-8")
        zones = self.full_zone(32, 32)
        zones["zones"][0]["current"] = {"x": 0, "y": 0, "w": 1, "h": 32}
        zones["zones"][0]["min_coverage"] = 0.85

        result = self.run_gate(image, image, zones)

        self.assertEqual(result.returncode, 2)
        self.assertIn("below min_coverage", result.stderr)
        self.assertFalse(report_path.exists())

    def test_mask_fraction_cap_rejects_a_broad_mask(self) -> None:
        image = np.full((32, 32, 3), 20, dtype=np.uint8)
        masks = {
            "masks": [
                {
                    "id": "too-broad",
                    "x": 0,
                    "y": 0,
                    "w": 24,
                    "h": 32,
                    "covers": "message-text",
                    "reason": "synthetic over-masking regression",
                }
            ]
        }

        result = self.run_gate(
            image, image, self.full_zone(32, 32), masks=masks
        )

        self.assertEqual(result.returncode, 2)
        self.assertIn("exceeds max_mask_fraction", result.stderr)
        self.assertFalse((self.root / "out" / "diff-report.json").exists())

    def test_masks_outside_zones_cannot_bypass_the_global_cap(self) -> None:
        image = np.full((32, 32, 3), 20, dtype=np.uint8)
        zones = {
            "threshold": 0.99,
            "zones": [
                {
                    "id": "small-zone",
                    "x": 0,
                    "y": 0,
                    "w": 8,
                    "h": 8,
                    "min_coverage": 1.0,
                }
            ],
        }
        masks = {
            "masks": [
                {
                    "id": "outside-zone",
                    "x": 8,
                    "y": 0,
                    "w": 24,
                    "h": 32,
                    "covers": "message-text",
                    "reason": "synthetic mask outside the compared zone",
                }
            ]
        }

        result = self.run_gate(image, image, zones, masks=masks)

        self.assertEqual(result.returncode, 2)
        self.assertIn("zone global mask fraction", result.stderr)
        self.assertFalse((self.root / "out" / "diff-report.json").exists())

    def test_mapped_heatmap_mask_union_cannot_exceed_the_cap(self) -> None:
        image = np.full((32, 32, 3), 20, dtype=np.uint8)
        zones = {
            "threshold": 0.99,
            "zones": [
                {
                    "id": f"shift-{shift}",
                    "x": 0,
                    "y": 0,
                    "w": 16,
                    "h": 32,
                    "min_coverage": 1.0,
                    "current": {"x": shift, "y": 0, "w": 16, "h": 32},
                }
                for shift in (4, 8, 12, 16)
            ],
        }
        masks = {
            "masks": [
                {
                    "id": "moving-value",
                    "x": 0,
                    "y": 0,
                    "w": 4,
                    "h": 32,
                    "covers": "message-text",
                    "reason": "synthetic repeated alignment mask",
                }
            ]
        }

        result = self.run_gate(image, image, zones, masks=masks)

        self.assertEqual(result.returncode, 2)
        self.assertIn("mapped heatmap mask fraction", result.stderr)
        self.assertFalse((self.root / "out" / "diff-report.json").exists())
        self.assertFalse((self.root / "out" / "overlay-50.png").exists())
        zone_evidence = self.root / "out" / "zone-evidence"
        self.assertFalse(any(zone_evidence.glob("*.png")))

    def test_hard_policy_limits_cannot_be_relaxed(self) -> None:
        image = np.full((32, 32, 3), 20, dtype=np.uint8)

        zones = self.full_zone(32, 32)
        zones["threshold"] = 0.98
        result = self.run_gate(image, image, zones)
        self.assertEqual(result.returncode, 2)
        self.assertIn("threshold must be at least 0.99", result.stderr)

        zones = self.full_zone(32, 32)
        zones["max_mask_fraction"] = 1.0
        result = self.run_gate(image, image, zones)
        self.assertEqual(result.returncode, 2)
        self.assertIn("must not exceed 0.35", result.stderr)

        zones = self.full_zone(32, 32)
        zones["zones"][0]["min_coverage"] = 0.49
        result = self.run_gate(image, image, zones)
        self.assertEqual(result.returncode, 2)
        self.assertIn("must be at least 0.5", result.stderr)

        zones = self.full_zone(32, 32)
        zones["max_mask_fraction"] = 0.20
        zones["zones"][0]["max_mask_fraction"] = 0.30
        result = self.run_gate(image, image, zones)
        self.assertEqual(result.returncode, 2)
        self.assertIn("cannot exceed global", result.stderr)

    def test_mask_taxonomy_and_reference_artifact_edge_are_enforced(self) -> None:
        image = np.full((32, 32, 3), 20, dtype=np.uint8)
        invalid_taxonomy = {
            "masks": [
                {
                    "id": "fake-exemption",
                    "x": 2,
                    "y": 2,
                    "w": 2,
                    "h": 2,
                    "covers": "missing-component",
                    "reason": "synthetic invalid exemption",
                }
            ]
        }
        result = self.run_gate(
            image, image, self.full_zone(32, 32), masks=invalid_taxonomy
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("invalid or duplicate tokens", result.stderr)

        broad_artifact = {
            "masks": [
                {
                    "id": "broad-artifact",
                    "x": 0,
                    "y": 0,
                    "w": 32,
                    "h": 3,
                    "covers": "reference-artifact",
                    "reason": "synthetic broad frame exemption",
                }
            ]
        }
        result = self.run_gate(
            image, image, self.full_zone(32, 32), masks=broad_artifact
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("within a 2px image edge", result.stderr)

    def test_invalid_anchor_is_input_error(self) -> None:
        image = np.full((32, 32, 3), 20, dtype=np.uint8)
        zones = self.full_zone(32, 32)
        zones["zones"][0]["anchor"] = "center"
        report_path = self.root / "out" / "diff-report.json"
        report_path.parent.mkdir(parents=True)
        report_path.write_text('{"pass": true}', "utf-8")

        result = self.run_gate(image, image, zones)

        self.assertEqual(result.returncode, 2)
        self.assertIn("invalid anchor", result.stderr)
        self.assertFalse(report_path.exists())

    def test_non_rgb_and_profiled_inputs_are_rejected(self) -> None:
        image = np.full((32, 32, 3), 20, dtype=np.uint8)
        grayscale = np.full((32, 32), 20, dtype=np.uint8)

        result = self.run_gate(
            grayscale,
            image,
            self.full_zone(32, 32),
            reference_mode="L",
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("reference must be RGB", result.stderr)

        result = self.run_gate(
            image,
            image,
            self.full_zone(32, 32),
            reference_icc=b"synthetic-profile",
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("has an ICC profile", result.stderr)


if __name__ == "__main__":
    unittest.main()
