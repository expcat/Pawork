#!/usr/bin/env python3
"""MOCK-3 自测：对照 docs/mock-simulation-plan.md §2.2/§2.3 逐端点核对形状。

三阶段：
  1) fixtures-root=fixtures/mock/synthetic —— fixture 命中优先、定速回放、
     /usage 合法形状 + 故意畸形变体（?fixture= 显式指定）；
  2) fixtures-root=空目录 —— 全 persona 目录端点与三种 transport SSE 兜底；
  3) fixtures-root=fixtures/mock —— 默认录制树命名约定回归（chat_text.sse /
     chat_tool.sse / usage.json 字节级命中；不依赖尚未录制的通道文件）。

/usage 红线校验（三窗独立 / percent 整数 / ok 0-99 / rate-limited 100 /
resetsAt 严格 24 字符真实日历）在本地复刻 channels/api_key.rs 的
parse_go_window / parse_go_reset 语义，用形状比对断言，不跑 pawork。
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from urllib import request as urlrequest
from urllib.error import HTTPError
from urllib.parse import urlencode

REPO = Path(__file__).resolve().parents[2]
SERVER = REPO / "scripts" / "mock" / "server.py"
SYNTHETIC = REPO / "fixtures" / "mock" / "synthetic"

RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append(ok)
    mark = "PASS" if ok else "FAIL"
    suffix = f" :: {detail}" if detail and not ok else ""
    print(f"{mark} {name}{suffix}")


def start_server(fixtures_root, extra=()):
    proc = subprocess.Popen(
        [sys.executable, str(SERVER), "--port", "0", "--fixtures-root", str(fixtures_root), *extra],
        stdout=subprocess.PIPE,
        text=True,
    )
    line = proc.stdout.readline()
    match = re.search(r"http://([\d.]+):(\d+)", line)
    if not match:
        proc.terminate()
        raise RuntimeError(f"server did not report listening: {line!r}")
    return proc, f"http://{match.group(1)}:{match.group(2)}"


def stop_server(proc):
    proc.terminate()
    proc.wait(timeout=5)


def http(method, base, path, token=None, api_key=None, payload=None, query=None):
    url = base + path
    if query:
        url += "?" + urlencode(query)
    headers = {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if api_key:
        headers["x-api-key"] = api_key
    data = json.dumps(payload).encode("utf-8") if payload is not None else None
    if data:
        headers["Content-Type"] = "application/json"
    req = urlrequest.Request(url, data=data, headers=headers, method=method)
    started = time.monotonic()
    try:
        with urlrequest.urlopen(req, timeout=15) as resp:
            status, head, body = resp.status, dict(resp.headers), resp.read()
    except HTTPError as error:
        status, head, body = error.code, dict(error.headers), error.read()
    return status, head, body, time.monotonic() - started


def sse_data_events(body):
    return [
        line[len("data:") :].lstrip()
        for block in body.decode("utf-8").split("\n\n")
        for line in block.splitlines()
        if line.startswith("data:")
    ]


def parse_sse_json(body):
    events = sse_data_events(body)
    if not events or events[-1] != "[DONE]":
        return events, []
    return events, [json.loads(event) for event in events[:-1]]


# --- /usage 红线校验（复刻 channels/api_key.rs 语义） -------------------------


def valid_go_reset(value):
    if not isinstance(value, str) or len(value) != 24:
        return False
    separators = {4: "-", 7: "-", 10: "T", 13: ":", 16: ":", 19: ".", 23: "Z"}
    for position, expected in separators.items():
        if value[position] != expected:
            return False
    spans = {(0, 4), (5, 7), (8, 10), (11, 13), (14, 16), (17, 19), (20, 23)}
    numbers = {}
    for start, end in spans:
        digits = value[start:end]
        if not digits.isdigit():
            return False
        numbers[(start, end)] = int(digits)
    year = numbers[(0, 4)]
    month = numbers[(5, 7)]
    day = numbers[(8, 10)]
    hour = numbers[(11, 13)]
    minute = numbers[(14, 16)]
    second = numbers[(17, 19)]
    if year < 1970 or not 1 <= month <= 12:
        return False
    if hour > 23 or minute > 59 or second > 59:
        return False
    leap = year % 4 == 0 and (year % 100 != 0 or year % 400 == 0)
    month_days = [31, 29 if leap else 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    return 1 <= day <= month_days[month - 1]


def valid_go_window(window):
    if not isinstance(window, dict):
        return False
    percent = window.get("percent")
    if isinstance(percent, bool) or not isinstance(percent, int) or not 0 <= percent <= 100:
        return False
    status = window.get("status")
    if status == "ok" and percent > 99:
        return False
    if status == "rate-limited" and percent != 100:
        return False
    if status not in {"ok", "rate-limited"}:
        return False
    return valid_go_reset(window.get("resetsAt"))


def valid_go_usage(payload):
    usage = payload.get("usage") if isinstance(payload, dict) else None
    if not isinstance(usage, dict):
        return False
    return all(valid_go_window(usage.get(name)) for name in ("rolling", "weekly", "monthly"))


# --- 阶段 1：fixture 优先 + /usage 合法与畸形 ---------------------------------


def phase_fixture_precedence():
    proc, base = start_server(SYNTHETIC, extra=("--chunk-bytes", "64", "--chunk-interval-ms", "10"))
    try:
        status, _, body, _ = http("GET", base, "/models", token="mock-glm-coding")
        expected = (SYNTHETIC / "glm-coding" / "models.json").read_bytes()
        check(
            "fixture precedence: glm-coding /models replays synthetic file bytes",
            status == 200 and body == expected,
            f"status={status} body={body[:120]!r}",
        )

        status, headers, body, elapsed = http(
            "POST", base, "/chat/completions", token="mock-glm-coding",
            payload={"model": "glm-5.3", "stream": True},
        )
        expected_sse = (SYNTHETIC / "glm-coding" / "chat-completions.sse").read_bytes()
        events, parsed = parse_sse_json(body)
        tool_started = any(
            "tool_calls" in json.dumps(chunk) for chunk in parsed
        )
        paced = elapsed >= 0.035  # 5 chunks × 64B，4 次间隔 ≥ 40ms
        check(
            "fixture precedence: glm-coding chat stream replays tool-call SSE bytes",
            status == 200 and body == expected_sse and events[-1] == "[DONE]" and tool_started,
            f"status={status} events={events}",
        )
        check(
            "paced replay: chunked SSE honors --chunk-interval-ms",
            paced,
            f"elapsed={elapsed:.3f}s",
        )
        check(
            "sse response carries text/event-stream content type",
            headers.get("Content-Type", "").startswith("text/event-stream"),
            f"content-type={headers.get('Content-Type')!r}",
        )

        status, _, body, _ = http("GET", base, "/usage", token="mock-opencode-go")
        payload = json.loads(body)
        monthly = payload.get("usage", {}).get("monthly", {})
        check(
            "usage fixture: three-window shape satisfies §2.3 red lines",
            status == 200 and valid_go_usage(payload),
            f"status={status} payload={payload}",
        )
        check(
            "usage fixture: monthly rate-limited pairs with percent 100",
            monthly.get("status") == "rate-limited" and monthly.get("percent") == 100,
            f"monthly={monthly}",
        )

        status, _, body, _ = http(
            "GET", base, "/usage", token="mock-opencode-go",
            query={"fixture": "usage.malformed-percent.json"},
        )
        payload = json.loads(body)
        check(
            "usage malformed variant rejected by §2.3 shape check (ok with 100)",
            status == 200 and not valid_go_usage(payload),
            f"status={status} payload={payload}",
        )

        status, _, body, _ = http(
            "GET", base, "/usage", token="mock-opencode-go",
            query={"fixture": "usage.malformed-resets.json"},
        )
        payload = json.loads(body)
        check(
            "usage malformed variant rejected by §2.3 shape check (Feb 30)",
            status == 200 and not valid_go_usage(payload),
            f"status={status} payload={payload}",
        )
    finally:
        stop_server(proc)


# --- 阶段 2：空 fixtures 根 → 全端点兜底 -------------------------------------


def phase_fallback_shapes():
    with tempfile.TemporaryDirectory() as empty_root:
        proc, base = start_server(Path(empty_root))
        try:
            data_channels = {
                "glm-coding": "glm-5.3",
                "opencode-go": "glm-5.3-flash",
                "qwen-token-plan": "qwen3.8-max",
                "deepseek": "deepseek-v4-pro",
                "kimi-platform": "kimi-k3",
                "kimi-code": "kimi-k3",
            }
            for channel, model_id in data_channels.items():
                status, _, body, _ = http("GET", base, "/models", token=f"mock-{channel}")
                payload = json.loads(body)
                ids = [entry.get("id") for entry in payload.get("data", [])]
                ok = (
                    status == 200
                    and isinstance(payload.get("data"), list)
                    and model_id in ids
                    and payload.get("has_more") is not True
                )
                check(
                    f"catalog fallback: {channel} /models data[] contains {model_id}",
                    ok,
                    f"status={status} payload={payload}",
                )

            status, _, body, _ = http("GET", base, "/models", token="mock-opencode-go")
            ids = [entry.get("id") for entry in json.loads(body).get("data", [])]
            check(
                "catalog fallback: opencode-go /models returns documented table groups",
                status == 200
                and {"grok-4.6", "glm-5.3-flash", "minimax-m3"}.issubset(set(ids)),
                f"ids={ids}",
            )

            status, _, body, _ = http(
                "GET", base, "/models", token="mock-chatgpt",
                query={"client_version": "0.153.0"},
            )
            models = json.loads(body).get("models", [])
            check(
                "catalog fallback: chatgpt /models?client_version models[].slug all visible",
                status == 200
                and models
                and all(entry.get("slug") and entry.get("visibility") == "list" for entry in models),
                f"status={status} models={models}",
            )

            status, _, body, _ = http("GET", base, "/language-models", token="mock-xai")
            models = json.loads(body).get("models", [])
            check(
                "catalog fallback: xai /language-models all text output modalities",
                status == 200
                and models
                and all("text" in entry.get("output_modalities", []) for entry in models),
                f"status={status} models={models}",
            )

            status, _, _, _ = http("GET", base, "/models", api_key="mock-anthropic")
            check("catalog: anthropic has no /models HTTP endpoint (404)", status == 404, f"status={status}")
            status, _, _, _ = http("GET", base, "/models", token="mock-xai")
            check("catalog: xai /models is not its catalog endpoint (404)", status == 404, f"status={status}")

            status, _, body, _ = http("GET", base, "/usage", token="mock-opencode-go")
            payload = json.loads(body)
            check(
                "usage fallback: dynamic three-window payload satisfies §2.3",
                status == 200 and valid_go_usage(payload),
                f"status={status} payload={payload}",
            )
            status, _, _, _ = http("GET", base, "/usage", token="mock-deepseek")
            check("usage: non-Go persona has no /usage endpoint (404)", status == 404, f"status={status}")

            chat_personas = [
                ("glm-coding", "glm-5.3"),
                ("opencode-go", "glm-5.3-flash"),
                ("qwen-token-plan", "qwen3.8-max"),
                ("deepseek", "deepseek-v4-pro"),
                ("kimi-platform", "kimi-k3"),
                ("kimi-code", "kimi-k3"),
                ("xai", "grok-3"),
                ("xai", "grok-4.6"),
            ]
            for channel, model_id in chat_personas:
                status, _, body, _ = http(
                    "POST", base, "/chat/completions", token=f"mock-{channel}",
                    payload={"model": model_id, "stream": True},
                )
                events, parsed = parse_sse_json(body)
                has_text = any(
                    chunk.get("choices") and chunk["choices"][0].get("delta", {}).get("content")
                    for chunk in parsed
                )
                has_finish = any(
                    chunk.get("choices") and chunk["choices"][0].get("finish_reason") == "stop"
                    for chunk in parsed
                )
                ok = status == 200 and events and events[-1] == "[DONE]" and has_text and has_finish
                check(
                    f"chat fallback: {channel} {model_id} text+finish+[DONE] SSE",
                    ok,
                    f"status={status} events={events}",
                )

            responses_personas = [
                ("chatgpt", "gpt-5.6-terra"),
                ("xai", "grok-4"),
                ("opencode-go", "grok-4.6"),
            ]
            for channel, model_id in responses_personas:
                status, _, body, _ = http(
                    "POST", base, "/responses", token=f"mock-{channel}",
                    payload={"model": model_id, "stream": True, "store": False},
                )
                events = sse_data_events(body)
                types = [json.loads(event).get("type") for event in events]
                ok = (
                    status == 200
                    and "response.created" in types
                    and "response.output_text.delta" in types
                    and "response.completed" in types
                )
                check(
                    f"responses fallback: {channel} {model_id} created/delta/completed SSE",
                    ok,
                    f"status={status} types={types}",
                )

            status, _, body, _ = http(
                "POST", base, "/v1/messages", api_key="mock-anthropic",
                payload={"model": "claude-sonnet-4.6", "stream": True},
            )
            events = sse_data_events(body)
            types = [json.loads(event).get("type") for event in events]
            ok = (
                status == 200
                and types
                and types[0] == "message_start"
                and "content_block_delta" in types
                and types[-1] == "message_stop"
            )
            check(
                "messages fallback: anthropic message_start…message_stop SSE via x-api-key",
                ok,
                f"status={status} types={types}",
            )

            cases = [
                ("opencode-go Messages-only model on /chat/completions is 400", "POST",
                 "/chat/completions", "mock-opencode-go", {"model": "minimax-m3"}, 400),
                ("opencode-go responses-model on /chat/completions is 404", "POST",
                 "/chat/completions", "mock-opencode-go", {"model": "grok-4.6"}, 404),
                ("opencode-go chat-model on /responses is 404", "POST",
                 "/responses", "mock-opencode-go", {"model": "glm-5.3"}, 404),
                ("xai chat-model on /responses is 404", "POST",
                 "/responses", "mock-xai", {"model": "grok-3"}, 404),
                ("xai responses-whitelisted grok-4 on /chat/completions is 404", "POST",
                 "/chat/completions", "mock-xai", {"model": "grok-4"}, 404),
                ("qwen-token-plan unregistered model on /chat/completions is 400", "POST",
                 "/chat/completions", "mock-qwen-token-plan", {"model": "gpt-4o"}, 400),
                ("chatgpt persona on /chat/completions is 404", "POST",
                 "/chat/completions", "mock-chatgpt", {"model": "gpt-5.6-terra"}, 404),
            ]
            for name, method, path, token, payload, expected in cases:
                status, _, _, _ = http(method, base, path, token=token, payload=payload)
                check(name, status == expected, f"status={status} expected={expected}")

            status, _, _, _ = http("GET", base, "/models")
            check("missing persona token is rejected 401", status == 401, f"status={status}")
            status, _, _, _ = http("GET", base, "/models", token="mock-unknown")
            check("unknown persona token is rejected 401", status == 401, f"status={status}")
        finally:
            stop_server(proc)


def phase_recorded_tree():
    recorded = REPO / "fixtures" / "mock"
    proc, base = start_server(recorded)
    try:
        cases = [
            (
                "recorded tree: glm-coding default chat replays chat_text.sse bytes",
                "POST",
                "/chat/completions",
                "mock-glm-coding",
                {"model": "glm-5.2"},
                None,
                recorded / "glm-coding" / "chat_text.sse",
            ),
            (
                "recorded tree: glm-coding ?variant=tool replays chat_tool.sse bytes",
                "POST",
                "/chat/completions?variant=tool",
                "mock-glm-coding",
                {"model": "glm-5.2"},
                None,
                recorded / "glm-coding" / "chat_tool.sse",
            ),
            (
                "recorded tree: opencode-go /usage replays usage.json bytes",
                "GET",
                "/usage",
                "mock-opencode-go",
                None,
                None,
                recorded / "opencode-go" / "usage.json",
            ),
        ]
        for name, method, path, token, payload, _, fixture in cases:
            status, _, body, _ = http(method, base, path, token=token, payload=payload)
            expected = fixture.read_bytes()
            check(name, status == 200 and body == expected,
                  f"status={status} body[:80]={body[:80]!r} expected[:80]={expected[:80]!r}")
    finally:
        stop_server(proc)


def main() -> int:
    print(f"repo: {REPO}")
    phase_fixture_precedence()
    phase_fallback_shapes()
    phase_recorded_tree()
    passed = sum(RESULTS)
    total = len(RESULTS)
    verdict = "PASS" if passed == total else "FAIL"
    print(f"SMOKE {verdict} {passed}/{total}")
    return 0 if passed == total else 1


if __name__ == "__main__":
    sys.exit(main())
