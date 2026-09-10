#!/usr/bin/env python3
"""MOCK-4 自测：场景库双触发、HTTP 状态码全表、needle、截断与定速。

对照 docs/mock-simulation-plan.md §2.4/§2.5/§4 MOCK-4 断言：
  1) manifest 形态（名字唯一、sse 场景 fixture 存在、transports 非空）；
  2) HTTP 状态码全表（400-504 + Retry-After 秒/IMF-fixdate 两形态）逐场景核对；
  3) persona 正交：chatgpt/xai/anthropic/opencode-go 各端点 + GET 端点全局场景；
  4) Responses 流内 error/response.failed needle（chatgpt usage+limit、
     xai insufficient_quota、opencode-go 透传）；glm-coding/qwen-token-plan
     在 /responses 无 wire 路径（404），不构造文案事件；
  5) 截断三态：Chat 无 [DONE]/finish_reason、Messages 缺 message_stop、
     Responses 无终态事件；
  6) usage 独立 chunk 与跨 chunk 工具参数分片（MOCK-2/3 fixture 未覆盖的补场景版）；
  7) 触发机制：关键字优先于全局、/__control 切换/复位/未知名/类型错、
     慢流按 interval_ms 定速且流完整。
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import server_smoke as base  # 复用 start/stop/http/check 与 SSE 解析

REPO = base.REPO
FIXTURES = REPO / "fixtures" / "mock"
MANIFEST = FIXTURES / "scenarios" / "manifest.json"
FIXDATE_RE = re.compile(r"^[A-Z][a-z]{2}, \d{2} [A-Z][a-z]{2} \d{4} \d{2}:\d{2}:\d{2} GMT$")


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
    return [json.loads(event) for event in base.sse_data_events(body) if event != "[DONE]"]


def control(base_url, payload=None, method="POST"):
    data = None if payload is None else json.dumps(payload)
    req = base.urlrequest.Request(
        base_url + "/__control",
        data=data.encode("utf-8") if data is not None else None,
        headers={"Content-Type": "application/json"} if data is not None else {},
        method=method,
    )
    try:
        with base.urlrequest.urlopen(req, timeout=15) as resp:
            status, raw = resp.status, resp.read()
    except base.HTTPError as error:
        status, raw = error.code, error.read()
    return status, json.loads(raw)


def phase_manifest(scenarios):
    names = [item["name"] for item in scenarios]
    base.check(
        "manifest: scenario names unique",
        len(names) == len(set(names)),
        f"duplicates={sorted({n for n in names if names.count(n) > 1})}",
    )
    http = [item for item in scenarios if item["kind"] == "http_error"]
    expected_codes = {400, 401, 402, 403, 404, 408, 413, 429, 451, 500, 502, 503, 504}
    base.check(
        "manifest: http_error covers the full §2.5 status table",
        {item["status"] for item in http} == expected_codes and len(http) == 14,
        f"statuses={sorted(item['status'] for item in http)}",
    )
    retry_forms = [item.get("retry_after") for item in http if item["status"] == 429]
    base.check(
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
    base.check("manifest: sse scenarios carry transports and fixture files", not missing, f"missing={missing}")
    no_chat_needles = [
        item["name"]
        for item in scenarios
        if item["kind"] == "sse" and "chat" in item.get("transports", [])
        and ("glm-coding" in item.get("description", "") or "qwen" in item.get("description", ""))
    ]
    base.check(
        "manifest: no vendor-copy events for glm-coding / qwen-token-plan (chat-only channels)",
        not no_chat_needles,
        f"violations={no_chat_needles}",
    )


def phase_http_table(url, scenarios):
    http = [item for item in scenarios if item["kind"] == "http_error"]
    for item in http:
        name = item["name"]
        status, headers, body, _ = base.http(
            "POST", url, "/chat/completions", token="mock-glm-coding",
            payload=chat_body("glm-5.2", f"please run MOCK:{name.upper()} now"),
        )
        retry = headers.get("Retry-After")
        expected_retry = str(item["retry_after"]) if item.get("retry_after") else None
        ok = status == item["status"] and retry == expected_retry
        base.check(
            f"http table: keyword MOCK:{name.upper()} -> {item['status']}"
            + (f" Retry-After={expected_retry}" if expected_retry else ""),
            ok,
            f"status={status} retry={retry!r} expected={item['status']}/{expected_retry!r} body={body[:80]!r}",
        )


def phase_orthogonality(url):
    status, headers, _, _ = base.http(
        "POST", url, "/responses", token="mock-chatgpt",
        payload=responses_body("gpt-5.6-terra", "MOCK:HTTP_402 via input string"),
    )
    base.check("orthogonality: chatgpt /responses keyword -> 402", status == 402, f"status={status}")

    status, _, _, _ = base.http(
        "POST", url, "/v1/messages", api_key="mock-anthropic",
        payload=messages_body("MOCK:HTTP_451 inside content blocks"),
    )
    base.check("orthogonality: anthropic /v1/messages keyword -> 451", status == 451, f"status={status}")

    status, headers, _, _ = base.http(
        "POST", url, "/chat/completions", token="mock-xai",
        payload=chat_body("grok-3", "MOCK:RATE_LIMIT on chat channel"),
    )
    base.check(
        "orthogonality: xai grok-3 chat keyword -> 429 + Retry-After 30",
        status == 429 and headers.get("Retry-After") == "30",
        f"status={status} retry={headers.get('Retry-After')!r}",
    )

    status, headers, _, _ = base.http(
        "POST", url, "/responses", token="mock-opencode-go",
        payload={
            "model": "grok-4.6",
            "stream": True,
            "input": [{"type": "message", "content": [{"type": "input_text", "text": "MOCK:RATE_LIMIT_DATE"}]}],
        },
    )
    base.check(
        "orthogonality: opencode-go /responses input array -> 429 + fixdate",
        status == 429 and FIXDATE_RE.match(headers.get("Retry-After") or "") is not None,
        f"status={status} retry={headers.get('Retry-After')!r}",
    )

    control(url, {"scenario": "rate_limit"})
    status, _, _, _ = base.http("GET", url, "/models", token="mock-glm-coding")
    base.check("orthogonality: global scenario drives GET /models (no body)", status == 429, f"status={status}")
    status, _, _, _ = base.http("GET", url, "/usage", token="mock-opencode-go")
    base.check("orthogonality: global scenario drives GET /usage", status == 429, f"status={status}")
    control(url, {"scenario": None})
    status, _, _, _ = base.http("GET", url, "/models", token="mock-glm-coding")
    base.check("orthogonality: reset restores GET /models", status == 200, f"status={status}")


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
        status, _, body, _ = base.http(
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
        base.check(name, ok, f"status={status} events={events}")

    for channel in ("mock-glm-coding", "mock-qwen-token-plan"):
        status, _, body, _ = base.http(
            "POST", url, "/responses", token=channel,
            payload=responses_body("glm-5.2", "MOCK:RESPONSES_ERROR_CHATGPT_QUOTA"),
        )
        base.check(
            f"responses needle: {channel[5:]} has no /responses wire path (404, no copy events)",
            status == 404 and b"insufficient" not in body and b"usage limit" not in body,
            f"status={status} body={body[:80]!r}",
        )


def phase_truncation(url):
    status, _, body, _ = base.http(
        "POST", url, "/chat/completions", token="mock-glm-coding",
        payload=chat_body("glm-5.2", "MOCK:TRUNCATED_CHAT"),
    )
    text = body.decode("utf-8")
    base.check(
        "truncation: chat stream has neither [DONE] nor finish_reason",
        status == 200 and "[DONE]" not in text and "finish_reason" not in text,
        f"status={status} tail={text[-90:]!r}",
    )

    status, _, body, _ = base.http(
        "POST", url, "/v1/messages", api_key="mock-anthropic",
        payload=messages_body("MOCK:TRUNCATED_MESSAGES"),
    )
    types = [event.get("type") for event in sse_events(body)]
    base.check(
        "truncation: anthropic stream lacks message_stop",
        status == 200 and "message_stop" not in types and "content_block_delta" in types,
        f"status={status} types={types}",
    )

    status, _, body, _ = base.http(
        "POST", url, "/responses", token="mock-chatgpt",
        payload=responses_body("gpt-5.6-terra", "MOCK:TRUNCATED_RESPONSES"),
    )
    types = [event.get("type") for event in sse_events(body)]
    terminals = {"response.completed", "response.incomplete", "response.failed", "error"}
    base.check(
        "truncation: responses stream has no terminal event",
        status == 200 and not (set(types) & terminals) and "response.output_text.delta" in types,
        f"status={status} types={types}",
    )


def phase_shapes(url):
    status, _, body, _ = base.http(
        "POST", url, "/chat/completions", token="mock-deepseek",
        payload=chat_body("deepseek-v4-pro", "MOCK:CHAT_USAGE_CHUNK"),
    )
    events = base.sse_data_events(body)
    parsed = [json.loads(event) for event in events[:-1]] if events[-1] == "[DONE]" else []
    usage_chunk = next((chunk for chunk in parsed if chunk.get("choices") == [] and "usage" in chunk), None)
    has_finish = any(
        chunk.get("choices") and chunk["choices"][0].get("finish_reason") == "stop" for chunk in parsed
    )
    base.check(
        "shape: usage arrives as an independent chunk before [DONE]",
        status == 200 and events[-1] == "[DONE]" and usage_chunk is not None and has_finish,
        f"status={status} events={events}",
    )

    status, _, body, _ = base.http(
        "POST", url, "/chat/completions", token="mock-qwen-token-plan",
        payload=chat_body("qwen3.8-max", "MOCK:CHAT_TOOL_ARGS_SPLIT"),
    )
    events = base.sse_data_events(body)
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
    base.check(
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
    base.check(
        "control: GET reports null scenario and full catalog",
        code == 200 and payload.get("scenario") is None and payload.get("available") == expected,
        f"code={code} payload={payload}",
    )

    code, payload = control(url, {"scenario": "rate_limit"})
    base.check(
        "control: POST sets global scenario (case-insensitive)",
        code == 200 and payload.get("scenario") == "rate_limit",
        f"code={code} payload={payload}",
    )

    status, headers, _, _ = base.http(
        "POST", url, "/chat/completions", token="mock-glm-coding",
        payload=chat_body("glm-5.2", "no keyword here"),
    )
    base.check(
        "control: global scenario applies to plain request without keyword",
        status == 429 and headers.get("Retry-After") == "30",
        f"status={status} retry={headers.get('Retry-After')!r}",
    )

    status, _, _, _ = base.http(
        "POST", url, "/chat/completions", token="mock-glm-coding",
        payload=chat_body("glm-5.2", "MOCK:HTTP_402 beats the global scenario"),
    )
    base.check(
        "control: prompt keyword takes priority over global scenario",
        status == 402,
        f"status={status} (expected 402 while global=rate_limit/429)",
    )

    status, _, _, _ = base.http(
        "POST", url, "/chat/completions", token="mock-glm-coding",
        payload=chat_body("glm-5.2", "MOCK:UNKNOWN_NAME falls back"),
    )
    base.check(
        "control: unknown keyword falls back to global scenario",
        status == 429,
        f"status={status} (expected 429 from global)",
    )

    code, payload = control(url, {"scenario": "no_such_scenario"})
    base.check(
        "control: unknown scenario name rejected 404",
        code == 404 and "available" in payload,
        f"code={code} payload={payload}",
    )
    code, _ = control(url, {"scenario": 123})
    base.check("control: non-string non-null scenario rejected 400", code == 400, f"code={code}")

    code, payload = control(url, {"scenario": None})
    base.check(
        "control: scenario null resets to normal behavior",
        code == 200 and payload.get("scenario") is None,
        f"code={code} payload={payload}",
    )
    status, _, body, _ = base.http(
        "POST", url, "/chat/completions", token="mock-glm-coding",
        payload=chat_body("glm-5.2", "back to normal"),
    )
    events = base.sse_data_events(body)
    base.check(
        "control: after reset plain request streams normally with [DONE]",
        status == 200 and events and events[-1] == "[DONE]",
        f"status={status} events={events}",
    )


def phase_slow_stream(url):
    # responses_text.sse ≈317B，--chunk-bytes 64 → 5 块 4 次间隔；150ms → ≥0.6s。
    status, _, body, elapsed = base.http(
        "POST", url, "/responses", token="mock-chatgpt",
        payload=responses_body("gpt-5.6-terra", "MOCK:SLOW_STREAM paced"),
        query={"interval_ms": "150"},
    )
    types = [event.get("type") for event in sse_events(body)]
    base.check(
        "slow stream: scenario pacing stretches chunk interval, stream still completes",
        status == 200 and elapsed >= 0.5 and "response.completed" in types,
        f"status={status} elapsed={elapsed:.3f}s types={types}",
    )

    status, _, body, elapsed = base.http(
        "POST", url, "/responses", token="mock-chatgpt",
        payload=responses_body("gpt-5.6-terra", "no scenario, no pacing"),
    )
    base.check(
        "slow stream: same endpoint without scenario stays fast",
        status == 200 and elapsed < 0.5,
        f"status={status} elapsed={elapsed:.3f}s",
    )


def main() -> int:
    print(f"repo: {REPO}")
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    scenarios = manifest["scenarios"]
    phase_manifest(scenarios)
    proc, url = base.start_server(FIXTURES, extra=("--chunk-bytes", "64"))
    try:
        phase_http_table(url, scenarios)
        phase_orthogonality(url)
        phase_responses_needles(url)
        phase_truncation(url)
        phase_shapes(url)
        phase_control_and_priority(url, scenarios)
        phase_slow_stream(url)
    finally:
        try:
            control(url, {"scenario": None})
        except Exception:
            pass
        base.stop_server(proc)
    passed = sum(base.RESULTS)
    total = len(base.RESULTS)
    verdict = "PASS" if passed == total else "FAIL"
    print(f"SCENARIOS {verdict} {passed}/{total}")
    return 0 if passed == total else 1


if __name__ == "__main__":
    sys.exit(main())
