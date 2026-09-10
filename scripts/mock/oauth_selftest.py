#!/usr/bin/env python3
"""MOCK-5 OAuth 端点自测（stdlib，不依赖 cargo / pytest）。

进程内起 server.MockProviderServer（空 fixtures root，隔离 MOCK-4 场景），
对 §2.1 三通道的 device / token 端点逐形状断言：

    xai / kimi-code  device_authorization 形状、pending→success 轮询剧本、
                     slow_down、expired_token、invalid_grant、未知 device_code、
                     refresh 轮转与 refresh_no_rotation（缺 refresh_token /
                     expires_in 的保留旧值形状）
    chatgpt          PKCE authorization_code 成功（含可本地提取 account claim
                     的未签名 id_token）、refresh、device 剧本拒绝
    seed_auth        默认 OAuth token（mock-<provider>-access-0）可作为
                     persona 命中对话端点
    /__control       oauth 剧本切换 / 复位与错误分支

用法：python3 scripts/mock/oauth_selftest.py
"""

from __future__ import annotations

import base64
import importlib.util
import json
import sys
import tempfile
import threading
import urllib.error
import urllib.request
from pathlib import Path


def load_server_module():
    path = Path(__file__).with_name("server.py")
    spec = importlib.util.spec_from_file_location("pawork_mock_server", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_seed_auth_module():
    path = Path(__file__).with_name("seed_auth.py")
    spec = importlib.util.spec_from_file_location("pawork_mock_seed_auth", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def post_form(url: str, form: dict) -> tuple:
    data = "&".join(f"{k}={urllib.request.quote(str(v))}" for k, v in form.items()).encode()
    request = urllib.request.Request(
        url, data=data, headers={"Content-Type": "application/x-www-form-urlencoded"}
    )
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, json.loads(response.read())
    except urllib.error.HTTPError as error:
        return error.code, json.loads(error.read())


def post_json(url: str, payload) -> tuple:
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, json.loads(response.read())
    except urllib.error.HTTPError as error:
        return error.code, json.loads(error.read())


def get_json(url: str) -> tuple:
    try:
        with urllib.request.urlopen(url, timeout=10) as response:
            return response.status, json.loads(response.read())
    except urllib.error.HTTPError as error:
        return error.code, json.loads(error.read())


def jwt_payload(token: str) -> dict:
    payload_b64 = token.split(".")[1]
    padded = payload_b64 + "=" * (-len(payload_b64) % 4)
    return json.loads(base64.urlsafe_b64decode(padded))


def device_flow(base: str, path: str):
    status, body = post_form(
        base + path, {"client_id": "mock-client", "scope": "openid offline_access"}
    )
    assert status == 200, body
    return body


def poll(base: str, token_path: str, client_id: str, device_code: str):
    return post_form(
        base + token_path,
        {
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            "device_code": device_code,
            "client_id": client_id,
        },
    )


def set_script(base: str, channel: str, script):
    status, body = post_json(base + "/__control", {"oauth": {channel: script}})
    assert status == 200, body
    return body


def assert_device_shape(auth: dict) -> None:
    for key in ("device_code", "user_code", "verification_uri", "verification_uri_complete"):
        value = auth.get(key)
        assert isinstance(value, str) and value, f"missing {key}: {auth}"
    assert isinstance(auth["expires_in"], int) and auth["expires_in"] > 0, auth
    assert isinstance(auth["interval"], int) and auth["interval"] >= 1, auth


def assert_success_shape(body: dict, channel: str) -> None:
    assert "error" not in body, body
    assert body["access_token"].startswith(f"mock-{channel}-access-"), body
    assert body["token_type"] == "Bearer", body
    assert isinstance(body.get("refresh_token"), str), body
    assert isinstance(body.get("expires_in"), int), body


def main() -> int:
    server_module = load_server_module()
    fixtures_root = Path(tempfile.mkdtemp(prefix="pawork-mock-oauth-selftest-"))
    config = server_module.MockConfig(
        fixtures_root=fixtures_root, chunk_bytes=1024, chunk_interval_ms=0.0, scenarios={}
    )
    server = server_module.MockProviderServer(
        ("127.0.0.1", 0), server_module.MockProviderHandler, config
    )
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}"

    checks = []

    def check(name, fn):
        fn()
        checks.append(name)
        print(f"ok  {name}")

    def xai_device_default():
        auth = device_flow(base, "/oauth2/device/code")
        assert_device_shape(auth)
        status, first = poll(base, "/oauth2/token", "mock-client", auth["device_code"])
        assert status == 200 and first["error"] == "authorization_pending", first
        _, second = poll(base, "/oauth2/token", "mock-client", auth["device_code"])
        assert second["error"] == "authorization_pending", second
        _, third = poll(base, "/oauth2/token", "mock-client", auth["device_code"])
        assert_success_shape(third, "xai")

    check("xai device auth shape + pending_then_success", xai_device_default)

    def xai_slow_down():
        set_script(base, "xai", "slow_down_then_success")
        auth = device_flow(base, "/oauth2/device/code")
        _, first = poll(base, "/oauth2/token", "mock-client", auth["device_code"])
        assert first["error"] == "slow_down", first
        _, second = poll(base, "/oauth2/token", "mock-client", auth["device_code"])
        assert second["error"] == "authorization_pending", second
        _, third = poll(base, "/oauth2/token", "mock-client", auth["device_code"])
        assert_success_shape(third, "xai")

    check("xai slow_down_then_success script", xai_slow_down)

    def xai_expired():
        set_script(base, "xai", "expired_token")
        auth = device_flow(base, "/oauth2/device/code")
        _, first = poll(base, "/oauth2/token", "mock-client", auth["device_code"])
        assert first["error"] == "expired_token", first
        _, second = poll(base, "/oauth2/token", "mock-client", auth["device_code"])
        assert second["error"] == "expired_token", second

    check("xai expired_token script", xai_expired)

    def xai_invalid_grant():
        set_script(base, "xai", "invalid_grant")
        auth = device_flow(base, "/oauth2/device/code")
        _, device_error = poll(base, "/oauth2/token", "mock-client", auth["device_code"])
        assert device_error["error"] == "invalid_grant", device_error
        _, refresh_error = post_form(
            base + "/oauth2/token",
            {"grant_type": "refresh_token", "refresh_token": "old", "client_id": "c"},
        )
        assert refresh_error["error"] == "invalid_grant", refresh_error

    check("xai invalid_grant script (device + refresh)", xai_invalid_grant)

    def xai_unknown_device():
        post_json(base + "/__control", {"oauth": None})
        _, body = poll(base, "/oauth2/token", "mock-client", "not-a-device-code")
        assert body["error"] == "invalid_grant", body

    check("xai unknown device_code -> invalid_grant", xai_unknown_device)

    def xai_refresh_rotation():
        post_json(base + "/__control", {"oauth": None})
        _, body = post_form(
            base + "/oauth2/token",
            {"grant_type": "refresh_token", "refresh_token": "old", "client_id": "c"},
        )
        assert_success_shape(body, "xai")
        assert body["refresh_token"] != "old", body

    check("xai refresh rotates refresh_token", xai_refresh_rotation)

    def xai_refresh_no_rotation():
        set_script(base, "xai", "refresh_no_rotation")
        _, body = post_form(
            base + "/oauth2/token",
            {"grant_type": "refresh_token", "refresh_token": "old", "client_id": "c"},
        )
        assert "error" not in body, body
        assert "refresh_token" not in body, body
        assert "expires_in" not in body, body
        assert body["access_token"].startswith("mock-xai-access-"), body

    check("xai refresh_no_rotation omits refresh_token/expires_in", xai_refresh_no_rotation)

    def refresh_missing_param():
        post_json(base + "/__control", {"oauth": None})
        _, body = post_form(base + "/oauth2/token", {"grant_type": "refresh_token"})
        assert body["error"] == "invalid_request", body

    check("refresh without refresh_token -> invalid_request", refresh_missing_param)

    def kimi_device_flow():
        auth = device_flow(base, "/api/oauth/device_authorization")
        assert_device_shape(auth)
        _, first = poll(base, "/api/oauth/token", "mock-client", auth["device_code"])
        assert first["error"] == "authorization_pending", first
        _, second = poll(base, "/api/oauth/token", "mock-client", auth["device_code"])
        assert second["error"] == "authorization_pending", second
        _, third = poll(base, "/api/oauth/token", "mock-client", auth["device_code"])
        assert_success_shape(third, "kimi-code")

    check("kimi-code device auth + pending_then_success", kimi_device_flow)

    def chatgpt_pkce():
        post_json(base + "/__control", {"oauth": None})
        _, body = post_form(
            base + "/oauth/token",
            {
                "grant_type": "authorization_code",
                "code": "mock-auth-code",
                "redirect_uri": "http://localhost:1455/auth/callback",
                "client_id": "app_EMoamEEZ73f0CkXaXp7hrann",
                "code_verifier": "v" * 64,
            },
        )
        assert_success_shape(body, "chatgpt")
        claims = jwt_payload(body["id_token"])
        account = claims["https://api.openai.com/auth"]["chatgpt_account_id"]
        assert account.startswith("acct-mock-"), claims
        _, refreshed = post_form(
            base + "/oauth/token",
            {"grant_type": "refresh_token", "refresh_token": "old", "client_id": "c"},
        )
        assert_success_shape(refreshed, "chatgpt")
        assert "id_token" not in refreshed, refreshed

    check("chatgpt pkce code + id_token claim + refresh", chatgpt_pkce)

    def chatgpt_refresh_no_rotation():
        set_script(base, "chatgpt", "refresh_no_rotation")
        _, body = post_form(
            base + "/oauth/token",
            {"grant_type": "refresh_token", "refresh_token": "old", "client_id": "c"},
        )
        assert "error" not in body, body
        assert "refresh_token" not in body, body
        assert "expires_in" not in body, body
        assert "id_token" not in body, body
        assert body["access_token"].startswith("mock-chatgpt-access-"), body

    check(
        "chatgpt refresh_no_rotation omits refresh_token/expires_in/id_token",
        chatgpt_refresh_no_rotation,
    )

    def chatgpt_rejects_device_script():
        status, body = post_json(
            base + "/__control", {"oauth": {"chatgpt": "pending_then_success"}}
        )
        assert status == 400, body
        status, body = post_json(
            base + "/__control", {"oauth": {"nope": "immediate_success"}}
        )
        assert status == 400, body
        status, body = post_json(
            base + "/__control", {"oauth": {"xai": "no_such_script"}}
        )
        assert status == 400, body
        status, body = post_json(base + "/__control", {"oauth": "bad"})
        assert status == 400, body

    check("/__control rejects bad oauth channels/scripts", chatgpt_rejects_device_script)

    def chatgpt_device_grant_rejected():
        _, body = post_form(
            base + "/oauth/token",
            {
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
                "device_code": "x",
                "client_id": "c",
            },
        )
        assert body["error"] == "unsupported_grant_type", body

    check("chatgpt token rejects device grant", chatgpt_device_grant_rejected)

    def control_snapshot_and_reset():
        set_script(base, "xai", "invalid_grant")
        status, body = get_json(base + "/__control")
        assert status == 200, body
        assert body["oauth"]["xai"] == "invalid_grant", body
        assert body["oauth"]["kimi-code"] == "pending_then_success", body
        status, body = post_json(base + "/__control", {"oauth": None})
        assert status == 200, body
        assert body["oauth"]["xai"] == "pending_then_success", body

    check("/__control oauth snapshot + reset", control_snapshot_and_reset)

    def device_page():
        with urllib.request.urlopen(
            base + "/device?user_code=ABCD-1234", timeout=10
        ) as response:
            page = response.read().decode("utf-8")
            assert response.status == 200 and "ABCD-1234" in page, page

    check("GET /device verification page", device_page)

    def issued_token_persona():
        body = json.dumps({"model": "grok-4.6", "stream": True, "messages": []}).encode()
        request = urllib.request.Request(
            base + "/chat/completions",
            data=body,
            headers={
                "Content-Type": "application/json",
                "Authorization": "Bearer mock-xai-access-99",
            },
        )
        with urllib.request.urlopen(request, timeout=10) as response:
            payload = response.read().decode("utf-8")
            assert response.status == 200, payload
            assert "data: " in payload, payload

    check("mock-issued oauth token acts as persona", issued_token_persona)

    def seed_auth_default_tokens():
        seed = load_seed_auth_module()
        home = Path(tempfile.mkdtemp(prefix="pawork-mock-seed-selftest-"))
        # 先 seed API key 再 seed OAuth：合并不得残留 default 槽——否则
        # resolve.rs 在 selected=null 时直接采用 default API key，xAI 双认证
        # 优先 API key，把 OAuth（含请求前 refresh）整条短路。
        seed.main(["--home", str(home), "--provider", "xai", "--api-key", "mock-xai"])
        seed.main(["--home", str(home), "--provider", "xai", "--oauth"])
        raw = json.loads((home / "auth.json").read_text(encoding="utf-8"))
        provider_entry = raw["entries"]["pawork.xai"]
        assert "default" not in provider_entry, provider_entry
        index = json.loads(provider_entry["accounts.meta"])
        assert [a["credential_id"] for a in index["accounts"]] == ["default-oauth"], index
        assert index["selected_credential_id"] is None, index
        entry = raw["entries"]["pawork.xai.oauth"]
        assert entry["default.access"] == "mock-xai-access-0", entry
        assert entry["default.refresh"] == "mock-xai-refresh-0", entry
        body = json.dumps({"model": "grok-4.6", "stream": True, "messages": []}).encode()
        request = urllib.request.Request(
            base + "/chat/completions",
            data=body,
            headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {entry['default.access']}",
            },
        )
        with urllib.request.urlopen(request, timeout=10) as response:
            payload = response.read().decode("utf-8")
            assert response.status == 200 and "data: " in payload, payload

    check("seed_auth default oauth token acts as persona", seed_auth_default_tokens)

    server.shutdown()
    server.server_close()
    print(f"\n{len(checks)} checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
