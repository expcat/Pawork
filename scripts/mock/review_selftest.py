#!/usr/bin/env python3
"""Review 回归：真实临时进程/文件往返，以及配置恢复、凭证与路径失败边界。

不读取真实 auth.json，不修改用户 Global config，不请求外网。
"""
import contextlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
import socket
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

import capture
import run_instance
import server
import seed_auth


class ReviewRegression(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='pawork-review-', dir='/tmp')
        self.root = Path(self.temp.name)
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            self.port = sock.getsockname()[1]
        # A tiny real Host stand-in. It binds the requested UDS and stays alive.
        self.host = self.root / 'fake-host'
        self.host.write_text('#!' + sys.executable + '\nimport socket,sys,time\ns=socket.socket(socket.AF_UNIX)\ns.bind(sys.argv[sys.argv.index("--socket")+1])\ns.listen()\ntime.sleep(60)\n')
        self.host.chmod(0o700)
        self.config = self.root / 'global.toml'
        self.original = b'default_provider="xai"\n[oauth.xai]\nclient_id="original"\n[[providers]]\nid="xai"\nbase_url="https://example.invalid"\n'
        self.config.write_bytes(self.original)
        self.config.chmod(0o640)
        self.environ = patch.dict(os.environ, {'PAWORK_MOCK_STATE_DIR': str(self.root / 'state'),
            'PAWORK_MOCK_GLOBAL_CONFIG': str(self.config), 'PAWORK_BIN': str(self.host), 'MOCK_PORT': str(self.port)})
        self.environ.start()
        self.instance = run_instance.Instance()

    def tearDown(self):
        # Only this temp directory may be touched, including on assertion failure.
        if self.instance.owner() == str(self.instance.state):
            if self.instance.expected.is_file():
                self.config.write_bytes(self.instance.expected.read_bytes())
            with self.instance.locked():
                self.instance.stop()
        self.environ.stop()
        self.temp.cleanup()

    def test_lifecycle_and_fixture_capture_contract(self):
        with self.instance.locked():
            self.instance.start()
            first = self.instance.process('host')
            self.instance.start()
            self.assertEqual(first, self.instance.process('host'))
            self.assertEqual(self.config.read_bytes().count(b'[oauth.xai]'), 1)
            # Parse when tomllib is available (the runner itself only needs stdlib 3.9).
            try:
                import tomllib
            except ImportError:
                tomllib = None
            if tomllib:
                tomllib.loads(self.config.read_text())
            self.instance.stop()
            self.assertEqual(self.config.read_bytes(), self.original)
            self.assertEqual(self.config.stat().st_mode & 0o777, 0o640)
            self.assertIsNone(self.instance.owner())
        self.assertEqual(capture.CHANNELS['kimi-code']['cred_service'], 'pawork.kimi-code.oauth')
        meta = {'pawork.chatgpt.oauth': {'default.meta': json.dumps({'account_id': 'acct-test'})}}
        with patch.object(capture, 'load_auth_entries', return_value=meta):
            headers = capture.channel_headers('chatgpt', capture.CHANNELS['chatgpt'], 'test-secret')
            self.assertEqual(headers['ChatGPT-Account-Id'], 'acct-test')
            self.assertEqual(headers['originator'], 'codex_cli_rs')
        with contextlib.redirect_stdout(io.StringIO()):
            seed_auth.main(['--home', str(self.root / 'seed'), '--provider', 'xai', '--oauth', '--expires-at-ms', '0'])
        auth = json.loads((self.root / 'seed/auth.json').read_text())
        self.assertEqual(json.loads(auth['entries']['pawork.xai.oauth']['default.meta'])['expires_at_ms'], 0)

    def test_failure_boundaries_preserve_config_and_secrets(self):
        with self.instance.locked():
            self.instance.inject()
            with patch.dict(os.environ, {'PAWORK_MOCK_STATE_DIR': str(self.root / 'other')}):
                with self.assertRaises(RuntimeError):
                    run_instance.Instance().inject()
            edited = self.config.read_bytes() + b'\n# user edit\n'
            self.config.write_bytes(edited)
            with self.assertRaises(RuntimeError):
                self.instance.restore()
            self.assertEqual(self.config.read_bytes(), edited)
            self.assertTrue(self.instance.backup.exists())
            self.assertIsNotNone(self.instance.owner())
            self.config.write_bytes(self.instance.expected.read_bytes())
            with patch.object(run_instance, 'atomic_write', side_effect=OSError('simulated disk failure')):
                with self.assertRaises(OSError):
                    self.instance.restore()
            self.assertTrue(self.instance.backup.exists())
            self.assertIsNotNone(self.instance.owner())
            self.instance.restore()
            self.instance.binary = Path('/usr/bin/false')
            with self.assertRaises(RuntimeError):
                self.instance.start()
            self.assertEqual(self.config.read_bytes(), self.original)
            self.assertIsNone(self.instance.process('server'))
            self.assertIsNone(self.instance.owner())
        with patch.dict(os.environ, {'PAWORK_MOCK_STATE_DIR': str(Path.home())}):
            with self.assertRaises(ValueError):
                run_instance.Instance()
        fixture_root = self.root / 'fixtures'
        channel = fixture_root / 'deepseek'
        channel.mkdir(parents=True)
        secret_file = fixture_root / 'secret.sse'
        secret_file.write_bytes(b'synthetic-secret')
        (channel / 'leak.sse').symlink_to(secret_file)
        (channel / 'chat-completions.x').mkdir()
        self.assertIsNone(server.find_fixture(fixture_root, 'deepseek', 'chat', 'x/../../secret', None))
        self.assertIsNone(server.find_fixture(fixture_root, 'deepseek', 'chat', '', 'leak.sse'))
        sanitized = capture.scrub_json(b'{"workspace_id":123,"account":{"id":"private"}}')
        self.assertEqual(json.loads(sanitized), {'workspace_id': '[REDACTED]', 'account': '[REDACTED]'})
        synthetic_key = 'sk-' + 'x' * 24
        self.assertNotIn(synthetic_key, str(capture.check_sanitized('example', synthetic_key.encode(), [])))
        hits = []
        class Redirect(BaseHTTPRequestHandler):
            def do_GET(self):
                hits.append(self.path)
                self.send_response(302)
                self.send_header('Location', '/credential-receiver')
                self.send_header('Content-Length', '0')
                self.end_headers()
            def log_message(self, *_):
                pass
        with ThreadingHTTPServer(('127.0.0.1', 0), Redirect) as http:
            thread = threading.Thread(target=http.serve_forever)
            thread.start()
            try:
                ok, _ = capture.try_request('GET', 'http://127.0.0.1:' + str(http.server_port) + '/redirect', {'Authorization': 'Bearer synthetic-only'})
                self.assertFalse(ok)
                self.assertEqual(hits, ['/redirect'])
            finally:
                http.shutdown()
                thread.join()


if __name__ == '__main__':
    unittest.main()
