#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""MOCK-2: record real provider responses into sanitized mock fixtures.

stdlib only. Facts mirror crates/providers source:
- endpoints / transport table: crates/providers/src/channels/registry.rs, api_key.rs
- chatgpt headers: crates/providers/src/channels/chatgpt.rs
- xai catalog: crates/providers/src/channels/xai.rs
- SSE shapes: crates/providers/tests/{contract,chatgpt,xai,anthropic}.rs and
  crates/providers/src/responses.rs unit tests.

Commands:
  record      real capture for channels with usable credentials; channels without
              (or per-fixture failures) fall back to contract-shaped synthesis
  synthesize  write synthetic fixtures only
  verify      validate shape + sanitization of everything under fixtures/mock

Credentials are read from $PAWORK_HOME/auth.json (default ~/.pawork/auth.json)
or PAWORK_API_KEY_* env vars, used in-memory only, and never written anywhere.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import sys
import urllib.error
import urllib.request
import uuid
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUT = REPO_ROOT / "fixtures" / "mock"
AUTH_PATH = Path(os.environ.get("PAWORK_HOME", str(Path.home() / ".pawork"))) / "auth.json"

CHAT = "chat_completions"
RESPONSES = "responses"
MESSAGES = "messages"

# Registry facts (registry.rs + api_key.rs documented_model_transports).
OPencodeGO_CHAT_MODELS = {
    "glm-5.3-flash", "glm-5.3", "glm-5.2", "glm-5.1", "kimi-k3", "kimi-k2.7-code",
    "kimi-k2.6", "longcat-2.0", "deepseek-v4-pro", "deepseek-v4-flash",
    "deepseek-v4-flash-vision-exp", "mimo-v2.5", "mimo-v2.5-pro", "hy4-preview",
    "hy3", "omen-alpha",
}
OPencodeGO_RESPONSES_MODELS = {
    "grok-4.6", "gpt-5.6-luna", "muse-spark-1.3-contributor", "muse-spark-1.2-contributor",
}
QWEN_CHAT_MODELS = {
    "qwen3.8-max", "qwen3.8-max-preview", "qwen3.8-flash", "qwen3.7-max",
    "qwen3.7-plus", "qwen3.6-flash", "glm-5.2", "deepseek-v4-pro",
    "deepseek-v4-pro-0813", "deepseek-v4-flash-0731",
}

CHANNELS = {
    "chatgpt": dict(
        base="https://chatgpt.com/backend-api/codex", kind="oauth",
        models_path="/models?client_version=0.153.0", models_shape="chatgpt",
        stream_transport=RESPONSES, cred_service="pawork.chatgpt.oauth",
        preferred=["gpt-5.6-codex"],
    ),
    "xai": dict(
        base="https://api.x.ai/v1", kind="oauth",
        models_path="/language-models", models_shape="xai",
        stream_transport=RESPONSES, cred_service="pawork.xai.oauth",
        # builtin(xai.rs): 仅 grok-4 / grok-4-fast 走 Responses；未登记 id
        # （如 grok-4.6）按 unknown_text_model 兜底走 Chat Completions。
        preferred=["grok-4", "grok-4-fast"],
        transport_note="适用 grok-4 系（xai builtin：grok-4/grok-4-fast → Responses）",
        secondary=dict(
            transport=CHAT, preferred=["grok-4.6"],
            note="grok-4.6 未登记 xai builtin，生产按 Chat Completions 兜底路由",
        ),
    ),
    "glm-coding": dict(
        base="https://api.z.ai/api/coding/paas/v4", kind="api_key",
        models_path="/models", models_shape="openai", stream_transport=CHAT,
        cred_service="pawork.glm-coding", preferred=["glm-5.2", "glm-5.3-flash", "glm-5.3"],
    ),
    "opencode-go": dict(
        base="https://opencode.ai/zen/go/v1", kind="api_key",
        models_path="/models", models_shape="openai", stream_transport=CHAT,
        usage_path="/usage", allowed=OPencodeGO_CHAT_MODELS,
        cred_service="pawork.opencode-go", preferred=["glm-5.3-flash", "deepseek-v4-flash", "kimi-k3"],
        # Go 是混合 transport：Responses 组模型（api_key.rs 官方逐模型端点表）
        # 走 POST /responses，与 Chat 组并存。
        secondary=dict(
            transport=RESPONSES, allowed=OPencodeGO_RESPONSES_MODELS,
            preferred=["grok-4.6"],
            note="Go Responses 组（api_key.rs 端点表：grok-4.6/gpt-5.6-luna/muse-spark-*）",
        ),
    ),
    "qwen-token-plan": dict(
        base="https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1", kind="api_key",
        models_path="/models", models_shape="openai", stream_transport=CHAT,
        allowed=QWEN_CHAT_MODELS,
        cred_service="pawork.qwen-token-plan", preferred=["qwen3.8-flash", "qwen3.6-flash"],
        tool_choice_forced=False,  # thinking mode rejects object tool_choice; auto matches Pawork wire
    ),
    "deepseek": dict(
        base="https://api.deepseek.com", kind="api_key",
        models_path="/models", models_shape="openai", stream_transport=CHAT,
        cred_service="pawork.deepseek", preferred=["deepseek-chat"],
        tool_choice_forced=False,  # catalog resolves to thinking models that reject object tool_choice
    ),
    "kimi-platform": dict(
        base="https://api.moonshot.ai/v1", kind="api_key",
        models_path="/models", models_shape="openai", stream_transport=CHAT,
        cred_service="pawork.kimi-platform", preferred=["kimi-k3"],
    ),
    "kimi-code": dict(
        base="https://api.kimi.com/coding/v1", kind="oauth",
        models_path="/models", models_shape="openai", stream_transport=CHAT,
        cred_service="pawork.kimi-code.oauth", preferred=["kimi-k3"],
    ),
    # Transport baseline only (not a registry channel): static catalog, no HTTP models.
    "anthropic": dict(
        base="https://api.anthropic.com", kind="api_key",
        models_path=None, models_shape=None, stream_transport=MESSAGES,
        messages_path="/v1/messages", cred_service="pawork.anthropic", preferred=["claude-sonnet-4-5"],
    ),
}

SYNTH_MODELS = {
    "chatgpt": ["gpt-5.6-codex", "gpt-5.6"],
    "xai": ["grok-4.6", "grok-4"],
    "glm-coding": ["glm-5.2", "glm-5.3-flash"],
    "opencode-go": ["glm-5.3-flash", "grok-4.6"],
    "qwen-token-plan": ["qwen3.8-flash", "qwen3.8-max"],
    "deepseek": ["deepseek-chat", "deepseek-reasoner"],
    "kimi-platform": ["kimi-k3", "kimi-k2.7-code"],
    "kimi-code": ["kimi-k3", "kimi-k2.7-code"],
}

# ---------------------------------------------------------------- sanitizing

SENSITIVE_KEY_RE = re.compile(
    r"(^|_)(email|account_id|account|owner|user_id|organization|organization_id"
    r"|workspace_id|workspace|org_id)($|_)|"
    r"(^|_)(token|secret|api_key|authorization)($|_)",
    re.IGNORECASE,
)
BEARER_RE = re.compile(r"Bearer\s+[A-Za-z0-9._~+/=-]{8,}")
EMAIL_RE = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
JWT_RE = re.compile(r"eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{4,}\.[A-Za-z0-9_-]{4,}")
APIKEY_RE = re.compile(r"\bsk-[A-Za-z0-9_-]{16,}")
AUTHZ_RE = re.compile(r"(?i)\bauthorization\b\s*[:=]")
RESETS_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$")


def scrub_text(text: str, secrets=()) -> str:
    for s in secrets:
        if s:
            text = text.replace(s, "[REDACTED]")
    text = JWT_RE.sub("[REDACTED-JWT]", text)
    text = BEARER_RE.sub("Bearer [REDACTED]", text)
    text = APIKEY_RE.sub("[REDACTED-KEY]", text)
    text = EMAIL_RE.sub("[REDACTED-EMAIL]", text)
    return text


def _scrub_json_obj(obj):
    """Redact identity-bearing string fields. Returns (obj, changed)."""
    changed = False
    if isinstance(obj, dict):
        out = {}
        for key, value in obj.items():
            if SENSITIVE_KEY_RE.search(str(key)):
                out[key] = "[REDACTED]"
                changed = True
            else:
                new_value, sub = _scrub_json_obj(value)
                out[key] = new_value
                changed = changed or sub
        return out, changed
    if isinstance(obj, list):
        out = []
        for value in obj:
            new_value, sub = _scrub_json_obj(value)
            out.append(new_value)
            changed = changed or sub
        return out, changed
    return obj, False


def scrub_sse(data: bytes, secrets=()) -> bytes:
    """Sanitize SSE bytes; untouched lines keep their original bytes."""
    text = data.decode("utf-8", errors="replace")
    pieces = []
    for line in text.splitlines(keepends=True):
        core = line.rstrip("\r\n")
        eol = line[len(core):]
        stripped = core.strip()
        if stripped.startswith("data:"):
            payload = stripped[5:].strip()
            if payload and payload != "[DONE]":
                try:
                    obj = json.loads(payload)
                    obj, changed = _scrub_json_obj(obj)
                    if changed:
                        core = "data: " + json.dumps(obj, ensure_ascii=False, separators=(",", ":"))
                except (json.JSONDecodeError, ValueError):
                    pass
        core = scrub_text(core, secrets)
        pieces.append(core + eol)
    return "".join(pieces).encode("utf-8")


def scrub_json(data: bytes, secrets=()) -> bytes:
    """Sanitize a JSON document; keep original bytes when nothing matched."""
    try:
        obj = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError):
        return scrub_text(data.decode("utf-8", errors="replace"), secrets).encode("utf-8")
    obj, changed = _scrub_json_obj(obj)
    text = data.decode("utf-8")
    if changed:
        text = json.dumps(obj, ensure_ascii=False, indent=2) + "\n"
    return scrub_text(text, secrets).encode("utf-8")

# ---------------------------------------------------------------- credentials


def load_auth_entries() -> dict:
    try:
        data = json.loads(AUTH_PATH.read_text())
    except (OSError, json.JSONDecodeError, ValueError):
        return {}
    entries = data.get("entries")
    return entries if isinstance(entries, dict) else {}


def collect_secrets():
    """Every credential string we know about, for defense-in-depth scrubbing."""
    secrets = []
    for accounts in load_auth_entries().values():
        if not isinstance(accounts, dict):
            continue
        for account, value in accounts.items():
            if account.endswith(".meta"):
                continue
            if isinstance(value, str) and value.strip():
                secrets.append(value)
    for key, value in os.environ.items():
        if key.startswith("PAWORK_API_KEY_") and value.strip():
            secrets.append(value)
    return sorted({s for s in secrets if len(s) >= 8})


def load_channel_credential(name, spec):
    """Return (secret_or_None, note_or_None). Never logs the secret."""
    env_name = "PAWORK_API_KEY_" + name.upper().replace("-", "_")
    env_value = os.environ.get(env_name, "").strip()
    if env_value:
        return env_value, None
    accounts = load_auth_entries().get(spec["cred_service"])
    if not isinstance(accounts, dict):
        return None, "无可用凭证（auth.json 未登记该通道），未经真实录制"
    if spec["kind"] == "api_key":
        value = accounts.get("default")
        if isinstance(value, str) and value.strip():
            return value, None
        return None, "无可用凭证（auth.json 无 default API key），未经真实录制"
    access = accounts.get("default.access")
    if not (isinstance(access, str) and access.strip()):
        return None, "无可用凭证（无 default.access OAuth token），未经真实录制"
    meta = {}
    raw_meta = accounts.get("default.meta")
    if isinstance(raw_meta, str):
        try:
            meta = json.loads(raw_meta)
        except (json.JSONDecodeError, ValueError):
            meta = {}
    expires = meta.get("expires_at_ms") if isinstance(meta, dict) else None
    if isinstance(expires, (int, float)) and expires <= _now_ms():
        return None, (
            "OAuth access token 已过期（expires_at_ms=%d 早于当前时间），按任务约定不执行 "
            "refresh、跳过真实录制，fixture 按契约形状合成（未经真实录制）" % int(expires)
        )
    return access, None


def _now_ms() -> int:
    return int(dt.datetime.now(dt.timezone.utc).timestamp() * 1000)


def utcnow_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")

# ---------------------------------------------------------------- http


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # Never forward a real credential to a redirect destination.
        return None


def try_request(method, url, headers, body=None, timeout=60):
    """One attempt, no retries (keep real-API request counts minimal)."""
    data = None
    send_headers = dict(headers)
    if body is not None:
        data = json.dumps(body, ensure_ascii=False).encode("utf-8")
        send_headers.setdefault("Content-Type", "application/json")
    request = urllib.request.Request(url, data=data, method=method, headers=send_headers)
    try:
        with urllib.request.build_opener(NoRedirect()).open(request, timeout=timeout) as response:
            return True, (response.status, dict(response.headers), response.read())
    except urllib.error.HTTPError as error:
        upstream = ""
        try:
            body = error.read()[:4096]
            obj = json.loads(body.decode("utf-8", errors="replace"))
            detail = obj.get("error") if isinstance(obj, dict) else None
            if isinstance(detail, dict):
                # type/code only: messages may embed workspace or account ids.
                upstream = str(detail.get("type") or detail.get("code") or "")[:60]
        except Exception:
            pass
        return False, "HTTP %s%s" % (error.code, " (upstream %s)" % upstream if upstream else "")
    except Exception as error:  # URLError, socket.timeout, ...
        message = str(error).splitlines()[0][:140] if str(error) else type(error).__name__
        return False, "%s: %s" % (type(error).__name__, message)


def channel_headers(name, spec, secret, streaming=False):
    headers = {"User-Agent": "pawork"}
    if name == "anthropic":
        headers["x-api-key"] = secret
        headers["anthropic-version"] = "2023-06-01"
    else:
        headers["Authorization"] = "Bearer " + secret
    if name == "opencode-go":
        headers["x-opencode-session"] = "capture-" + str(uuid.uuid4())
    if name == "chatgpt":
        accounts = load_auth_entries().get(spec["cred_service"], {})
        meta = json.loads(accounts.get("default.meta", "{}"))
        account_id = meta.get("account_id")
        if not isinstance(account_id, str) or not account_id:
            raise ValueError("ChatGPT account id missing in OAuth metadata")
        headers.update({"ChatGPT-Account-Id": account_id,
                        "originator": "codex_cli_rs", "User-Agent": "codex_cli_rs/0.153.0"})
    if streaming:
        headers["Accept"] = "text/event-stream"
    return headers

# ---------------------------------------------------------------- requests


def chat_body(model, tool=False, forced=True):
    body = {
        "model": model,
        "stream": True,
        "stream_options": {"include_usage": True},
        "messages": [{
            "role": "user",
            "content": "What is the weather in Paris? Use the get_weather tool." if tool else "Say hi",
        }],
        "max_tokens": 1024 if tool else 512,
    }
    if tool:
        body["tools"] = [{
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get the current weather for a city",
                "parameters": {
                    "type": "object",
                    "properties": {"city": {"type": "string", "description": "City name"}},
                    "required": ["city"],
                },
            },
        }]
        body["tool_choice"] = (
            {"type": "function", "function": {"name": "get_weather"}} if forced else "auto"
        )
    return body


def responses_body(name, model, tool=False):
    body = {
        "model": model,
        "stream": True,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": "What is the weather in Paris? Use the get_weather tool." if tool else "Say hi",
            }],
        }],
        "max_output_tokens": 1024 if tool else 512,
    }
    if name == "chatgpt":
        body["store"] = False
    if tool:
        body["tools"] = [{
            "type": "function",
            "name": "get_weather",
            "description": "Get the current weather for a city",
            "parameters": {
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
            },
        }]
        body["tool_choice"] = {"type": "function", "name": "get_weather"}
    return body


def catalog_ids(shape, body):
    """Extract advertised model ids from a models response (pre-scrub parse)."""
    try:
        obj = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError):
        return []
    ids = []
    if shape == "openai":
        for entry in obj.get("data") or []:
            if isinstance(entry, dict) and isinstance(entry.get("id"), str):
                ids.append(entry["id"])
    elif shape == "chatgpt":
        for entry in obj.get("models") or []:
            if not isinstance(entry, dict):
                continue
            if entry.get("visibility", "list") != "list":
                continue
            if isinstance(entry.get("slug"), str):
                ids.append(entry["slug"])
    elif shape == "xai":
        for entry in obj.get("models") or []:
            if not isinstance(entry, dict):
                continue
            modalities = entry.get("output_modalities")
            if isinstance(modalities, list) and "text" in modalities and isinstance(entry.get("id"), str):
                ids.append(entry["id"])
    return ids


def pick_model_from(ids, allowed, preferred):
    for candidate in preferred:
        if candidate in ids and (allowed is None or candidate in allowed):
            return candidate
    for candidate in ids:
        if allowed is None or candidate in allowed:
            return candidate
    return None


def pick_model(name, spec, ids):
    return pick_model_from(ids, spec.get("allowed"), spec.get("preferred") or [])


def existing_catalog(out_dir, spec):
    """Model ids from the on-disk models.json fixture (when models was not re-fetched)."""
    path = out_dir / "models.json"
    if not path.exists():
        return []
    try:
        return catalog_ids(spec["models_shape"], path.read_bytes())
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError):
        return []

# ---------------------------------------------------------------- synthesis


def sse_encode(events) -> bytes:
    parts = []
    for event in events:
        parts.append("data: " + json.dumps(event, ensure_ascii=False, separators=(",", ":")) + "\n\n")
    return "".join(parts).encode("utf-8")


def synth_models(name, spec):
    ids = SYNTH_MODELS.get(name, ["model-a"])
    if spec["models_shape"] == "chatgpt":
        return json.dumps({"models": [
            {"slug": ids[0], "display_name": ids[0], "context_window": 400000,
             "visibility": "list", "supports_parallel_tool_calls": True},
            {"slug": ids[1], "display_name": ids[1], "context_window": 400000,
             "visibility": "list", "supports_parallel_tool_calls": True},
        ]}, indent=2).encode() + b"\n"
    if spec["models_shape"] == "xai":
        return json.dumps({"models": [
            {"id": "grok-4.6", "context_length": 262144,
             "input_modalities": ["text", "image"], "output_modalities": ["text"]},
            {"id": "grok-4", "context_length": 262144,
             "input_modalities": ["text", "image"], "output_modalities": ["text"]},
        ]}, indent=2).encode() + b"\n"
    return json.dumps({"data": [{"id": i, "object": "model"} for i in ids]}, indent=2).encode() + b"\n"


def synth_chat_sse(model, tool=False):
    def chunk(choices):
        return json.dumps(
            {"id": "chatcmpl-mock0001", "object": "chat.completion.chunk", "created": 0,
             "model": model, "choices": choices},
            ensure_ascii=False, separators=(",", ":"))

    lines = []
    if tool:
        lines.append(chunk([{"index": 0, "delta": {"tool_calls": [{
            "index": 0, "id": "call_mock_1", "type": "function",
            "function": {"name": "get_weather", "arguments": "{\"city\":"}}]}, "finish_reason": None}]))
        lines.append(chunk([{"index": 0, "delta": {"tool_calls": [{
            "index": 0, "function": {"arguments": "\"Paris\"}"}}]}, "finish_reason": None}]))
        lines.append(chunk([{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]))
    else:
        lines.append(chunk([{"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": None}]))
        lines.append(chunk([{"index": 0, "delta": {"content": "Hi"}, "finish_reason": None}]))
        lines.append(chunk([{"index": 0, "delta": {"content": "!"}, "finish_reason": None}]))
        lines.append(chunk([{"index": 0, "delta": {}, "finish_reason": "stop"}]))
    lines.append(json.dumps(
        {"id": "chatcmpl-mock0001", "object": "chat.completion.chunk", "created": 0,
         "model": model, "choices": [],
         "usage": {"prompt_tokens": 9, "completion_tokens": 3, "total_tokens": 12}},
        separators=(",", ":")))
    lines.append("[DONE]")
    return ("".join("data: " + line + "\n\n" for line in lines)).encode("utf-8")


def synth_responses_sse(tool=False):
    events = [{"type": "response.created", "response": {"id": "resp_mock_1"}}]
    if tool:
        events.append({"type": "response.output_item.added", "output_index": 0,
                       "item": {"type": "function_call", "id": "fc_mock_1",
                                "call_id": "call_mock_1", "name": "get_weather"}})
        events.append({"type": "response.function_call_arguments.delta", "item_id": "fc_mock_1",
                       "output_index": 0, "delta": "{\"city\":"})
        events.append({"type": "response.function_call_arguments.delta", "item_id": "fc_mock_1",
                       "output_index": 0, "delta": "\"Paris\"}"})
        events.append({"type": "response.output_item.done", "output_index": 0,
                       "item": {"type": "function_call", "id": "fc_mock_1",
                                "call_id": "call_mock_1", "name": "get_weather",
                                "arguments": "{\"city\":\"Paris\"}"}})
    else:
        events.append({"type": "response.output_text.delta", "delta": "Hi"})
        events.append({"type": "response.output_text.delta", "delta": "!"})
    events.append({"type": "response.completed", "response": {
        "id": "resp_mock_1", "status": "completed",
        "usage": {"input_tokens": 9, "output_tokens": 3}}})
    return sse_encode(events)


def synth_messages_sse(tool=False):
    events = [{"type": "message_start", "message": {"id": "msg_mock_1",
               "usage": {"input_tokens": 10, "output_tokens": 1}}}]
    if tool:
        events += [
            {"type": "content_block_start", "index": 0,
             "content_block": {"type": "tool_use", "id": "toolu_mock_1", "name": "get_weather"}},
            {"type": "content_block_delta", "index": 0,
             "delta": {"type": "input_json_delta", "partial_json": "{\"city\":"}},
            {"type": "content_block_delta", "index": 0,
             "delta": {"type": "input_json_delta", "partial_json": "\"Paris\"}"}},
            {"type": "content_block_stop", "index": 0},
            {"type": "message_delta", "delta": {"stop_reason": "tool_use"},
             "usage": {"output_tokens": 5}},
        ]
    else:
        events += [
            {"type": "content_block_start", "index": 0, "content_block": {"type": "text"}},
            {"type": "content_block_delta", "index": 0,
             "delta": {"type": "text_delta", "text": "Hi"}},
            {"type": "content_block_delta", "index": 0,
             "delta": {"type": "text_delta", "text": "!"}},
            {"type": "content_block_stop", "index": 0},
            {"type": "message_delta", "delta": {"stop_reason": "end_turn"},
             "usage": {"output_tokens": 5}},
        ]
    events.append({"type": "message_stop"})
    return sse_encode(events)


def synth_usage():
    def window(percent, hours):
        later = dt.datetime.now(dt.timezone.utc) + dt.timedelta(hours=hours)
        stamp = later.strftime("%Y-%m-%dT%H:%M:%S.") + "%03dZ" % (later.microsecond // 1000)
        return {"status": "ok", "percent": percent, "resetsAt": stamp}

    return (json.dumps({"usage": {
        "rolling": window(12, 6), "weekly": window(34, 48), "monthly": window(57, 240),
    }}, indent=2) + "\n").encode("utf-8")


def synth_stream(name, spec, kind, model):
    if kind.startswith("messages"):
        return synth_messages_sse(tool=kind.endswith("tool"))
    if kind.startswith("responses"):
        return synth_responses_sse(tool=kind.endswith("tool"))
    return synth_chat_sse(model or "model-mock", tool=kind.endswith("tool"))

# ---------------------------------------------------------------- recording


def stream_path_for(transport, spec):
    if transport == MESSAGES:
        return spec.get("messages_path", "/v1/messages")
    if transport == RESPONSES:
        return "/responses"
    return "/chat/completions"


def stream_endpoint_for(transport, spec):
    return "POST " + stream_path_for(transport, spec)


def stream_endpoint(spec):
    return stream_endpoint_for(spec["stream_transport"], spec)


def stream_path(spec):
    return stream_path_for(spec["stream_transport"], spec)


def stream_groups(spec):
    """Ordered transport groups a channel actually uses in production."""
    groups = [{
        "transport": spec["stream_transport"],
        "allowed": spec.get("allowed"),
        "preferred": spec.get("preferred") or [],
        "forced": spec.get("tool_choice_forced", True),
        "note": spec.get("transport_note"),
    }]
    secondary = spec.get("secondary")
    if secondary:
        groups.append({
            "transport": secondary["transport"],
            "allowed": secondary.get("allowed"),
            "preferred": secondary.get("preferred") or [],
            "forced": secondary.get("tool_choice_forced", True),
            "note": secondary.get("note"),
        })
    return groups


def kinds_for_transport(transport):
    if transport == RESPONSES:
        return [
            ("responses_text.sse", "responses_text", False),
            ("responses_tool.sse", "responses_tool", True),
        ]
    if transport == MESSAGES:
        return [
            ("messages_text.sse", "messages_text", False),
            ("messages_tool.sse", "messages_tool", True),
        ]
    return [
        ("chat_text.sse", "chat_text", False),
        ("chat_tool.sse", "chat_tool", True),
    ]


def write_fixture(out_dir, filename, data, meta, entry):
    (out_dir / filename).write_bytes(data)
    entry["file"] = filename
    meta["fixtures"].append(entry)


def record_channel(name, spec, out_root, secrets, only_kinds=None):
    out_dir = out_root / name
    out_dir.mkdir(parents=True, exist_ok=True)
    secret, cred_note = load_channel_credential(name, spec)
    meta = {
        "channel": name,
        "base_url": spec["base"],
        "recorded_at": utcnow_iso(),
        "requests": 0,
        "fixtures": [],
        "notes": [],
    }
    if cred_note:
        meta["notes"].append(cred_note)
    prev = {}
    try:
        loaded = json.loads((out_dir / "meta.json").read_text())
        if isinstance(loaded, dict) and loaded.get("channel") == name:
            prev = loaded
    except (OSError, json.JSONDecodeError, ValueError):
        prev = {}
    prev_by_kind = {
        entry.get("kind"): entry
        for entry in prev.get("fixtures") or []
        if isinstance(entry, dict)
    }
    prev_model = None
    for key in ("chat_text", "responses_text", "messages_text"):
        candidate = (prev_by_kind.get(key) or {}).get("model")
        if isinstance(candidate, str) and candidate:
            prev_model = candidate
            break
    usable = secret is not None
    model = None
    fresh_ids = None

    def wanted(kind):
        return only_kinds is None or kind in only_kinds

    def carry_or_synth(kind, filename, synth_bytes, synth_entry):
        """Keep a previously recorded fixture untouched, or synthesize one."""
        entry = prev_by_kind.get(kind)
        if entry and entry.get("file") == filename and (out_dir / filename).exists():
            meta["fixtures"].append(entry)
            return
        write_fixture(out_dir, filename, synth_bytes, meta, synth_entry)

    if spec.get("models_path"):
        synth_entry = {
            "kind": "models", "source": "synthetic", "endpoint": "GET " + spec["models_path"],
            "transport": None, "model": SYNTH_MODELS.get(name, ["model-a"])[0],
        }
        if not wanted("models"):
            carry_or_synth("models", "models.json", synth_models(name, spec), synth_entry)
        elif usable:
            ok, result = try_request(
                "GET", spec["base"] + spec["models_path"],
                channel_headers(name, spec, secret), timeout=60)
            meta["requests"] += 1
            if ok:
                status, headers, body = result
                fresh_ids = catalog_ids(spec["models_shape"], body)
                model = pick_model_from(fresh_ids, spec.get("allowed"), spec.get("preferred") or [])
                write_fixture(out_dir, "models.json", scrub_json(body, secrets), meta, {
                    "kind": "models", "source": "real", "endpoint": "GET " + spec["models_path"],
                    "transport": None, "model": None, "http_status": status,
                    "content_type": headers.get("Content-Type", ""),
                })
            else:
                meta["notes"].append("models 真实录制失败（%s），回退合成 fixture（未经真实录制）" % result)
                write_fixture(out_dir, "models.json", synth_models(name, spec), meta, synth_entry)
        else:
            write_fixture(out_dir, "models.json", synth_models(name, spec), meta, synth_entry)

    if spec.get("usage_path"):
        synth_entry = {
            "kind": "usage", "source": "synthetic", "endpoint": "GET " + spec["usage_path"],
            "transport": None, "model": None,
        }
        if not wanted("usage"):
            carry_or_synth("usage", "usage.json", synth_usage(), synth_entry)
        elif usable:
            ok, result = try_request(
                "GET", spec["base"] + spec["usage_path"],
                channel_headers(name, spec, secret), timeout=60)
            meta["requests"] += 1
            if ok:
                status, headers, body = result
                write_fixture(out_dir, "usage.json", scrub_json(body, secrets), meta, {
                    "kind": "usage", "source": "real", "endpoint": "GET " + spec["usage_path"],
                    "transport": None, "model": None, "http_status": status,
                    "content_type": headers.get("Content-Type", ""),
                })
            else:
                meta["notes"].append("usage 真实录制失败（%s），回退合成 fixture（未经真实录制）" % result)
                synth_entry["note"] = "真实录制失败（%s），未经真实录制" % result
                write_fixture(out_dir, "usage.json", synth_usage(), meta, synth_entry)
        else:
            write_fixture(out_dir, "usage.json", synth_usage(), meta, synth_entry)

    tool_marker = {
        CHAT: b"tool_calls",
        RESPONSES: b"function_call",
        MESSAGES: b"tool_use",
    }
    catalog = fresh_ids if fresh_ids is not None else existing_catalog(out_dir, spec)
    primary = True
    for group in stream_groups(spec):
        transport = group["transport"]
        picked = pick_model_from(catalog, group["allowed"], group["preferred"])
        if primary:
            chosen = picked or prev_model or (group["preferred"] or [None])[0]
        else:
            chosen = picked or (group["preferred"] or [None])[0]
        for filename, kind, tool in kinds_for_transport(transport):
            endpoint = stream_endpoint_for(transport, spec)
            synth_entry = {
                "kind": kind, "source": "synthetic", "endpoint": endpoint,
                "transport": transport,
                "model": chosen or SYNTH_MODELS.get(name, ["model-a"])[0],
            }
            if group.get("note"):
                synth_entry["note"] = group["note"]
            if not wanted(kind):
                carry_or_synth(kind, filename, synth_stream(name, spec, kind, chosen), synth_entry)
                continue
            if usable and chosen:
                if transport == RESPONSES:
                    body = responses_body(name, chosen, tool)
                else:
                    body = chat_body(chosen, tool, forced=group["forced"])
                ok, result = try_request(
                    "POST", spec["base"] + stream_path_for(transport, spec),
                    channel_headers(name, spec, secret, streaming=True), body=body, timeout=120)
                meta["requests"] += 1
                if ok:
                    status, headers, raw = result
                    if tool and tool_marker[transport] not in raw:
                        meta["notes"].append(
                            "%s 真实响应未产生工具调用，回退合成 fixture（未经真实录制）" % kind)
                    else:
                        entry = {
                            "kind": kind, "source": "real", "endpoint": endpoint,
                            "transport": transport, "model": chosen,
                            "http_status": status,
                            "content_type": headers.get("Content-Type", ""),
                            "prompt": body["messages"][0]["content"]
                            if transport != RESPONSES else
                            body["input"][0]["content"][0]["text"],
                            "max_tokens": 1024 if tool else 512,
                        }
                        if group.get("note"):
                            entry["note"] = group["note"]
                        write_fixture(out_dir, filename, scrub_sse(raw, secrets), meta, entry)
                        continue
                else:
                    meta["notes"].append(
                        "%s 真实录制失败（%s），回退合成 fixture（未经真实录制）" % (kind, result))
                    synth_entry["note"] = "; ".join(
                        part for part in (synth_entry.get("note"),
                                          "真实录制失败（%s），未经真实录制" % result) if part)
            elif usable:
                meta["notes"].append(
                    "%s 无法选到可请求的真实模型 id，回退合成 fixture（未经真实录制）" % kind)
            write_fixture(out_dir, filename, synth_stream(name, spec, kind, chosen), meta, synth_entry)
        primary = False

    meta_text = scrub_text(json.dumps(meta, ensure_ascii=False, indent=2), secrets)
    (out_dir / "meta.json").write_text(meta_text + "\n", encoding="utf-8")
    return meta

# ---------------------------------------------------------------- verifying


def sse_payloads(data: bytes):
    payloads = []
    for line in data.decode("utf-8", errors="replace").splitlines():
        stripped = line.strip()
        if stripped.startswith("data:"):
            payloads.append(stripped[5:].strip())
    return [p for p in payloads if p]


def check_chat_sse(payloads, kind):
    errors = []
    saw_done = saw_finish = saw_delta = False
    tool_named = []
    tool_args = {}
    for payload in payloads:
        if payload == "[DONE]":
            saw_done = True
            continue
        try:
            obj = json.loads(payload)
        except (json.JSONDecodeError, ValueError):
            errors.append("non-JSON data payload: %r" % payload[:48])
            continue
        if not isinstance(obj, dict):
            errors.append("data payload is not a JSON object")
            continue
        for choice in obj.get("choices") or []:
            if not isinstance(choice, dict):
                errors.append("choice entry is not an object")
                continue
            delta = choice.get("delta") or {}
            content = delta.get("content")
            reasoning = delta.get("reasoning_content", delta.get("reasoning"))
            if (isinstance(content, str) and content.strip()) or (
                    isinstance(reasoning, str) and reasoning.strip()):
                saw_delta = True
            for call in delta.get("tool_calls") or []:
                if not isinstance(call, dict):
                    continue
                function = call.get("function") or {}
                if isinstance(function.get("name"), str) and function["name"]:
                    tool_named.append(function["name"])
                if isinstance(function.get("arguments"), str) and function["arguments"]:
                    index = call.get("index", 0)
                    tool_args[index] = tool_args.get(index, "") + function["arguments"]
            if choice.get("finish_reason"):
                saw_finish = True
        usage = obj.get("usage")
        if isinstance(usage, dict) and usage.get("total_tokens") is not None:
            saw_delta = saw_delta or bool(usage)
    if not (saw_done or saw_finish):
        errors.append("no [DONE] sentinel and no finish_reason chunk")
    if kind == "chat_text" and not saw_delta:
        errors.append("text fixture has no content/reasoning delta")
    if kind == "chat_tool":
        if not tool_named:
            errors.append("tool fixture has no tool_calls chunk carrying function.name")
        joined = tool_args.get(0, "")
        if joined:
            try:
                json.loads(joined)
            except (json.JSONDecodeError, ValueError):
                errors.append("concatenated tool arguments are not valid JSON")
        elif not errors:
            errors.append("tool fixture carries no function.arguments delta")
    return errors


def check_responses_sse(payloads, kind):
    errors = []
    types = []
    arg_deltas = {}
    named_function_items = 0
    done_items = 0
    for payload in payloads:
        try:
            obj = json.loads(payload)
        except (json.JSONDecodeError, ValueError):
            errors.append("non-JSON data payload: %r" % payload[:48])
            continue
        if not isinstance(obj, dict) or not isinstance(obj.get("type"), str):
            errors.append("event without string type field")
            continue
        event_type = obj["type"]
        types.append(event_type)
        if not (event_type.startswith("response.") or event_type == "error"):
            errors.append("unexpected event type %r" % event_type)
        if event_type == "response.output_item.added":
            item = obj.get("item") or {}
            if item.get("type") == "function_call" and item.get("name"):
                named_function_items += 1
        if event_type == "response.function_call_arguments.delta":
            arg_deltas[obj.get("item_id", "")] = arg_deltas.get(obj.get("item_id", ""), "") + str(obj.get("delta", ""))
        if event_type == "response.output_item.done":
            item = obj.get("item") or {}
            if item.get("type") == "function_call":
                done_items += 1
    if not (set(types) & {"response.completed", "response.incomplete", "response.failed", "error"}):
        errors.append("no terminal event (completed/incomplete/failed/error)")
    if kind == "responses_text" and "response.output_text.delta" not in types:
        errors.append("text fixture has no response.output_text.delta")
    if kind == "responses_tool":
        if not named_function_items:
            errors.append("tool fixture has no function_call output_item.added")
        if not arg_deltas:
            errors.append("tool fixture has no function_call_arguments.delta")
        elif not done_items:
            errors.append("tool fixture has no function_call output_item.done")
        else:
            try:
                json.loads("".join(arg_deltas.values()))
            except (json.JSONDecodeError, ValueError):
                errors.append("concatenated function_call arguments are not valid JSON")
    return errors


def check_messages_sse(payloads, kind):
    errors = []
    events = []
    for payload in payloads:
        try:
            obj = json.loads(payload)
        except (json.JSONDecodeError, ValueError):
            errors.append("non-JSON data payload: %r" % payload[:48])
            continue
        if isinstance(obj, dict) and isinstance(obj.get("type"), str):
            events.append(obj)
        else:
            errors.append("event without string type field")
    types = [event["type"] for event in events]
    if not types or types[0] != "message_start":
        errors.append("first event must be message_start")
    if "message_stop" not in types:
        errors.append("missing message_stop")
    if "message_delta" not in types:
        errors.append("missing message_delta")
    started, stopped = set(), set()
    saw_text = False
    json_args = []
    for event in events:
        event_type = event["type"]
        index = event.get("index")
        if event_type == "content_block_start":
            if index in started:
                errors.append("duplicate content_block_start for index %r" % index)
            started.add(index)
        elif event_type == "content_block_delta":
            if index not in started or index in stopped:
                errors.append("content_block_delta outside open block (index %r)" % index)
            delta = event.get("delta") or {}
            if delta.get("type") == "text_delta" and str(delta.get("text", "")).strip():
                saw_text = True
            if delta.get("type") == "input_json_delta":
                json_args.append(str(delta.get("partial_json", "")))
        elif event_type == "content_block_stop":
            if index not in started or index in stopped:
                errors.append("content_block_stop without open block (index %r)" % index)
            stopped.add(index)
    if kind == "messages_text" and not saw_text:
        errors.append("text fixture has no text_delta")
    if kind == "messages_tool":
        if not json_args:
            errors.append("tool fixture has no input_json_delta")
        else:
            try:
                json.loads("".join(json_args))
            except (json.JSONDecodeError, ValueError):
                errors.append("concatenated input_json_delta is not valid JSON")
    return errors


def check_models(obj, shape):
    errors = []
    if shape == "openai":
        data = obj.get("data") if isinstance(obj, dict) else None
        if not isinstance(data, list):
            return ["models response must contain a data array"]
        for entry in data:
            if not (isinstance(entry, dict) and isinstance(entry.get("id"), str) and entry["id"]):
                errors.append("data entry missing string id")
                break
    elif shape == "chatgpt":
        models = obj.get("models") if isinstance(obj, dict) else None
        if not isinstance(models, list):
            return ["models response must contain a models array"]
        for entry in models:
            if not (isinstance(entry, dict) and isinstance(entry.get("slug"), str) and entry["slug"]):
                errors.append("models entry missing string slug")
                break
    elif shape == "xai":
        models = obj.get("models") if isinstance(obj, dict) else None
        if not isinstance(models, list):
            return ["models response must contain a models array"]
        for entry in models:
            if not (isinstance(entry, dict) and isinstance(entry.get("id"), str) and entry["id"]):
                errors.append("models entry missing string id")
                break
    return errors


def check_usage(obj):
    errors = []
    usage = obj.get("usage") if isinstance(obj, dict) else None
    if not isinstance(usage, dict):
        return ["usage response must contain a usage object"]
    for window_name in ("rolling", "weekly", "monthly"):
        window = usage.get(window_name)
        if not isinstance(window, dict):
            errors.append("window %s missing or not an object" % window_name)
            continue
        status = window.get("status")
        percent = window.get("percent")
        if status not in ("ok", "rate-limited"):
            errors.append("%s.status must be ok|rate-limited" % window_name)
        if not isinstance(percent, int) or isinstance(percent, bool):
            errors.append("%s.percent must be an integer" % window_name)
        elif status == "ok" and not 0 <= percent <= 99:
            errors.append("%s.percent must be 0-99 when ok" % window_name)
        elif status == "rate-limited" and percent != 100:
            errors.append("%s.percent must be 100 when rate-limited" % window_name)
        resets = window.get("resetsAt")
        if not (isinstance(resets, str) and RESETS_RE.match(resets)):
            errors.append("%s.resetsAt must be YYYY-MM-DDTHH:mm:ss.sssZ" % window_name)
        else:
            try:
                dt.datetime.strptime(resets, "%Y-%m-%dT%H:%M:%S.%fZ")
            except ValueError:
                errors.append("%s.resetsAt is not a real calendar date" % window_name)
    return errors


def check_redacted_keys(value, label):
    """Identity-bearing keys must already be [REDACTED] inside fixture JSON."""
    errors = []
    if isinstance(value, dict):
        for key, item in value.items():
            if isinstance(key, str) and SENSITIVE_KEY_RE.search(key) and item != "[REDACTED]":
                errors.append("%s: key %r must be redacted to [REDACTED]" % (label, key))
            errors += check_redacted_keys(item, label)
    elif isinstance(value, list):
        for item in value:
            errors += check_redacted_keys(item, label)
    return errors


def check_sanitized(relative, data, secrets):
    text = data.decode("utf-8", errors="replace")
    errors = []
    for label, pattern in (
        ("bearer token", BEARER_RE), ("email", EMAIL_RE),
        ("JWT", JWT_RE), ("api key", APIKEY_RE), ("authorization header", AUTHZ_RE),
    ):
        match = pattern.search(text)
        if match:
            errors.append("%s contains %s" % (relative, label))
    for secret in secrets:
        if secret in text:
            errors.append("%s contains known credential material" % relative)
            break
    return errors


def verify_channel(out_dir, spec, secrets):
    errors = []
    meta_path = out_dir / "meta.json"
    if not meta_path.exists():
        return ["meta.json missing"]
    raw = meta_path.read_bytes()
    errors += check_sanitized("meta.json", raw, secrets)
    try:
        meta = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
        return errors + ["meta.json not valid JSON: %s" % error]
    if meta.get("channel") != out_dir.name:
        errors.append("meta channel mismatch")
    fixtures = meta.get("fixtures")
    if not isinstance(fixtures, list) or not fixtures:
        return errors + ["meta fixtures list missing or empty"]
    for entry in fixtures:
        if not isinstance(entry, dict):
            errors.append("fixture entry is not an object")
            continue
        filename = entry.get("file")
        kind = entry.get("kind")
        source = entry.get("source")
        if not filename or not kind:
            errors.append("fixture entry missing file/kind")
            continue
        if source not in ("real", "synthetic"):
            errors.append("%s: source must be real|synthetic" % filename)
        if source == "real" and not isinstance(entry.get("http_status"), int):
            errors.append("%s: real fixture missing http_status" % filename)
        path = out_dir / filename
        if not path.exists():
            errors.append("%s: fixture file missing" % filename)
            continue
        data = path.read_bytes()
        errors += check_sanitized(filename, data, secrets)
        if kind == "models":
            try:
                obj = json.loads(data.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
                errors.append("models.json not valid JSON: %s" % error)
            else:
                errors += check_redacted_keys(obj, filename)
                errors += check_models(obj, spec["models_shape"])
        elif kind == "usage":
            try:
                obj = json.loads(data.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
                errors.append("usage.json not valid JSON: %s" % error)
            else:
                errors += check_redacted_keys(obj, filename)
                errors += check_usage(obj)
        elif kind in ("chat_text", "chat_tool", "responses_text", "responses_tool",
                      "messages_text", "messages_tool"):
            payloads = sse_payloads(data)
            for payload in payloads:
                if payload == "[DONE]":
                    continue
                try:
                    errors += check_redacted_keys(json.loads(payload), filename)
                except (json.JSONDecodeError, ValueError):
                    pass  # shape checkers below report non-JSON payloads
            if kind in ("chat_text", "chat_tool"):
                errors += check_chat_sse(payloads, kind)
            elif kind in ("responses_text", "responses_tool"):
                errors += check_responses_sse(payloads, kind)
            else:
                errors += check_messages_sse(payloads, kind)
        else:
            errors.append("%s: unknown fixture kind %r" % (filename, kind))
    expected_kinds = set()
    for group in stream_groups(spec):
        for _filename, kind, _tool in kinds_for_transport(group["transport"]):
            expected_kinds.add(kind)
    if spec.get("models_path"):  # anthropic uses a static catalog, no HTTP models endpoint
        expected_kinds.add("models")
    if spec.get("usage_path"):
        expected_kinds.add("usage")
    actual_kinds = {entry.get("kind") for entry in fixtures if isinstance(entry, dict)}
    for missing in sorted(expected_kinds - actual_kinds):
        errors.append("missing required fixture kind: %s" % missing)
    return errors

# ---------------------------------------------------------------- commands


def parse_only(value):
    return {item.strip() for item in value.split(",") if item.strip()} if value else None


def cmd_record(args):
    selected = parse_only(args.only)
    secrets = collect_secrets()
    results = []
    for name, spec in CHANNELS.items():
        if selected and name not in selected:
            continue
        results.append(record_channel(name, spec, args.out, secrets, only_kinds=parse_only(args.kinds)))
    for meta in results:
        real = [entry["kind"] for entry in meta["fixtures"] if entry.get("source") == "real"]
        synthetic = [entry["kind"] for entry in meta["fixtures"] if entry.get("source") == "synthetic"]
        print("%s: requests=%d real=[%s] synthetic=[%s] notes=%d" % (
            meta["channel"], meta["requests"], ", ".join(real) or "-",
            ", ".join(synthetic) or "-", len(meta["notes"])))
        for note in meta["notes"]:
            print("  note: %s" % note)
    return 0


def cmd_synthesize(args):
    selected = parse_only(args.only)
    secrets = collect_secrets()
    for name, spec in CHANNELS.items():
        if selected and name not in selected:
            continue
        meta = {
            "channel": name, "base_url": spec["base"], "recorded_at": utcnow_iso(),
            "requests": 0, "fixtures": [], "notes": ["未经真实录制：按契约测试形状合成"],
        }
        out_dir = args.out / name
        out_dir.mkdir(parents=True, exist_ok=True)
        if spec.get("models_path"):
            (out_dir / "models.json").write_bytes(synth_models(name, spec))
            meta["fixtures"].append({
                "kind": "models", "source": "synthetic", "file": "models.json",
                "endpoint": "GET " + spec["models_path"], "transport": None,
                "model": SYNTH_MODELS.get(name, ["model-a"])[0],
            })
        if spec.get("usage_path"):
            (out_dir / "usage.json").write_bytes(synth_usage())
            meta["fixtures"].append({
                "kind": "usage", "source": "synthetic", "file": "usage.json",
                "endpoint": "GET " + spec["usage_path"], "transport": None, "model": None,
            })
        for group in stream_groups(spec):
            transport = group["transport"]
            model = (group["preferred"] or ["model-mock"])[0]
            for filename, kind, _tool in kinds_for_transport(transport):
                (out_dir / filename).write_bytes(synth_stream(name, spec, kind, model))
                entry = {
                    "kind": kind, "source": "synthetic", "file": filename,
                    "endpoint": stream_endpoint_for(transport, spec), "transport": transport,
                    "model": model,
                }
                if group.get("note"):
                    entry["note"] = group["note"]
                meta["fixtures"].append(entry)
        (out_dir / "meta.json").write_text(
            scrub_text(json.dumps(meta, ensure_ascii=False, indent=2), secrets) + "\n", encoding="utf-8")
        print("%s: synthesized %d fixtures" % (name, len(meta["fixtures"])))
    return 0


def cmd_verify(args):
    root = args.out
    if not root.is_dir():
        print("no fixtures directory at %s" % root)
        return 2
    secrets = collect_secrets()
    failures = 0
    known_dirs = []
    for path in sorted(root.iterdir()):
        if path.is_dir() and path.name in CHANNELS:
            known_dirs.append(path)
        elif path.is_dir():
            # Other agents own scenario/sample dirs (e.g. fixtures/mock/synthetic).
            print("%s: skipped (not a capture channel)" % path.name)
    for out_dir in known_dirs:
        spec = CHANNELS[out_dir.name]
        errors = verify_channel(out_dir, spec, secrets)
        if errors:
            failures += 1
            print("%s: FAIL" % out_dir.name)
            for error in errors:
                print("  - %s" % error)
        else:
            meta = json.loads((out_dir / "meta.json").read_text())
            kinds = ", ".join(
                "%s(%s)" % (entry["kind"], entry.get("source")) for entry in meta["fixtures"])
            print("%s: OK [%s]" % (out_dir.name, kinds))
    print("verify: %d/%d channels OK" % (len(known_dirs) - failures, len(known_dirs)))
    return 1 if failures else 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    record_parser = sub.add_parser("record")
    record_parser.add_argument("--only", help="comma-separated channel ids")
    record_parser.add_argument("--kinds", help="comma-separated fixture kinds to re-record")
    record_parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    synth_parser = sub.add_parser("synthesize")
    synth_parser.add_argument("--only", help="comma-separated channel ids")
    synth_parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    verify_parser = sub.add_parser("verify")
    verify_parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    args = parser.parse_args(argv)
    if args.command == "record":
        return cmd_record(args)
    if args.command == "synthesize":
        return cmd_synthesize(args)
    return cmd_verify(args)


if __name__ == "__main__":
    sys.exit(main())
