#!/usr/bin/env python3
"""MOCK-6 三窗额度探针：经真实 GUI Connection Protocol 查询 QuotaOverview。

用途：opencode-go 的 GET /usage 三窗展示宿主路径是 GUI 协议
AppQuery::QuotaOverview（crates/app/src/provider_quota.rs），CLI 无独立
子命令；本探针按 pawork-client 同款线格式（LE u32 长度前缀 + JSON 帧）
连到运行中的 gui serve，握手后发送 account quota 查询并打印三窗结果，
作为 mock 环境 CLI 侧脚本化证据。

用法：
    python3 scripts/mock/quota_probe.py --socket <sock> --token <token-file> \
        [--provider opencode-go] [--credential default-api-key]
"""

from __future__ import annotations

import argparse
import json
import socket
import struct
import sys


def send_frame(sock: socket.socket, payload: dict) -> None:
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    sock.sendall(struct.pack("<I", len(body)) + body)


def recv_exact(sock: socket.socket, count: int) -> bytes:
    chunks = []
    while count > 0:
        chunk = sock.recv(count)
        if not chunk:
            raise SystemExit("connection closed while reading frame")
        chunks.append(chunk)
        count -= len(chunk)
    return b"".join(chunks)


def recv_frame(sock: socket.socket) -> dict:
    (length,) = struct.unpack("<I", recv_exact(sock, 4))
    if length > 4 * 1024 * 1024:
        raise SystemExit(f"frame too large: {length}")
    return json.loads(recv_exact(sock, length))


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="GUI 协议 QuotaOverview 探针")
    parser.add_argument("--socket", required=True, help="gui serve UDS socket 路径")
    parser.add_argument("--token", required=True, help="gui token 文件路径")
    parser.add_argument("--provider", default="opencode-go")
    parser.add_argument("--credential", default="default-api-key")
    parser.add_argument("--timeout", type=float, default=15.0)
    args = parser.parse_args(argv)

    token = open(args.token, encoding="utf-8").read().strip()
    request = {
        "type": "handshake",
        "data": {
            "request_id": "probe-hs",
            "client_name": "mock-quota-probe",
            "client_version": "0.1",
            "supported_api_versions": [{"major": 1, "minor": 16}],
            "authentication": {"scheme": "pawork-token", "proof": token},
        },
    }
    query = {
        "type": "query",
        "data": {
            "api_version": {"major": 1, "minor": 16},
            "request_id": "probe-quota",
            "source": {"type": "remote_gui", "client_id": "probe", "connection_id": "probe"},
            "identity": {"type": "local_user", "actor_id": "actor-probe"},
            "issued_at": 1,
            "query": {
                "method": "quota_overview",
                "params": {
                    "query": {
                        "tenant_id": "local",
                        "account_id": "local/default",
                        "provider_id": args.provider,
                        "credential_id": args.credential,
                        "unit": {"kind": "percent"},
                    }
                },
            },
        },
    }

    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(args.timeout)
        sock.connect(args.socket)
        send_frame(sock, request)
        handshake = recv_frame(sock)
        status = handshake.get("data", {}).get("status")
        if status != "accepted":
            print(json.dumps(handshake, ensure_ascii=False))
            return 1
        send_frame(sock, query)
        while True:
            frame = recv_frame(sock)
            if frame.get("type") != "response":
                continue
            data = frame.get("data", {})
            response = data.get("response", {})
            if isinstance(response, dict) and response.get("type") == "error":
                print(json.dumps(response, ensure_ascii=False))
                return 1
            payload = response.get("data")
            if payload is None:
                print(json.dumps(frame, ensure_ascii=False))
                return 1
            windows = payload.get("windows", [])
            provider = payload.get("scope", {}).get("provider_id", "-")
            for entry in windows:
                read = entry.get("read", {})
                snapshot = read.get("snapshot", {})
                values = snapshot.get("values", {})
                reset = snapshot.get("reset", {})
                reset_at = reset.get("at", "-") if reset.get("kind") == "absolute" else "-"
                used = values.get("used")
                if isinstance(used, dict):
                    used = used.get("value")
                print(
                    f"{provider} {entry.get('window')}: "
                    f"{read.get('status', 'ok')} used={used}% "
                    f"resets={reset_at}"
                )
            print(f"windows={len(windows)} unit=percent")
            return 0


if __name__ == "__main__":
    sys.exit(main())
