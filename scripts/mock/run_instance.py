#!/usr/bin/env python3
"""隔离 mock 实例；Global config 临时替换、独占并原样恢复（方案 A）。

start 后台运行；run 前台等待（适合常驻 exec）；stop 停止并恢复；
status 查看；seed 注入八通道凭证；env 输出 shell exports；desktop 打开平行 bundle。
不提供递归 clean 或 host wrapper；stop 保留日志，关闭 Desktop 后可自行处理状态目录。

环境变量：MOCK_PORT（8787）、PAWORK_MOCK_INSTANCE（mock）、PAWORK_MOCK_STATE_DIR、
PAWORK_BIN、PAWORK_MOCK_BUNDLE。PAWORK_MOCK_GLOBAL_CONFIG 仅供脚本隔离测试重定向，
不改变 Pawork 的配置查找规则。所有 start/stop 必须使用相同的 state/config 参数。
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import fcntl
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request

REPO = Path(__file__).resolve().parents[2]
KEY_CHANNELS = ('glm-coding', 'opencode-go', 'qwen-token-plan', 'deepseek', 'kimi-platform')
OAUTH_CHANNELS = ('xai', 'kimi-code', 'chatgpt')


def atomic_write(path, data, mode=0o600):
    fd, name = tempfile.mkstemp(dir=path.parent, prefix='.' + path.name)
    try:
        with os.fdopen(fd, 'wb') as handle:
            os.fchmod(handle.fileno(), mode)
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(name, path)
    finally:
        Path(name).unlink(missing_ok=True)


class Instance:
    def __init__(self):
        self.state = Path(os.environ.get('PAWORK_MOCK_STATE_DIR', '/tmp/pawork-mock')).expanduser().resolve()
        self.config = Path(os.environ.get('PAWORK_MOCK_GLOBAL_CONFIG', str(Path.home() / 'Library/Application Support/dev.pawork.pawork/config.toml'))).expanduser().absolute()
        self.name = os.environ.get('PAWORK_MOCK_INSTANCE', 'mock')
        self.port = int(os.environ.get('MOCK_PORT', '8787'))
        if not re.fullmatch(r'[a-zA-Z0-9_-]+', self.name) or not 1 <= self.port <= 65535:
            raise ValueError('invalid instance name or port')
        if self.state in (Path('/'), Path.home().resolve(), REPO) or self.state in REPO.parents or self.state in self.config.parents:
            raise ValueError('state directory must be a dedicated mock directory')
        self.binary = Path(os.environ.get('PAWORK_BIN', str(REPO / 'target/debug/pawork'))).absolute()
        self.data, self.home = self.state / 'data', self.state / 'home'
        self.socket = self.data / ('pawork-gui-' + self.name + '.sock')
        if len(os.fsencode(self.socket)) >= 104:
            raise ValueError('state directory is too long for a macOS UDS socket')
        self.children = {}
        self.lease = self.config.with_name(self.config.name + '.mock-owner')
        self.backup = self.state / 'config.backup.json'
        self.expected = self.state / 'config.mock.toml'
        self.env = dict(os.environ, PAWORK_DATA_DIR=str(self.data), PAWORK_HOME=str(self.home))
        for key in list(self.env):
            if key.startswith('PAWORK_API_KEY_'):
                del self.env[key]

    @contextmanager
    def locked(self):
        self.config.parent.mkdir(parents=True, exist_ok=True)
        # Advisory lock serializes commands; the persistent lease spans start/stop.
        with self.config.with_name(self.config.name + '.mock-lock').open('a') as handle:
            fcntl.flock(handle, fcntl.LOCK_EX)
            yield

    def owner(self):
        return self.lease.read_text() if self.lease.exists() else None

    def mock_config(self):
        base = 'http://127.0.0.1:' + str(self.port)
        lines = ['# pawork-mock temporary config', 'default_provider = "glm-coding"', 'default_model = "glm-5.3"']
        for channel in KEY_CHANNELS + OAUTH_CHANNELS:
            lines += ['[[providers]]', 'id = ' + json.dumps(channel), 'base_url = ' + json.dumps(base)]
        for channel, device, token in (
            ('xai', '/oauth2/device/code', '/oauth2/token'),
            ('kimi-code', '/api/oauth/device_authorization', '/api/oauth/token'),
            ('chatgpt', None, '/oauth/token'),
        ):
            lines += ['[oauth.' + channel + ']', 'client_id = "mock-client"', 'token_url = ' + json.dumps(base + token)]
            if device:
                lines += ['device_auth_url = ' + json.dumps(base + device)]
            else:
                lines += ['auth_url = ' + json.dumps(base + '/oauth/authorize'), 'redirect_uri = "http://localhost:1455/auth/callback"']
        return ('\n'.join(lines) + '\n').encode()

    def inject(self):
        if self.config.is_symlink():
            raise RuntimeError('refusing to replace a symlink config')
        if (self.state / 'config.injected').exists():
            raise RuntimeError('legacy injection detected; restore its config.backup before starting')
        if self.owner() is not None:
            if self.owner() != str(self.state):
                raise RuntimeError('Global config owned by another mock state: ' + self.owner())
            if not self.expected.is_file() or not self.config.is_file() or self.config.read_bytes() != self.expected.read_bytes():
                raise RuntimeError('injected config changed; preserve edits and recover from ' + str(self.backup))
            if self.expected.read_bytes() != self.mock_config():
                raise RuntimeError('port/config changed; stop with the original state first')
            return
        self.state.mkdir(parents=True, exist_ok=True, mode=0o700)
        original = self.config.read_bytes() if self.config.exists() else None
        if original and (b'# pawork-mock' in original or b'pawork-mock begin' in original):
            raise RuntimeError('legacy mock config detected; restore its backup before starting')
        snapshot = {'hex': original.hex() if original is not None else None,
                    'mode': self.config.stat().st_mode & 0o777 if original is not None else 0o600}
        atomic_write(self.backup, json.dumps(snapshot).encode())
        atomic_write(self.expected, self.mock_config())
        atomic_write(self.lease, str(self.state).encode())
        atomic_write(self.config, self.expected.read_bytes())

    def restore(self):
        if self.owner() is None:
            if (self.state / 'config.injected').exists():
                raise RuntimeError('legacy injection detected; preserve state and restore its config.backup manually')
            return
        if self.owner() != str(self.state):
            raise RuntimeError('cannot restore config owned by another state: ' + self.owner())
        snapshot = json.loads(self.backup.read_text())
        original = bytes.fromhex(snapshot['hex']) if snapshot['hex'] is not None else None
        if self.config.is_symlink():
            raise RuntimeError('config became a symlink; backup and owner retained')
        current = self.config.read_bytes() if self.config.exists() else None
        if current != original:
            if current != self.expected.read_bytes():
                raise RuntimeError('config edited during mock use; backup and owner retained: ' + str(self.backup))
            if original is None:
                self.config.unlink()
            else:
                atomic_write(self.config, original, snapshot['mode'])
        self.lease.unlink()
        # Backups are retained for inspection; never recursively delete state.

    def seed(self):
        self.home.mkdir(parents=True, exist_ok=True, mode=0o700)
        for channel in KEY_CHANNELS + OAUTH_CHANNELS:
            form = ['--api-key', 'mock-' + channel] if channel in KEY_CHANNELS else ['--oauth']
            subprocess.run([sys.executable, str(REPO / 'scripts/mock/seed_auth.py'), '--provider', channel, *form], env=self.env, check=True)

    @staticmethod
    def process_identity(pid):
        # A shebang launcher can exec its interpreter after Popen returns; its
        # command changes while PID/start time remain the same.
        result = subprocess.run(['ps', '-p', str(pid), '-o', 'lstart='], text=True, capture_output=True)
        return result.stdout.strip() if result.returncode == 0 else ''

    def process(self, kind):
        child = self.children.get(kind)
        if child is not None and child.poll() is not None:
            return None
        path = self.state / (kind + '.pid.json')
        if not path.exists():
            return None
        record = json.loads(path.read_text())
        return record if record['identity'] and self.process_identity(record['pid']) == record['identity'] else None

    def spawn(self, kind, args):
        with (self.state / (kind + '.log')).open('ab') as log:
            child = subprocess.Popen(args, cwd=REPO, env=self.env, stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
        identity = self.process_identity(child.pid)
        if not identity:
            raise RuntimeError(kind + ' exited during startup')
        atomic_write(self.state / (kind + '.pid.json'), json.dumps({'pid': child.pid, 'identity': identity}).encode())
        self.children[kind] = child
        return child

    def start(self):
        if not self.binary.is_file() or not os.access(self.binary, os.X_OK):
            raise RuntimeError('pawork binary missing: ' + str(self.binary))
        try:
            self.inject()
            self.data.mkdir(parents=True, exist_ok=True)
            if not (self.home / 'auth.json').exists():
                self.seed()
            if not self.process('server'):
                child = self.spawn('server', [sys.executable, str(REPO / 'scripts/mock/server.py'), '--host', '127.0.0.1', '--port', str(self.port)])
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                for _ in range(100):
                    if child.poll() is not None:
                        raise RuntimeError('mock server exited; see server.log')
                    try:
                        with opener.open('http://127.0.0.1:' + str(self.port) + '/__control', timeout=.2):
                            break
                    except OSError:
                        time.sleep(.1)
                else:
                    raise RuntimeError('mock server did not become ready')
                time.sleep(.1)
                if child.poll() is not None:
                    raise RuntimeError('mock server failed to bind; see server.log')
            if not self.process('host'):
                self.socket.unlink(missing_ok=True)
                child = self.spawn('host', [str(self.binary), 'gui', 'serve', '--instance', self.name, '--socket', str(self.socket)])
                for _ in range(100):
                    if child.poll() is not None:
                        raise RuntimeError('host exited; see host.log')
                    if self.socket.is_socket():
                        break
                    time.sleep(.1)
                else:
                    raise RuntimeError('host socket did not appear')
        except BaseException:
            self.stop()
            raise
        print('mock ready: ' + str(self.socket))

    def stop(self):
        if self.owner() not in (None, str(self.state)):
            raise RuntimeError('another mock state owns Global config')
        for kind in ('host', 'server'):
            record = self.process(kind)
            if record:
                try:
                    os.kill(record['pid'], signal.SIGTERM)
                    for _ in range(50):
                        if self.process(kind) is None:
                            break
                        time.sleep(.1)
                    if self.process(kind):
                        os.kill(record['pid'], signal.SIGKILL)
                except ProcessLookupError:
                    pass
            (self.state / (kind + '.pid.json')).unlink(missing_ok=True)
            child = self.children.pop(kind, None)
            if child is not None:
                child.wait(timeout=5)
        self.restore()
        print('mock stopped; config restored; logs retained in ' + str(self.state))

    def desktop(self):
        if not self.process('host'):
            raise RuntimeError('start the mock host first')
        source = Path(os.environ.get('PAWORK_MOCK_BUNDLE', str(REPO / 'target/pawork-desktop-runtime/Pawork.app')))
        bundle = self.state / 'Pawork-mock.app'
        if not bundle.exists():
            shutil.copytree(source, bundle)
        subprocess.run(['open', '-na', str(bundle), '--env', 'PAWORK_DATA_DIR=' + str(self.data), '--env', 'PAWORK_HOME=' + str(self.home), '--args', '--instance', self.name], check=True)
        print('Desktop opened via launchd; close its window separately')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('command', choices=['start', 'run', 'stop', 'status', 'seed', 'env', 'desktop'])
    args = parser.parse_args()
    def interrupted(*_):
        raise KeyboardInterrupt
    # Install before startup so its BaseException rollback also handles TERM.
    signal.signal(signal.SIGTERM, interrupted)
    instance = Instance()
    if args.command == 'env':
        for key, value in [('PAWORK_DATA_DIR', instance.data), ('PAWORK_HOME', instance.home), ('MOCK_PORT', instance.port)]:
            print('export ' + key + '=' + shlex.quote(str(value)))
        for channel in KEY_CHANNELS + OAUTH_CHANNELS:
            print('unset PAWORK_API_KEY_' + channel.upper().replace('-', '_'))
        return
    if args.command == 'status':
        print(json.dumps({'host': instance.process('host'), 'server': instance.process('server'), 'config_owner': instance.owner()}, indent=2))
        return
    with instance.locked():
        if args.command in ('start', 'run'):
            instance.start()
        else:
            getattr(instance, args.command)()
    if args.command == 'run':
        try:
            while instance.process('host'):
                time.sleep(.2)
        except KeyboardInterrupt:
            pass
        finally:
            with instance.locked():
                instance.stop()


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        sys.exit('run-instance: ' + str(error))
    except KeyboardInterrupt:
        sys.exit(130)
