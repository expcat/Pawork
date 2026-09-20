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
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

import capture
import run_instance
import server
import seed_auth
import quota_probe


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
        # 只替换测试 Host 的启动方式，仍创建真实进程/socket 并经过 Instance.spawn。
        # macOS 对新建 shebang 可执行文件的启动审查可能超过就绪窗口；
        # 使用已有 Python 解释器读取脚本，避免把该延迟误判为生命周期回归。
        spawn = self.instance.spawn

        def spawn_fixture(kind, args):
            if kind == 'host' and args[0] == str(self.host):
                args = [sys.executable, *args]
            return spawn(kind, args)

        self.instance.spawn = spawn_fixture

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
        # ADR-061：kimi-code 同时接受 Coding Plan API key 与 OAuth，capture 按序解析。
        self.assertEqual(capture.CHANNELS['kimi-code']['cred_service'], 'pawork.kimi-code')
        self.assertEqual(
            capture.CHANNELS['kimi-code'].get('alt_cred_services'), ('pawork.kimi-code.oauth',)
        )
        # ADR-061 账号索引选中项优先于 legacy default（api key 与 OAuth access 均覆盖）。
        entries = {
            'pawork.kimi-code': {
                'accounts.meta': json.dumps({'selected_credential_id': 'cred_abc_0'}),
                'cred_abc_0': 'selected-key',
                'default': 'legacy-key',
            },
            'pawork.xai.oauth': {
                'accounts.meta': json.dumps({'selected_credential_id': 'cred_xyz_0'}),
                'cred_xyz_0.access': 'selected-access',
                'cred_xyz_0.meta': json.dumps({'expires_at_ms': 9999999999999}),
                'default.access': 'legacy-access',
                'default.meta': json.dumps({'expires_at_ms': 9999999999999}),
            },
        }
        with patch.object(capture, 'load_auth_entries', return_value=entries):
            secret, note = capture.load_channel_credential('kimi-code', capture.CHANNELS['kimi-code'])
            self.assertEqual((secret, note), ('selected-key', None))
            secret, note = capture.load_channel_credential('xai', capture.CHANNELS['xai'])
            self.assertEqual((secret, note), ('selected-access', None))
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

    def test_quota_probe_requires_complete_fresh_windows(self):
        token = self.root / 'gui.token'
        token.write_text('synthetic-probe-token')
        address = str(self.root / 'quota.sock')
        def window(name):
            return {'window': name, 'read': {'status': 'ok', 'snapshot': {
                'scope': {'provider_id': 'opencode-go'}, 'window': name,
                'unit': {'kind': 'percent'}, 'values': {'used': {'kind': 'exact', 'value': 12}},
                'reset': {'kind': 'absolute', 'at': 1_800_000_000_000},
                'provenance': {'stale': False, 'adapter_kind': 'api_key_api'},
                'served_stale': False,
            }}}

        windows = [window(name) for name in ('rolling5h', 'weekly', 'monthly')]
        unknown_reset = [window('rolling5h') for _ in range(3)]
        for item in unknown_reset:
            item['read']['snapshot']['reset'] = {'kind': 'unknown'}
        no_provenance = [window('rolling5h') for _ in range(3)]
        for item in no_provenance:
            del item['read']['snapshot']['provenance']
        for name, entries, expected in [
            ('complete', windows, 0), ('empty', [], 1), ('missing', windows[:2], 1),
            ('failed', [{'window': 'weekly', 'read': {'status': 'failed'}}], 1),
            ('stale', [dict(item, read={'status': 'ok', 'snapshot': {
                **item['read']['snapshot'], 'served_stale': True}}) for item in windows], 1),
            ('unknown-reset', unknown_reset, 1),
            ('missing-provenance', no_provenance, 1),
        ]:
            with self.subTest(name=name), socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
                listener.bind(address)
                listener.listen()
                listener.settimeout(5)
                command = [sys.executable, str(Path(quota_probe.__file__)),
                           '--socket', address, '--token', str(token), '--timeout', '3']
                with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as proc:
                    try:
                        conn, _ = listener.accept()
                        with conn:
                            conn.settimeout(5)
                            self.assertEqual(quota_probe.recv_frame(conn)['type'], 'handshake')
                            quota_probe.send_frame(conn, {'type': 'handshake', 'data': {'status': 'accepted'}})
                            self.assertEqual(quota_probe.recv_frame(conn)['data']['request_id'], 'probe-quota')
                            payload = {'scope': {'provider_id': 'opencode-go'}, 'windows': entries}
                            quota_probe.send_frame(conn, {'type': 'response', 'data': {
                                'request_id': 'probe-quota', 'response': {'type': 'data', 'data': payload}}})
                        out, err = proc.communicate(timeout=5)
                        self.assertEqual(proc.returncode, expected, out + err)
                    finally:
                        if proc.poll() is None:
                            proc.kill()
                            proc.wait()
                Path(address).unlink()


class TestEntryRegression(unittest.TestCase):
    def test_dispatch_rejects_options_and_propagates_cargo_failure(self):
        repo = Path(__file__).resolve().parents[2]
        # Only replace Cargo: execute the real Bash entries and their argument parsing.
        with tempfile.TemporaryDirectory(prefix='pawork-entry-') as directory:
            log = Path(directory) / 'commands'
            cargo = Path(directory) / 'cargo'
            exe_dir = Path(directory) / 'exe dir'
            fake_script = """#!/bin/bash
printf "%s\n" "$*" >> "$COMMAND_LOG"
if [ -n "${PAWORK_BIN:-}" ]; then printf "PAWORK_BIN=%s\n" "$PAWORK_BIN" >> "$COMMAND_LOG"; fi
if [ "$1" = "build" ] && [ "${FAKE_STATUS:-0}" = "0" ]; then
  mkdir -p "$FAKE_EXE_DIR"; exe="$FAKE_EXE_DIR/pawork"
  printf '#!/bin/bash\nexit 0\n' > "$exe"; chmod +x "$exe"
  printf '{"reason":"compiler-artifact","target":{"name":"pawork","kind":["bin"]},"executable":"%s"}\n' "$exe"
fi
exit "${FAKE_STATUS:-0}"
"""
            cargo.write_text(fake_script)
            cargo.chmod(0o755)
            env = {**os.environ, 'COMMAND_LOG': str(log), 'FAKE_STATUS': '0',
                   'FAKE_EXE_DIR': str(exe_dir),
                   'PATH': directory + os.pathsep + os.environ.get('PATH', ''),
                   'CARGO_TARGET_DIR': str(Path(directory) / 'target with spaces')}

            def run(script, args, expected):
                before = len(log.read_text().splitlines()) if log.exists() else 0
                result = subprocess.run(['bash', script, *args],
                                        # Watchdog only: process startup under build load is not
                                        # the behavior this argument/exit-code test measures.
                                        cwd=repo, env=env, capture_output=True, text=True, timeout=60)
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                lines = log.read_text().splitlines() if log.exists() else []
                return lines[before:]

            for package in ('--print', '--help', '--host', '../../auth'):
                self.assertEqual(run('scripts/mock/gate.sh',
                    ['--level', '1', '--packages=' + package], 3), [])
            for args in ([], ['--workspace'], ['unknown'], ['desktop', 'app'], ['--host', 'app']):
                self.assertEqual(run('scripts/test.sh', args, 2), [])
            lines = run('scripts/test.sh', ['providers', 'policy', 'pawork-policy'], 0)
            self.assertEqual(len(lines), 1)
            self.assertEqual(lines[0].count('-p pawork-policy'), 1)
            self.assertIn('pawork-providers/kimi-code', lines[0])
            lines = run('scripts/test.sh', ['--host'], 0)
            self.assertEqual(len(lines), 3)
            self.assertIn('-p pawork', lines[0])
            self.assertIn('--message-format=json', lines[0])
            self.assertIn('--features spawn-e2e --test spawn_e2e', lines[1])
            self.assertEqual(lines[2], 'PAWORK_BIN=' + str(exe_dir / 'pawork'))
            self.assertEqual(run('scripts/test.sh', ['--print', 'client'], 0), [])
            env['FAKE_STATUS'] = '17'
            lines = run('scripts/test.sh', ['--host'], 17)
            self.assertEqual(len(lines), 1, 'failed build must not execute host tests')
            lines = run('scripts/mock/gate.sh',
                        ['--level', '1', '--packages=providers,policy,pawork-policy'], 3)
            self.assertEqual(len(lines), 1)
            self.assertEqual(lines[-1].count('-p pawork-policy'), 1)
            self.assertEqual(lines[-1].count('-p pawork-providers'), 1)


if __name__ == '__main__':
    unittest.main()
