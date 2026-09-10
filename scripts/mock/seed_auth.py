#!/usr/bin/env python3
"""MOCK-5 凭证注入工具：直接生成 auth.json，跳过 OAuth HTTP 流。

形状严格对齐 crates/auth/src/file_backend.rs 与 default_credential.rs 的
解析（形状取自 pawork auth login / set-key 的真实落盘产物）：

    API key：entries["pawork.<provider>"] = {
        "default": "<明文>",
        "accounts.meta": "<索引 JSON，default-api-key 条目>",
    }
    OAuth：entries["pawork.<provider>.oauth"] = {
        "default.access" / "default.refresh" / "default.meta"，
        meta 含远期 expires_at_ms（避免每次请求触发 refresh）；
        索引写在 entries["pawork.<provider>"]["accounts.meta"]（default-oauth）。
    }

用法示例：
    PAWORK_HOME=/tmp/x python3 scripts/mock/seed_auth.py --provider glm-coding \
        --api-key mock-glm-coding
    PAWORK_HOME=/tmp/x python3 scripts/mock/seed_auth.py --provider xai --oauth \
        --expires-at-ms 9999999999999

默认 OAuth token 与 mock server 签发形状一致（mock-<provider>-access-0），
可直接作为 persona 命中对话端点；chatgpt OAuth 默认 account id 为
acct-mock-seed（缺值会在 seed 阶段报错，不写出装配必炸的 auth.json）。

默认追加合并到既有 auth.json（0600、临时文件 + rename 原子写）。
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
import time
from pathlib import Path


DAY_MS = 24 * 60 * 60 * 1000


def mask_secret(value: str) -> str:
    """复刻 MaskedCredential::mask 的三档脱敏（Unicode 标量计数）。"""
    count = len(value)
    if count <= 4:
        return "\u2022" * 4
    if count <= 8:
        return "\u2026" + value[-2:]
    return value[:3] + "\u2026" + value[-4:]


def load_entries(path: Path) -> dict:
    if not path.exists():
        return {}
    raw = json.loads(path.read_text(encoding="utf-8"))
    entries = raw.get("entries")
    if not isinstance(entries, dict):
        raise SystemExit(f"auth.json 缺少 entries 对象：{path}")
    return entries


def atomic_write_0600(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    body = json.dumps(payload, ensure_ascii=False, indent=2) + "\n"
    fd, tmp_name = tempfile.mkstemp(
        dir=str(path.parent), prefix=f".{path.name}.", suffix=".tmp"
    )
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(body)
        os.replace(tmp_name, path)
    except BaseException:
        Path(tmp_name).unlink(missing_ok=True)
        raise


def index_json(kind: str, created_at_ms=None) -> str:
    """accounts.meta 索引；kind=oauth -> default-oauth，否则 default-api-key。"""
    entry = {
        "credential_id": "default-oauth" if kind == "oauth" else "default-api-key",
        "kind": kind,
        "display_name": "Default OAuth" if kind == "oauth" else "Default API key",
        "created_at_ms": created_at_ms,
    }
    return json.dumps(
        {
            "version": 1,
            "revision": 1,
            "selection_mode": "manual",
            "accounts": [entry],
            "selected_credential_id": None,
        },
        ensure_ascii=False,
        separators=(",", ":"),
    )


def oauth_meta(access_token: str, created_ms: int, expires_at_ms: int, scopes, account_id):
    return json.dumps(
        {
            "masked": {"masked": mask_secret(access_token)},
            "created_at_ms": created_ms,
            "expires_at_ms": expires_at_ms,
            "scopes": list(scopes),
            "account_id": account_id,
        },
        ensure_ascii=False,
        separators=(",", ":"),
    )


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        description="生成 Pawork mock auth.json（API key 或 OAuth 形态）"
    )
    parser.add_argument("--home", help="目标 PAWORK_HOME（默认读环境变量）")
    parser.add_argument("--provider", required=True, help="provider id，如 xai / glm-coding")
    form = parser.add_mutually_exclusive_group(required=True)
    form.add_argument("--api-key", help="按 API key 形态写入（值为明文）")
    form.add_argument(
        "--oauth", action="store_true", help="按 OAuth default 条目写入（mock token）"
    )
    parser.add_argument(
        "--access-token", default=None,
        help="OAuth access token（默认 mock-<provider>-access-0，mock persona 可识别）",
    )
    parser.add_argument(
        "--refresh-token", default=None,
        help="OAuth refresh token（默认 mock-<provider>-refresh-0；传空串则不写 refresh 槽）",
    )
    parser.add_argument(
        "--expires-at-ms",
        type=int,
        default=None,
        help="OAuth 到期毫秒时间戳（默认 now + 365 天；须远期以免每次请求触发 refresh）",
    )
    parser.add_argument("--scopes", nargs="*", default=[], help="OAuth scopes")
    parser.add_argument(
        "--account-id", default=None,
        help="ChatGPT 路由用 account id（写 meta，非 secret；chatgpt OAuth 默认 acct-mock-seed）",
    )
    args = parser.parse_args(argv)

    home = Path(args.home or os.environ.get("PAWORK_HOME", "")).expanduser()
    if not str(home) or str(home) == ".":
        parser.error("--home 或环境变量 PAWORK_HOME 必须指定目标目录")
    auth_path = home / "auth.json"
    entries = load_entries(auth_path)
    now_ms = int(time.time() * 1000)

    if args.api_key is not None:
        if not args.api_key.strip():
            parser.error("--api-key 不能为空")
        provider_entries = entries.setdefault(f"pawork.{args.provider}", {})
        provider_entries["default"] = args.api_key
        provider_entries["accounts.meta"] = index_json("api_key")
        written = "api_key default"
    else:
        account_id = args.account_id
        if args.provider == "chatgpt":
            # chatgpt OAuth 装配 fail-closed：缺 account id 的凭证必然装配失败。
            # 缺省补 acct-mock-seed；显式传空值则在 seed 阶段报错，不写出必炸的 auth.json。
            if account_id is None:
                account_id = "acct-mock-seed"
            if not account_id.strip():
                parser.error(
                    "--account-id 不能为空：chatgpt OAuth 装配需要 account id（默认 acct-mock-seed）"
                )
        access = args.access_token or f"mock-{args.provider}-access-0"
        refresh = args.refresh_token
        if refresh is None:
            refresh = f"mock-{args.provider}-refresh-0"
        expires = args.expires_at_ms if args.expires_at_ms is not None else (now_ms + 365 * DAY_MS)
        oauth_entries = {"default.access": access}
        if refresh != "":
            oauth_entries["default.refresh"] = refresh
        oauth_entries["default.meta"] = oauth_meta(
            access, now_ms, expires, args.scopes, account_id
        )
        entries[f"pawork.{args.provider}.oauth"] = oauth_entries
        provider_entries = entries.setdefault(f"pawork.{args.provider}", {})
        # 合并 seed 时残留的 default API key 槽会在 selected=null 时被
        # resolve.rs 直接读取（resolve 链第二跳），xAI 双认证优先 API key，
        # 把刚 seed 的 OAuth 整条短路（含请求前 refresh）。seed --oauth 的
        # 语义是 OAuth 为生效凭证，因此移除 default 槽——与全新 HOME 上真实
        # auth login 的落盘形态一致（无 default 槽、索引仅 default-oauth、
        # selected=null，见 accounts.rs store_legacy_oauth）。
        provider_entries.pop("default", None)
        provider_entries["accounts.meta"] = index_json("oauth", created_at_ms=now_ms)
        written = f"oauth default.access/.refresh/.meta (expires_at_ms={expires})"

    atomic_write_0600(auth_path, {"version": 1, "entries": entries})
    print(f"seeded {args.provider} {written} -> {auth_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
