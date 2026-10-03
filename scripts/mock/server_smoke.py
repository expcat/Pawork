#!/usr/bin/env python3
"""MOCK-3 自测：对照 Git 历史 37fae8f3:docs/mock-simulation-plan.md §2.2/§2.3 逐端点核对形状。

三阶段：
  1) fixtures-root=fixtures/mock/synthetic —— fixture 命中优先、定速回放、
     /usage 合法形状 + 故意畸形变体（?fixture= 显式指定）；
  2) fixtures-root=空目录 —— 全 persona 目录端点、三种 transport SSE 兜底与媒体任务；
  3) fixtures-root=fixtures/mock —— 默认录制树命名约定回归（chat_text.sse /
     chat_tool.sse / usage.json 字节级命中），并在同一服务验证错误、截断、定速与场景触发。

/usage 形状复用 capture.check_usage；HTTP 自测不替代 Pawork 的真实解析回归。
"""

from __future__ import annotations

import json
import re
import struct
import subprocess
import sys
import tempfile
import time
import zlib
from pathlib import Path
from urllib import request as urlrequest
from urllib.error import HTTPError
from urllib.parse import urlencode

from capture import check_usage

REPO = Path(__file__).resolve().parents[2]
SERVER = REPO / "scripts" / "mock" / "server.py"
FIXTURES = REPO / "fixtures" / "mock"
SYNTHETIC = FIXTURES / "synthetic"
MANIFEST = FIXTURES / "scenarios" / "manifest.json"
FIXDATE_RE = re.compile(r"^[A-Z][a-z]{2}, \d{2} [A-Z][a-z]{2} \d{4} \d{2}:\d{2}:\d{2} GMT$")

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


def http(method, base, path, token=None, api_key=None, payload=None, query=None, extra_headers=None):
    url = base + path
    if query:
        url += "?" + urlencode(query)
    headers = dict(extra_headers or {})
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
            status == 200
            and normalize_stream_ids(body) == normalize_stream_ids(expected_sse)
            and events[-1] == "[DONE]"
            and tool_started,
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
            status == 200 and not check_usage(payload),
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
            status == 200 and bool(check_usage(payload)),
            f"status={status} payload={payload}",
        )

        status, _, body, _ = http(
            "GET", base, "/usage", token="mock-opencode-go",
            query={"fixture": "usage.malformed-resets.json"},
        )
        payload = json.loads(body)
        check(
            "usage malformed variant rejected by §2.3 shape check (Feb 30)",
            status == 200 and bool(check_usage(payload)),
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
                "kimi-code": "kimi-for-coding",
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
                status == 200 and not check_usage(payload),
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
                ("kimi-code", "kimi-for-coding"),
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
                ("opencode-go unregistered text model streams on /chat/completions", "POST",
                 "/chat/completions", "mock-opencode-go", {"model": "brandnew-text-1"}, 200),
                ("opencode-go unregistered grok-family model streams on /responses", "POST",
                 "/responses", "mock-opencode-go", {"model": "grok-4.5"}, 200),
                ("opencode-go messages-family qwen on /chat/completions is 400", "POST",
                 "/chat/completions", "mock-opencode-go", {"model": "qwen3.5-plus"}, 400),
                ("opencode-go non-text model on /chat/completions is 400", "POST",
                 "/chat/completions", "mock-opencode-go", {"model": "gpt-image-1"}, 400),
                ("xai chat-model on /responses is 404", "POST",
                 "/responses", "mock-xai", {"model": "grok-3"}, 404),
                ("xai responses-whitelisted grok-4 on /chat/completions is 404", "POST",
                 "/chat/completions", "mock-xai", {"model": "grok-4"}, 404),
                ("qwen-token-plan unregistered text model streams on /chat/completions", "POST",
                 "/chat/completions", "mock-qwen-token-plan", {"model": "gpt-4o"}, 200),
                ("qwen-token-plan unsupported non-text model on /chat/completions is 400", "POST",
                 "/chat/completions", "mock-qwen-token-plan", {"model": "qwen-audio"}, 400),
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
            phase_media_tasks(base)
        finally:
            stop_server(proc)


def phase_recorded_tree():
    recorded = FIXTURES
    proc, base = start_server(recorded, extra=("--chunk-bytes", "64"))
    try:
        cases = [
            (
                "recorded tree: glm-coding default chat replays chat_text.sse (ids uniquified)",
                "POST",
                "/chat/completions",
                "mock-glm-coding",
                {"model": "glm-5.2"},
                None,
                recorded / "glm-coding" / "chat_text.sse",
            ),
            (
                "recorded tree: glm-coding ?variant=tool replays chat_tool.sse (ids uniquified)",
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
            if name.endswith("usage.json bytes"):
                same = body == expected and not check_usage(json.loads(body))
            else:
                # 流式回放 id 逐响应唯一化（-m<salt> 后缀），归一后应与录制字节一致。
                same = normalize_stream_ids(body) == normalize_stream_ids(expected)
            check(name, status == 200 and same,
                  f"status={status} body[:80]={body[:80]!r} expected[:80]={expected[:80]!r}")

        phase_tool_loop(base)
        phase_scenarios(base)
    finally:
        stop_server(proc)


ID_SUFFIX_RE = re.compile(r"-m[0-9a-f]{12,}")


def normalize_stream_ids(body):
    """SSE 事件逐条 canonical JSON（排序键）并剥离 -m<salt> 唯一化后缀；
    与录制字节的空格/键序差异无关，只比事件内容。"""
    if isinstance(body, bytes):
        text = body.decode("utf-8")
    else:
        text = body
    events = []
    for block in text.split("\n\n"):
        for line in block.splitlines():
            if not line.startswith("data:"):
                continue
            raw = line[len("data:"):].lstrip()
            if raw == "[DONE]":
                events.append(raw)
                continue
            canonical = json.dumps(
                json.loads(raw), ensure_ascii=False, sort_keys=True, separators=(",", ":")
            )
            events.append(ID_SUFFIX_RE.sub("", canonical))
    return events


def chat_tool_call_id(body):
    for event in sse_data_events(body):
        if event == "[DONE]":
            continue
        for choice in json.loads(event).get("choices") or []:
            for call in (choice.get("delta") or {}).get("tool_calls") or []:
                if call.get("id"):
                    return call["id"]
    return None


def phase_tool_loop(base):
    """工具循环闭环：MOCK:TOOL 首轮工具调用 → 回传工具结果 → 终答文本；
    重复工具 Run 的工具 id 不再撞事件 UNIQUE（id 逐响应唯一化）。"""
    user = {"role": "user", "content": "MOCK:TOOL check the weather"}
    status, _, body, _ = http(
        "POST", base, "/chat/completions", token="mock-glm-coding",
        payload={"model": "glm-5.2", "messages": [user]},
    )
    first_call = chat_tool_call_id(body)
    check("tool loop: MOCK:TOOL keyword drives chat tool-call stream",
          status == 200 and first_call is not None,
          f"status={status} call_id={first_call!r}")

    followup = {
        "model": "glm-5.2",
        "messages": [
            user,
            {"role": "assistant", "tool_calls": [{
                "id": first_call or "call_x", "type": "function",
                "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"},
            }]},
            {"role": "tool", "tool_call_id": first_call or "call_x", "content": "sunny"},
        ],
    }
    status, _, body2, _ = http(
        "POST", base, "/chat/completions", token="mock-glm-coding", payload=followup,
    )
    events, parsed = parse_sse_json(body2)
    has_tool = any(
        (choice.get("delta") or {}).get("tool_calls")
        for event in parsed
        for choice in event.get("choices") or []
    )
    check("tool loop: tool result in request yields final text, not another tool call",
          status == 200 and events and events[-1] == "[DONE]" and not has_tool,
          f"status={status} has_tool={has_tool} tail={events[-1:]!r}")

    status, _, body3, _ = http(
        "POST", base, "/chat/completions", token="mock-glm-coding",
        payload={"model": "glm-5.2", "messages": [user]},
    )
    second_call = chat_tool_call_id(body3)
    check("tool loop: repeated tool run gets a fresh tool_call_id",
          status == 200 and second_call is not None and second_call != first_call,
          f"first={first_call!r} second={second_call!r}")

    # Responses transport：MOCK:TOOL → function_call；function_call_output → 终答文本。
    responses_user = {"model": "gpt-5.6-codex", "input": [{"type": "message", "role": "user",
                      "content": [{"type": "input_text", "text": "MOCK:TOOL weather"}]}]}
    status, _, body, _ = http(
        "POST", base, "/responses", token="mock-chatgpt", payload=responses_user,
    )
    check("tool loop: MOCK:TOOL drives responses function_call stream",
          status == 200 and b"function_call" in body, f"status={status}")
    responses_user["input"] = responses_user["input"] + [
        {"type": "function_call", "call_id": "call_prev", "name": "get_weather",
         "arguments": "{\"city\":\"Paris\"}"},
        {"type": "function_call_output", "call_id": "call_prev", "output": "sunny"},
    ]
    status, _, body, _ = http(
        "POST", base, "/responses", token="mock-chatgpt", payload=responses_user,
    )
    check("tool loop: responses function_call_output yields final text",
          status == 200 and b"output_text" in body and b"function_call_arguments" not in body,
          f"status={status}")

    # Messages transport：tool_result block → 终答文本。
    messages_followup = {"model": "claude-sonnet-4-5", "messages": [
        {"role": "user", "content": "MOCK:TOOL weather"},
        {"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_prev", "name": "get_weather",
             "input": {"city": "Paris"}}]},
        {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_prev", "content": "sunny"}]},
    ]}
    status, _, body, _ = http(
        "POST", base, "/v1/messages", api_key="mock-anthropic", payload=messages_followup,
    )
    check("tool loop: messages tool_result yields final text",
          status == 200 and b"text_delta" in body and b"tool_use" not in body,
          f"status={status}")

    # 成功路径：MOCK:TOOLFILE → read_file（Pawork 内建只读工具）工具调用流。
    status, _, body, _ = http(
        "POST", base, "/chat/completions", token="mock-glm-coding",
        payload={"model": "glm-5.2", "messages": [
            {"role": "user", "content": "MOCK:TOOLFILE read the readme"}]},
    )
    check("tool loop: MOCK:TOOLFILE drives chat read_file tool-call stream",
          status == 200 and b"read_file" in body and b"README.md" in body,
          f"status={status}")
    status, _, body, _ = http(
        "POST", base, "/responses", token="mock-chatgpt",
        payload={"model": "gpt-5.6-codex", "input": [{"type": "message", "role": "user",
                 "content": [{"type": "input_text", "text": "MOCK:TOOLFILE readme"}]}]},
    )
    check("tool loop: MOCK:TOOLFILE drives responses read_file function_call stream",
          status == 200 and b"read_file" in body and b"README.md" in body,
          f"status={status}")


def phase_media_tasks(base):
    status, _, body, _ = http("GET", base, "/compatible-mode/v1/models", token="mock-qwen-token-plan")
    check("Qwen compatible catalogue advertises image generation", status == 200 and "wan2.7-image" in [m["id"] for m in json.loads(body)["data"]])
    image_request = {"model": "wan2.7-image", "stream": False, "messages": [{"role": "user", "content": [{"type": "text", "text": "a paper boat"}]}]}
    status, headers, body, _ = http("POST", base, "/compatible-mode/v1/chat/completions", token="mock-qwen-token-plan", payload=image_request)
    check("Qwen image request returns JSON output.choices", status == 200 and headers.get("Content-Type") == "application/json" and json.loads(body)["output"]["finished"])
    status, _, image, _ = http("GET", base, "/__media/image.png")
    valid_png = image.startswith(b"\x89PNG\r\n\x1a\n")
    offset = 8
    chunks = {}
    while valid_png and offset < len(image):
        length = int.from_bytes(image[offset:offset + 4], "big")
        end = offset + 12 + length
        chunk = image[offset + 4:offset + 8 + length]
        valid_png = end <= len(image) and zlib.crc32(chunk) == int.from_bytes(image[end - 4:end], "big")
        chunks[chunk[:4]] = chunk[4:]
        offset = end
    try:
        valid_png = (
            valid_png
            and offset == len(image)
            and struct.unpack("!II", chunks[b"IHDR"][:8]) == (1, 1)
            and len(zlib.decompress(chunks[b"IDAT"])) == 5
            and b"IEND" in chunks
        )
    except (KeyError, struct.error, zlib.error):
        valid_png = False
    check("image output downloads as valid PNG pixels", status == 200 and valid_png)
    submit = {"model": "happyhorse-1.1-t2v", "input": {"prompt": "a paper boat"}, "parameters": {"resolution": "720P", "ratio": "16:9", "duration": 5}}
    endpoint = "/api/v1/services/aigc/video-generation/video-synthesis"
    status, _, _, _ = http("POST", base, endpoint, token="mock-qwen-token-plan", payload=submit)
    check("native video requires async header", status == 400)
    for failed in (False, True):
        submit["input"]["prompt"] = "MOCK:VIDEO_FAILED" if failed else "a paper boat"
        status, _, body, _ = http("POST", base, endpoint, token="mock-qwen-token-plan", payload=submit, extra_headers={"X-DashScope-Async": "enable"})
        task = json.loads(body)["output"]
        check("video submits PENDING", status == 200 and task["task_status"] == "PENDING")
        for expected in ("RUNNING", "FAILED" if failed else "SUCCEEDED"):
            status, _, body, _ = http("GET", base, "/api/v1/tasks/" + task["task_id"], token="mock-qwen-token-plan")
            check("video progresses " + expected, status == 200 and json.loads(body)["output"]["task_status"] == expected)
        status, _, _, _ = http("GET", base, "/api/v1/tasks/" + task["task_id"], token="qwen-token-plan")
        check("different video account cannot query task", status == 404)


# --- 阶段 3 附加：同一录制树服务上的错误、截断、定速与触发回归 ------------

def chat_body(model, text):
    return {"model": model, "stream": True, "messages": [{"role": "user", "content": text}]}


def responses_body(model, text):
    return {"model": model, "stream": True, "store": False, "input": text}


def messages_body(text):
    return {
        "model": "claude-sonnet-4.6",
        "stream": True,
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": text}]}
        ],
    }


def sse_events(body):
    return [json.loads(event) for event in sse_data_events(body) if event != "[DONE]"]


def control(base_url, payload=None, method="POST"):
    data = None if payload is None else json.dumps(payload)
    req = urlrequest.Request(
        base_url + "/__control",
        data=data.encode("utf-8") if data is not None else None,
        headers={"Content-Type": "application/json"} if data is not None else {},
        method=method,
    )
    try:
        with urlrequest.urlopen(req, timeout=15) as resp:
            status, raw = resp.status, resp.read()
    except HTTPError as error:
        status, raw = error.code, error.read()
    return status, json.loads(raw)


def phase_manifest(scenarios):
    names = [item["name"] for item in scenarios]
    check(
        "manifest: scenario names unique",
        len(names) == len(set(names)),
        f"duplicates={sorted({n for n in names if names.count(n) > 1})}",
    )
    http_scenarios = [item for item in scenarios if item["kind"] == "http_error"]
    expected_codes = {400, 401, 402, 403, 404, 408, 413, 429, 451, 500, 502, 503, 504}
    check(
        "manifest: http_error covers the full §2.5 status table",
        {item["status"] for item in http_scenarios} == expected_codes and len(http_scenarios) == 14,
        f"statuses={sorted(item['status'] for item in http_scenarios)}",
    )
    retry_forms = [item.get("retry_after") for item in http_scenarios if item["status"] == 429]
    check(
        "manifest: 429 has seconds and IMF-fixdate Retry-After variants",
        len(retry_forms) == 2
        and any(str(v).isdigit() for v in retry_forms)
        and any(FIXDATE_RE.match(str(v)) for v in retry_forms),
        f"retry_forms={retry_forms}",
    )
    missing = [
        item["fixture"]
        for item in scenarios
        if item["kind"] == "sse"
        and (not item.get("transports") or not (FIXTURES / "scenarios" / item["fixture"]).is_file())
    ]
    check("manifest: sse scenarios carry transports and fixture files", not missing, f"missing={missing}")


def phase_http_table(url, scenarios):
    http_scenarios = [item for item in scenarios if item["kind"] == "http_error"]
    for item in http_scenarios:
        name = item["name"]
        status, headers, body, _ = http(
            "POST", url, "/chat/completions", token="mock-glm-coding",
            payload=chat_body("glm-5.2", f"please run MOCK:{name.upper()} now"),
        )
        retry = headers.get("Retry-After")
        expected_retry = str(item["retry_after"]) if item.get("retry_after") else None
        ok = status == item["status"] and retry == expected_retry
        check(
            f"http table: keyword MOCK:{name.upper()} -> {item['status']}"
            + (f" Retry-After={expected_retry}" if expected_retry else ""),
            ok,
            f"status={status} retry={retry!r} expected={item['status']}/{expected_retry!r} body={body[:80]!r}",
        )


def phase_orthogonality(url):
    status, headers, _, _ = http(
        "POST", url, "/responses", token="mock-chatgpt",
        payload=responses_body("gpt-5.6-terra", "MOCK:HTTP_402 via input string"),
    )
    check("orthogonality: chatgpt /responses keyword -> 402", status == 402, f"status={status}")

    status, _, _, _ = http(
        "POST", url, "/v1/messages", api_key="mock-anthropic",
        payload=messages_body("MOCK:HTTP_451 inside content blocks"),
    )
    check("orthogonality: anthropic /v1/messages keyword -> 451", status == 451, f"status={status}")

    status, headers, _, _ = http(
        "POST", url, "/chat/completions", token="mock-xai",
        payload=chat_body("grok-3", "MOCK:RATE_LIMIT on chat channel"),
    )
    check(
        "orthogonality: xai grok-3 chat keyword -> 429 + Retry-After 30",
        status == 429 and headers.get("Retry-After") == "30",
        f"status={status} retry={headers.get('Retry-After')!r}",
    )

    status, headers, _, _ = http(
        "POST", url, "/responses", token="mock-opencode-go",
        payload={
            "model": "grok-4.6",
            "stream": True,
            "input": [{"type": "message", "content": [{"type": "input_text", "text": "MOCK:RATE_LIMIT_DATE"}]}],
        },
    )
    check(
        "orthogonality: opencode-go /responses input array -> 429 + fixdate",
        status == 429 and FIXDATE_RE.match(headers.get("Retry-After") or "") is not None,
        f"status={status} retry={headers.get('Retry-After')!r}",
    )

    control(url, {"scenario": "rate_limit"})
    status, _, _, _ = http("GET", url, "/models", token="mock-glm-coding")
    check("orthogonality: global scenario drives GET /models (no body)", status == 429, f"status={status}")
    status, _, _, _ = http("GET", url, "/usage", token="mock-opencode-go")
    check("orthogonality: global scenario drives GET /usage", status == 429, f"status={status}")
    control(url, {"scenario": None})
    status, _, _, _ = http("GET", url, "/models", token="mock-glm-coding")
    check("orthogonality: reset restores GET /models", status == 200, f"status={status}")


def phase_responses_needles(url):
    cases = [
        (
            "responses needle: chatgpt error event carries usage+limit",
            "mock-chatgpt",
            "gpt-5.6-terra",
            "MOCK:RESPONSES_ERROR_CHATGPT_QUOTA",
            "error",
            "usage",
        ),
        (
            "responses needle: xai response.failed carries insufficient_quota",
            "mock-xai",
            "grok-4",
            "MOCK:RESPONSES_ERROR_XAI_QUOTA",
            "response.failed",
            "insufficient_quota",
        ),
        (
            "responses needle: opencode-go response.failed message passes through",
            "mock-opencode-go",
            "grok-4.6",
            "MOCK:RESPONSES_ERROR_GO_QUOTA",
            "response.failed",
            "opencode-go responses quota",
        ),
    ]
    for name, token, model, keyword, event_type, needle in cases:
        status, _, body, _ = http(
            "POST", url, "/responses", token=token,
            payload=responses_body(model, f"{keyword} please"),
        )
        events = sse_events(body)
        hit = next(
            (
                event
                for event in events
                if event.get("type") == event_type
                and needle in json.dumps(event, ensure_ascii=False)
            ),
            None,
        )
        ok = status == 200 and hit is not None
        if ok and event_type == "response.failed":
            ok = needle in hit.get("response", {}).get("error", {}).get("message", "")
        elif ok:
            ok = needle in hit.get("message", "")
        check(name, ok, f"status={status} events={events}")

    for channel in ("mock-glm-coding", "mock-qwen-token-plan"):
        status, _, body, _ = http(
            "POST", url, "/responses", token=channel,
            payload=responses_body("glm-5.2", "MOCK:RESPONSES_ERROR_CHATGPT_QUOTA"),
        )
        check(
            f"responses needle: {channel[5:]} has no /responses wire path (404, no copy events)",
            status == 404 and b"insufficient" not in body and b"usage limit" not in body,
            f"status={status} body={body[:80]!r}",
        )


def phase_truncation(url):
    status, _, body, _ = http(
        "POST", url, "/chat/completions", token="mock-glm-coding",
        payload=chat_body("glm-5.2", "MOCK:TRUNCATED_CHAT"),
    )
    text = body.decode("utf-8")
    check(
        "truncation: chat stream has neither [DONE] nor finish_reason",
        status == 200 and "[DONE]" not in text and "finish_reason" not in text,
        f"status={status} tail={text[-90:]!r}",
    )

    status, _, body, _ = http(
        "POST", url, "/v1/messages", api_key="mock-anthropic",
        payload=messages_body("MOCK:TRUNCATED_MESSAGES"),
    )
    types = [event.get("type") for event in sse_events(body)]
    check(
        "truncation: anthropic stream lacks message_stop",
        status == 200 and "message_stop" not in types and "content_block_delta" in types,
        f"status={status} types={types}",
    )

    status, _, body, _ = http(
        "POST", url, "/responses", token="mock-chatgpt",
        payload=responses_body("gpt-5.6-terra", "MOCK:TRUNCATED_RESPONSES"),
    )
    types = [event.get("type") for event in sse_events(body)]
    terminals = {"response.completed", "response.incomplete", "response.failed", "error"}
    check(
        "truncation: responses stream has no terminal event",
        status == 200 and not (set(types) & terminals) and "response.output_text.delta" in types,
        f"status={status} types={types}",
    )


def phase_shapes(url):
    status, _, body, _ = http(
        "POST", url, "/chat/completions", token="mock-deepseek",
        payload=chat_body("deepseek-v4-pro", "MOCK:CHAT_USAGE_CHUNK"),
    )
    events = sse_data_events(body)
    parsed = [json.loads(event) for event in events[:-1]] if events[-1] == "[DONE]" else []
    usage_chunk = next((chunk for chunk in parsed if chunk.get("choices") == [] and "usage" in chunk), None)
    has_finish = any(
        chunk.get("choices") and chunk["choices"][0].get("finish_reason") == "stop" for chunk in parsed
    )
    check(
        "shape: usage arrives as an independent chunk before [DONE]",
        status == 200 and events[-1] == "[DONE]" and usage_chunk is not None and has_finish,
        f"status={status} events={events}",
    )

    status, _, body, _ = http(
        "POST", url, "/chat/completions", token="mock-qwen-token-plan",
        payload=chat_body("qwen3.8-max", "MOCK:CHAT_TOOL_ARGS_SPLIT"),
    )
    events = sse_data_events(body)
    parsed = [json.loads(event) for event in events[:-1]] if events[-1] == "[DONE]" else []
    fragments = [
        call
        for chunk in parsed
        for call in (chunk.get("choices") or [{}])[0].get("delta", {}).get("tool_calls", [])
        if (call.get("function") or {}).get("arguments")
    ]
    firsts = [
        call
        for chunk in parsed
        for call in (chunk.get("choices") or [{}])[0].get("delta", {}).get("tool_calls", [])
        if call.get("id") and call.get("function", {}).get("name")
    ]
    joined = "".join(call["function"]["arguments"] for call in fragments)
    has_finish = any(
        chunk.get("choices") and chunk["choices"][0].get("finish_reason") == "tool_calls" for chunk in parsed
    )
    try:
        reassembled = json.loads(joined)
    except ValueError:
        reassembled = None
    check(
        "shape: tool arguments split across chunks reassemble to valid JSON",
        status == 200
        and events[-1] == "[DONE]"
        and len(firsts) == 1
        and len(fragments) >= 2
        and reassembled == {"city": "Paris", "unit": "celsius"}
        and has_finish,
        f"status={status} joined={joined!r} reassembled={reassembled}",
    )


def phase_control_and_priority(url, scenarios):
    code, payload = control(url, method="GET")
    expected = sorted(item["name"] for item in scenarios)
    check(
        "control: GET reports null scenario and full catalog",
        code == 200 and payload.get("scenario") is None and payload.get("available") == expected,
        f"code={code} payload={payload}",
    )

    code, payload = control(url, {"scenario": "rate_limit"})
    check(
        "control: POST sets global scenario (case-insensitive)",
        code == 200 and payload.get("scenario") == "rate_limit",
        f"code={code} payload={payload}",
    )

    status, headers, _, _ = http(
        "POST", url, "/chat/completions", token="mock-glm-coding",
        payload=chat_body("glm-5.2", "no keyword here"),
    )
    check(
        "control: global scenario applies to plain request without keyword",
        status == 429 and headers.get("Retry-After") == "30",
        f"status={status} retry={headers.get('Retry-After')!r}",
    )

    status, _, _, _ = http(
        "POST", url, "/chat/completions", token="mock-glm-coding",
        payload=chat_body("glm-5.2", "MOCK:HTTP_402 beats the global scenario"),
    )
    check(
        "control: prompt keyword takes priority over global scenario",
        status == 402,
        f"status={status} (expected 402 while global=rate_limit/429)",
    )

    status, _, _, _ = http(
        "POST", url, "/chat/completions", token="mock-glm-coding",
        payload=chat_body("glm-5.2", "MOCK:UNKNOWN_NAME falls back"),
    )
    check(
        "control: unknown keyword falls back to global scenario",
        status == 429,
        f"status={status} (expected 429 from global)",
    )

    code, payload = control(url, {"scenario": "no_such_scenario"})
    check(
        "control: unknown scenario name rejected 404",
        code == 404 and "available" in payload,
        f"code={code} payload={payload}",
    )
    code, _ = control(url, {"scenario": 123})
    check("control: non-string non-null scenario rejected 400", code == 400, f"code={code}")

    code, payload = control(url, {"scenario": None})
    check(
        "control: scenario null resets to normal behavior",
        code == 200 and payload.get("scenario") is None,
        f"code={code} payload={payload}",
    )
    status, _, body, _ = http(
        "POST", url, "/chat/completions", token="mock-glm-coding",
        payload=chat_body("glm-5.2", "back to normal"),
    )
    events = sse_data_events(body)
    check(
        "control: after reset plain request streams normally with [DONE]",
        status == 200 and events and events[-1] == "[DONE]",
        f"status={status} events={events}",
    )


def phase_slow_stream(url):
    # responses_text.sse ≈317B，--chunk-bytes 64 → 5 块 4 次间隔；150ms → ≥0.6s。
    status, _, body, elapsed = http(
        "POST", url, "/responses", token="mock-chatgpt",
        payload=responses_body("gpt-5.6-terra", "MOCK:SLOW_STREAM paced"),
        query={"interval_ms": "150"},
    )
    types = [event.get("type") for event in sse_events(body)]
    check(
        "slow stream: scenario pacing stretches chunk interval, stream still completes",
        status == 200 and elapsed >= 0.5 and "response.completed" in types,
        f"status={status} elapsed={elapsed:.3f}s types={types}",
    )


def phase_scenarios(url):
    scenarios = json.loads(MANIFEST.read_text(encoding="utf-8"))["scenarios"]
    phase_manifest(scenarios)
    phase_http_table(url, scenarios)
    phase_orthogonality(url)
    phase_responses_needles(url)
    phase_truncation(url)
    phase_shapes(url)
    phase_control_and_priority(url, scenarios)
    phase_slow_stream(url)


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
