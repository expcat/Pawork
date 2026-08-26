#!/usr/bin/env python3
"""scripts/ui-fixture-scan.py 的正负例回归（unittest，仅标准库）。

运行：python3 scripts/test_ui_fixture_scan.py
"""

from __future__ import annotations

import importlib.util
import os
import socket
import sys
import tempfile
import unittest
from pathlib import Path

# 被测文件名按 brief 冻结为连字符形式，无法常规 import，按路径加载。
_SCAN_PATH = Path(__file__).resolve().parent / "ui-fixture-scan.py"
_SPEC = importlib.util.spec_from_file_location("ui_fixture_scan", _SCAN_PATH)
scanner = importlib.util.module_from_spec(_SPEC)
assert _SPEC.loader is not None
sys.modules[_SPEC.name] = scanner
_SPEC.loader.exec_module(scanner)

HEX64 = "a" * 63 + "b"  # 64 位 hex 样例
OTHER_HEX64 = "f" * 64


class ScannerTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def write(self, name: str, content: str) -> Path:
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        return path

    def rules(self, name: str) -> set:
        findings, _scanned = scanner.scan_root(self.root)
        return {f.rule for f in findings if str(f.path).endswith(name)}

    # ---- 正例：命中即 exit 2 ----

    def test_token_prefixes_hit(self):
        cases = [
            "sk-abcdef1234567890",
            "glpat-abcdefghijklmnop",
            "xai-0123456789abcdef",
            "ghp_" + "A1b2C3d4E5f6G7h8",
            "github_pat_" + "ABCD1234EFGH5678",
        ]
        for sample in cases:
            with tempfile.TemporaryDirectory() as tmp:
                Path(tmp, "note.md").write_text("prefix leak %s\n" % sample, "utf-8")
                findings, _ = scanner.scan_root(Path(tmp))
                self.assertIn("token-prefix", {f.rule for f in findings}, sample)

    def test_key_assignment_hits(self):
        cases = [
            'api_key = "abcdefghij1234567890XYZ"',
            "token: 0123456789abcdefghij",
            "secret='AAAABBBBCCCCDDDDEEEE'",
            "api-key: ffff00001111122223333",
            "apikey=zzzz9999yyy8888xxx777",
        ]
        for sample in cases:
            with tempfile.TemporaryDirectory() as tmp:
                Path(tmp, "config.txt").write_text("leak\n%s\n" % sample, "utf-8")
                findings, _ = scanner.scan_root(Path(tmp))
                self.assertIn("key-assignment", {f.rule for f in findings}, sample)

    def test_home_paths_hit(self):
        self.write("seed.json", '{"path": "/Users/alice/dev/repo"}\n')
        self.assertIn("home-path", self.rules("seed.json"))
        self.write("home2.txt", "log from /home/bob/project\n")
        self.assertIn("home-path", self.rules("home2.txt"))

    def test_seed_json_absolute_path_hits(self):
        self.write("seed.json", '{"path": "/tmp/somewhere/else"}\n')
        self.assertIn("seed-abs-path", self.rules("seed.json"))

    def test_auth_json_filename_hits(self):
        self.write("data/auth.json", "{}\n")
        self.assertIn("auth-json", self.rules("auth.json"))

    def test_auth_json_symlink_hits_without_following_target(self):
        target = self.write("outside.txt", "sk-abcdef1234567890\n")
        link = self.root / "data/auth.json"
        link.parent.mkdir(parents=True)
        try:
            os.symlink(target, link)
        except OSError as error:
            self.skipTest("当前平台无法创建 symlink：%s" % error)
        findings, _ = scanner.scan_root(self.root)
        link_findings = [finding for finding in findings if finding.path == Path("data/auth.json")]
        self.assertEqual({finding.rule for finding in link_findings}, {"auth-json"})

    def test_gui_token_shape_hits_with_token_context(self):
        self.write("log.txt", "connect token value %s done\n" % HEX64)
        self.assertIn("gui-token-shape", self.rules("log.txt"))

    def test_io_unreadable_file_fails_closed(self):
        secret = self.root / "locked.txt"
        secret.write_text("x\n", "utf-8")
        secret.chmod(0o000)
        self.addCleanup(secret.chmod, 0o644)
        try:
            still_readable = secret.read_text(encoding="utf-8") == "x"
        except PermissionError:
            still_readable = False
        if still_readable:  # root 环境仍可读 000 文件则跳过
            self.skipTest("当前用户可读 000 权限文件（root），跳过 fail-closed 断言")
        findings, _ = scanner.scan_root(self.root)
        self.assertIn("io-error", {f.rule for f in findings})

    # ---- 负例：正常内容不误报 ----

    def test_clean_content_passes(self):
        self.write(
            "README.md",
            "# 说明\n数据经 Host/协议/projection 到达 Desktop。\n"
            "路径写法如 ${ROOT}/workspaces/alpha-app。\n",
        )
        self.write("pty-fixture.sh", 'printf \'echo: %s\\n\' "$line"\n')
        findings, _ = scanner.scan_root(self.root)
        self.assertEqual(findings, [])

    def test_short_assignment_value_passes(self):
        self.write("notes.txt", "token: short\n")
        self.assertEqual(self.rules("notes.txt"), set())

    def test_placeholder_root_in_seed_passes(self):
        self.write(
            "seed.json",
            '{"workspaces": [{"path": "${ROOT}/workspaces/alpha-app"}]}\n',
        )
        self.assertEqual(self.rules("seed.json"), set())

    def test_home_placeholder_documentation_passes(self):
        self.write("doc.md", "禁止 /Users/<name> 或 /home/<name> 形路径。\n")
        self.assertEqual(self.rules("doc.md"), set())

    def test_bare_hex_digest_without_token_context_passes(self):
        self.write("manifest.json", '{"blake3": "%s"}\n' % OTHER_HEX64)
        self.assertEqual(self.rules("manifest.json"), set())

    def test_gui_token_store_file_passes_shape_rule(self):
        self.write("data/gui.token", HEX64 + "\n")
        self.assertEqual(self.rules("gui.token"), set())

    def test_unix_socket_is_not_treated_as_unreadable_file(self):
        socket_path = self.root / "fixture.sock"
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.addCleanup(listener.close)
        listener.bind(str(socket_path))
        findings, scanned = scanner.scan_root(self.root)
        self.assertEqual(findings, [])
        self.assertEqual(scanned, 0)

    # ---- 行为与退出码 ----

    def test_findings_carry_line_numbers_and_masked_detail(self):
        self.write("leak.txt", "line1\ntoken: 0123456789abcdefghij\n")
        findings, _ = scanner.scan_root(self.root)
        hit = next(f for f in findings if f.rule == "key-assignment")
        self.assertEqual(hit.line, 2)
        self.assertNotIn("0123456789abcdefghij", hit.detail)

    def test_home_path_detail_masks_user_name(self):
        self.write("leak.txt", "from /Users/private-user/project\n")
        findings, _ = scanner.scan_root(self.root)
        hit = next(f for f in findings if f.rule == "home-path")
        self.assertNotIn("private-user", hit.detail)
        self.assertIn("<redacted>", hit.detail)

    def test_default_roots_point_to_repo_fixture_assets(self):
        roots = scanner.default_roots()
        self.assertEqual(len(roots), 1)
        self.assertTrue(roots[0].is_dir())
        self.assertEqual(roots[0].name, "ui")

    def test_main_exit_codes(self):
        with tempfile.TemporaryDirectory() as clean:
            Path(clean, "a.txt").write_text("clean\n", "utf-8")
            self.assertEqual(scanner.main([clean]), 0)
        with tempfile.TemporaryDirectory() as dirty:
            Path(dirty, "a.txt").write_text("sk-abcdef1234567890\n", "utf-8")
            self.assertEqual(scanner.main([dirty]), 2)
        self.assertEqual(scanner.main(["/nonexistent/pawork-scan-path"]), 1)

    def test_skip_dirs_not_walked(self):
        git_dir = self.root / ".git" / "objects"
        git_dir.mkdir(parents=True)
        (git_dir / "leak.txt").write_text("sk-abcdef1234567890\n", "utf-8")
        findings, _ = scanner.scan_root(self.root)
        self.assertEqual(findings, [])


if __name__ == "__main__":
    unittest.main()
