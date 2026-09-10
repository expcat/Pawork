#!/usr/bin/env python3
"""Pawork 本地 Provider mock server（MOCK-3）。

单端口承载全部通道 persona：按 Authorization: Bearer mock-<channel>
（anthropic 另认 x-api-key: mock-anthropic）区分通道，路由
docs/mock-simulation-plan.md §2.2 端点矩阵：

    GET  /models                    OpenAI data[] 目录（glm-coding / opencode-go /
                                    qwen-token-plan / deepseek / kimi-platform / kimi-code）
    GET  /models?client_version=…   ChatGPT models[].slug 目录（query 原样接受）
    GET  /language-models           xAI models[] 目录（output_modalities 含 text）
    GET  /usage                     opencode-go 三窗额度（§2.3 红线形状）
    POST /chat/completions          Chat Completions SSE
    POST /responses                 Responses SSE（chatgpt / xai grok-4、grok-4-fast /
                                    Go Responses 组；未登记/错组模型按生产语义 400/404）
    POST /v1/messages               Anthropic Messages SSE

fixture 查找（<fixtures-root>/<channel>/，显式 ?fixture=<basename> 优先）：
    models / usage → <kind>.json；xai 目录 language-models.json 优先、
                     其次 models.json（MOCK-2 录制命名，内容即 language-models 形状）
    chat      → chat_<variant>.sse（MOCK-2 录制命名；?variant=tool 选工具流，
                默认 text）；亦认 chat-completions[.<model>].sse 等分隔变体
    responses → responses_<variant>.sse / responses[.<model>].sse
    messages  → messages_<variant>.sse / messages[.<model>].sse
    命中即按原始字节定速回放（--chunk-bytes / --chunk-interval-ms）；
    未命中按通道 transport 回最小合法兜底响应（形状对照 providers 契约测试）。

环境变量：MOCK_HOST / MOCK_PORT / MOCK_FIXTURES_ROOT / MOCK_CHUNK_INTERVAL_MS /
MOCK_CHUNK_BYTES（命令行参数优先）。

MOCK-4/5/6 扩展点：路由集中在 ROUTES 表；场景切换 POST /__control 与 OAuth
device/token 端点各加一行即可，persona 解析 / fixture 回放 / 定速机制复用。

MOCK-4 场景库（fixtures/mock/scenarios/manifest.json 驱动，详见该文件注释）：
    触发  (a) 请求体任意文本含 MOCK:<NAME 大写>（关键字优先）；
          (b) POST /__control {"scenario":"<name>"} 切全局场景，
              {"scenario":null} 或重启复位；GET /__control 读当前值与清单。
    生效  kind=http_error —— 全部 persona 端点直接回状态码 + 可选 Retry-After
          （§2.5：错误体被读但不解析，故只发最小 JSON 体）；
          kind=sse —— 仅对话端点（transports 匹配）回放场景 SSE 字节；
          kind=slow —— 正常流解析不变（fixture 命中优先，否则兜底），
          按场景 interval_ms 定速，query interval_ms 可覆盖（0-60000ms）。

MOCK-5 OAuth 层（§2.1；登录阶段尚无 Bearer，端点在 persona 检查前路由，
不参与 MOCK-4 场景机制）：
    POST /oauth2/device/code             xai Device Flow 设备码
    POST /oauth2/token                   xai token（device 轮询 / refresh）
    POST /api/oauth/device_authorization kimi-code Device Flow 设备码
    POST /api/oauth/token                kimi-code token（device 轮询 / refresh）
    POST /oauth/token                    chatgpt PKCE token（authorization_code /
                                         refresh；成功含带 chatgpt_account_id claim
                                         的未签名 id_token，程序本地提取不验签）
    GET  /device                         人工验证页（展示 user_code）

    轮询剧本经 POST /__control {"oauth":{"<channel>":"<script>"}} 切换，
    {"oauth":null} 或重启复位；复位同时清空 device_code 轮询计数，旧
    device_code 立即失效，换剧本后必须重新走 device/code。GET /__control
    一并读回剧本。剧本状态按 device_code 维度维护（服务端计数，线程安全）：
      pending_then_success（默认）第 1-2 次轮询 authorization_pending，之后成功
      slow_down_then_success  第 1 次 slow_down、第 2 次 authorization_pending
      immediate_success       首次轮询即成功
      expired_token           恒 expired_token
      invalid_grant           任意 token 请求恒 {"error":"invalid_grant"}
      refresh_no_rotation     device 轮询同默认剧本；refresh 成功响应缺
                              refresh_token 与 expires_in（客户端保留旧值形状）
"""

from __future__ import annotations

import argparse
import base64
import html
import json
import os
import secrets
import re
import sys
import threading
import time
import traceback
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit


# --- 通道 persona 与逐模型 transport 表（对照 channels/api_key.rs 快照） -------

CHAT_CHANNELS = (
    "glm-coding",
    "opencode-go",
    "qwen-token-plan",
    "deepseek",
    "kimi-platform",
    "kimi-code",
)

OPENCODE_RESPONSES_MODELS = frozenset(
    {"grok-4.6", "gpt-5.6-luna", "muse-spark-1.3-contributor", "muse-spark-1.2-contributor"}
)
OPENCODE_CHAT_MODELS = frozenset(
    {
        "glm-5.3-flash",
        "glm-5.3",
        "glm-5.2",
        "glm-5.1",
        "kimi-k3",
        "kimi-k2.7-code",
        "kimi-k2.6",
        "longcat-2.0",
        "deepseek-v4-pro",
        "deepseek-v4-flash",
        "deepseek-v4-flash-vision-exp",
        "mimo-v2.5",
        "mimo-v2.5-pro",
        "hy4-preview",
        "hy3",
        "omen-alpha",
    }
)
OPENCODE_MESSAGES_MODELS = frozenset(
    {
        "minimax-m3",
        "minimax-m2.7",
        "minimax-m2.5",
        "qwen3.8-max",
        "qwen3.8-flash",
        "qwen3.7-max",
        "qwen3.7-plus",
        "qwen3.6-plus",
    }
)
QWEN_TOKEN_PLAN_MODELS = (
    "qwen3.8-max",
    "qwen3.8-max-preview",
    "qwen3.8-flash",
    "qwen3.7-max",
    "qwen3.7-plus",
    "qwen3.6-flash",
    "glm-5.2",
    "deepseek-v4-pro",
    "deepseek-v4-pro-0813",
    "deepseek-v4-flash-0731",
)
QWEN_TOKEN_PLAN_MODELS_SET = frozenset(QWEN_TOKEN_PLAN_MODELS)

# xAI builtin 白名单：仅 grok-4 / grok-4-fast 走 Responses；grok-4.6 等其余
# id（含未知 id）生产默认 Chat Completions（channels/xai.rs transport_for）。
XAI_RESPONSES_MODELS = frozenset({"grok-4", "grok-4-fast"})

PERSONA_CHANNELS = (
    "chatgpt",
    "xai",
    "glm-coding",
    "opencode-go",
    "qwen-token-plan",
    "deepseek",
    "kimi-platform",
    "kimi-code",
    "anthropic",
)
PERSONA_TOKENS = {f"mock-{channel}": channel for channel in PERSONA_CHANNELS}
PERSONA_TOKENS.update({channel: channel for channel in PERSONA_CHANNELS})
# MOCK-5：OAuth 端点签发的 access token（mock-<channel>-access-<n>）等同该通道
# persona——登录/刷新后的 provider 请求带的就是这些 token。
MOCK_ISSUED_TOKEN = re.compile(r"^mock-([a-z0-9][a-z0-9-]*)-access-[0-9]+$")


# --- MOCK-4 场景库 -----------------------------------------------------------

SCENARIO_KEYWORD = re.compile(r"\bMOCK:([A-Za-z0-9_]+)")


class Scenario:
    """manifest.json 中一条场景定义；kind 决定生效位置（见模块 docstring）。"""

    def __init__(self, raw: dict):
        self.name = str(raw["name"])
        self.kind = str(raw["kind"])  # http_error / sse / slow
        self.status = int(raw.get("status") or 0)
        self.retry_after = raw.get("retry_after")
        self.error_kind = raw.get("error_kind") or ""
        self.transports = frozenset(raw.get("transports") or ())
        self.fixture = raw.get("fixture") or ""
        self.interval_ms = float(raw.get("interval_ms") or 0.0)
        self.description = raw.get("description") or ""

    @property
    def key(self) -> str:
        return self.name.upper()


def load_scenarios(fixtures_root: Path) -> dict:
    """加载 fixtures/mock/scenarios/manifest.json；缺失或畸形时返回空表。"""
    manifest = fixtures_root / "scenarios" / "manifest.json"
    if not manifest.is_file():
        return {}
    try:
        raw = json.loads(manifest.read_text(encoding="utf-8"))
        entries = raw.get("scenarios")
    except ValueError:
        return {}
    scenarios = {}
    if isinstance(entries, list):
        for entry in entries:
            if isinstance(entry, dict) and entry.get("name") and entry.get("kind"):
                scenario = Scenario(entry)
                scenarios[scenario.key] = scenario
    return scenarios


def scenario_keyword(body) -> "str | None":
    """递归收集请求体全部字符串，取首个 MOCK:<NAME> 关键字（小写返回）。

    覆盖 Chat messages[].content、Responses input（字符串或分块数组）、
    Anthropic content blocks 等一切放置方式，无需 transport 专属代码。
    """
    found = []

    def walk(value):
        if isinstance(value, str):
            match = SCENARIO_KEYWORD.search(value)
            if match:
                found.append(match.group(1).lower())
        elif isinstance(value, list):
            for item in value:
                walk(item)
        elif isinstance(value, dict):
            for item in value.values():
                walk(item)

    walk(body)
    return found[0] if found else None


# --- MOCK-5 OAuth 端点与轮询剧本（§2.1） --------------------------------------

OAUTH_CHANNELS = ("xai", "kimi-code", "chatgpt")
# chatgpt 无 device 端点，只接受 token 侧剧本。
OAUTH_TOKEN_ONLY_SCRIPTS = frozenset(
    {"immediate_success", "invalid_grant", "refresh_no_rotation"}
)
OAUTH_SCRIPTS = frozenset(
    {
        "pending_then_success",
        "slow_down_then_success",
        "immediate_success",
        "expired_token",
        "invalid_grant",
        "refresh_no_rotation",
    }
)
DEFAULT_OAUTH_SCRIPT = "pending_then_success"
DEVICE_GRANT = "urn:ietf:params:oauth:grant-type:device_code"


def oauth_script_allowed(channel: str, script: str) -> bool:
    if script not in OAUTH_SCRIPTS:
        return False
    return channel != "chatgpt" or script in OAUTH_TOKEN_ONLY_SCRIPTS


def device_poll_error(script: str, poll: int):
    """device 轮询第 poll 次应返回的 OAuth error；None 表示成功。"""
    if script == "immediate_success":
        return None
    if script == "expired_token":
        return "expired_token"
    if script == "invalid_grant":
        return "invalid_grant"
    if poll == 1 and script == "slow_down_then_success":
        return "slow_down"
    pending_budget = 2
    return "authorization_pending" if poll <= pending_budget else None


def _b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def build_mock_id_token(sequence: int) -> str:
    """未签名 JWT：payload 带 https://api.openai.com/auth.chatgpt_account_id claim，
    供程序本地提取 account id（生产不验签，签名段留空）。"""
    header = {"alg": "none", "typ": "JWT"}
    payload = {
        "iss": "https://auth.openai.com/",
        "sub": f"mock-user-{sequence}",
        "aud": "app_EMoamEEZ73f0CkXaXp7hrann",
        "https://api.openai.com/auth": {"chatgpt_account_id": f"acct-mock-{sequence}"},
    }
    return (
        _b64url(json.dumps(header).encode("utf-8"))
        + "."
        + _b64url(json.dumps(payload).encode("utf-8"))
        + "."
    )


# --- 兜底目录（/models data[] 与各通道专属目录形状） ---------------------------

CATALOG_DATA = {
    "glm-coding": ["glm-5.3", "glm-5.3-flash", "glm-5.2", "glm-5.1"],
    "opencode-go": [
        *sorted(OPENCODE_RESPONSES_MODELS),
        *sorted(OPENCODE_CHAT_MODELS),
        *sorted(OPENCODE_MESSAGES_MODELS),
    ],
    "qwen-token-plan": list(QWEN_TOKEN_PLAN_MODELS),
    "deepseek": ["deepseek-v4-pro", "deepseek-v4-flash", "deepseek-v4-flash-vision-exp"],
    "kimi-platform": ["kimi-k3", "kimi-k2.6"],
    "kimi-code": ["kimi-k3", "kimi-k2.7-code", "kimi-k2.6"],
}

CHATGPT_CATALOG = [
    {
        "slug": "gpt-5.6-terra",
        "display_name": "GPT-5.6 Terra",
        "visibility": "list",
        "context_window": 400000,
    },
    {
        "slug": "gpt-5.6-sol",
        "display_name": "GPT-5.6 Sol",
        "visibility": "list",
        "context_window": 400000,
    },
]

XAI_CATALOG = [
    {
        "id": "grok-4.6",
        "output_modalities": ["text"],
        "input_modalities": ["text", "image"],
        "context_length": 256000,
    },
    {"id": "grok-4", "output_modalities": ["text"], "context_length": 256000},
    {"id": "grok-3", "output_modalities": ["text"], "context_length": 131072},
]


# --- 最小合法兜底流（形状对照 crates/providers/tests 契约样例） -----------------


def build_chat_sse(channel: str, model: str) -> bytes:
    text = f"mock reply from {channel} (model {model or 'unknown'})"
    parts = [
        "data: "
        + json.dumps({"choices": [{"delta": {"content": text}}]}, ensure_ascii=False)
        + "\n\n",
        'data: {"choices":[{"delta":{},"finish_reason":"stop"}]}\n\n',
        "data: [DONE]\n\n",
    ]
    return "".join(parts).encode("utf-8")


def build_responses_sse(channel: str, model: str) -> bytes:
    response_id = "resp_mock_1"
    text = f"mock reply from {channel} (model {model or 'unknown'})"
    parts = [
        "data: "
        + json.dumps({"type": "response.created", "response": {"id": response_id}})
        + "\n\n",
        "data: "
        + json.dumps({"type": "response.output_text.delta", "delta": text}, ensure_ascii=False)
        + "\n\n",
        "data: "
        + json.dumps(
            {
                "type": "response.completed",
                "response": {
                    "id": response_id,
                    "status": "completed",
                    "usage": {"input_tokens": 12, "output_tokens": 3},
                },
            }
        )
        + "\n\n",
    ]
    return "".join(parts).encode("utf-8")


def build_messages_sse(channel: str, model: str) -> bytes:
    text = f"mock reply from {channel} (model {model or 'unknown'})"
    events = [
        json.dumps(
            {
                "type": "message_start",
                "message": {
                    "id": "msg_mock_1",
                    "usage": {"input_tokens": 10, "output_tokens": 1},
                },
            }
        ),
        json.dumps(
            {"type": "content_block_start", "index": 0, "content_block": {"type": "text"}}
        ),
        json.dumps(
            {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": text},
            }
        ),
        json.dumps({"type": "content_block_stop", "index": 0}),
        json.dumps(
            {
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn"},
                "usage": {"output_tokens": 5},
            }
        ),
        json.dumps({"type": "message_stop"}),
    ]
    return "".join(f"event: message\ndata: {event}\n\n" for event in events).encode("utf-8")


def _iso_z(moment: datetime) -> str:
    """Go resetsAt 红线形状：YYYY-MM-DDTHH:mm:ss.sssZ（严格 24 字符）。"""
    moment = moment.astimezone(timezone.utc)
    return moment.strftime("%Y-%m-%dT%H:%M:%S.") + f"{moment.microsecond // 1000:03d}Z"


def build_usage_payload(now=None) -> dict:
    now = now or datetime.now(timezone.utc)
    return {
        "usage": {
            "rolling": {
                "status": "ok",
                "percent": 17,
                "resetsAt": _iso_z(now + timedelta(hours=1)),
            },
            "weekly": {
                "status": "ok",
                "percent": 42,
                "resetsAt": _iso_z(now + timedelta(days=2)),
            },
            # 三窗独立：rate-limited 仅配 100 也是合法凭证。
            "monthly": {
                "status": "rate-limited",
                "percent": 100,
                "resetsAt": _iso_z(now + timedelta(days=9)),
            },
        }
    }


# --- fixture 查找 -------------------------------------------------------------


def fixture_candidates(kind: str, model: str, variant: str) -> list:
    names = []
    if kind == "chat":
        if model:
            names += [f"chat-completions{sep}{model}.sse" for sep in (".", "-", "_")]
        names += [f"chat_{variant}.sse", "chat-completions.sse", "chat_completions.sse"]
    elif kind == "responses":
        if model:
            names += [f"responses{sep}{model}.sse" for sep in (".", "-", "_")]
        names += [f"responses_{variant}.sse", "responses.sse"]
    elif kind == "messages":
        if model:
            names.append(f"messages.{model}.sse")
        names += [f"messages_{variant}.sse", "messages.sse"]
    else:  # models / language-models / usage
        if kind == "language-models":
            names += ["language-models.json", "models.json"]
        else:
            names.append(f"{kind}.json")
    return names


def find_fixture(root: Path, channel: str, kind: str, model: str, explicit, variant="text"):
    directory = (root / channel).resolve()
    if directory.parent != root.resolve():
        return None
    # Model ids are wire data, never paths. Also reject symlink escapes for
    # explicit fixtures: the local HTTP endpoint must not serve arbitrary files.
    if model and (model != Path(model).name or model in {".", ".."}):
        model = ""
    def contained_file(name):
        candidate = (directory / name).resolve()
        return candidate if candidate.parent == directory and candidate.is_file() else None
    if explicit is not None:
        # 只接受纯 basename，防止路径穿越。
        if explicit != Path(explicit).name or explicit in {".", ".."}:
            return None
        return contained_file(explicit)
    for name in fixture_candidates(kind, model, variant):
        candidate = contained_file(name)
        if candidate is not None:
            return candidate
    return None


# --- HTTP handler -------------------------------------------------------------


class MockConfig:
    def __init__(
        self,
        fixtures_root: Path,
        chunk_bytes: int,
        chunk_interval_ms: float,
        scenarios: dict,
    ):
        self.fixtures_root = fixtures_root
        self.chunk_bytes = max(1, chunk_bytes)
        self.chunk_interval_ms = max(0.0, chunk_interval_ms)
        self.scenarios = scenarios
        self.scenarios_dir = fixtures_root / "scenarios"


ROUTES = {
    ("GET", "/models"): "handle_models",
    ("GET", "/language-models"): "handle_language_models",
    ("GET", "/usage"): "handle_usage",
    ("POST", "/chat/completions"): "handle_chat",
    ("POST", "/responses"): "handle_responses",
    ("POST", "/v1/messages"): "handle_messages",
}
# MOCK-4：/__control 场景切换不走本表（无 persona 要求），在 _dispatch 特判。
# MOCK-5：OAuth 端点同样不走本表（登录阶段无 Bearer，persona 检查前特判）。
OAUTH_ROUTES = {
    ("POST", "/oauth2/device/code"): ("device", "xai"),
    ("POST", "/oauth2/token"): ("token", "xai"),
    ("POST", "/api/oauth/device_authorization"): ("device", "kimi-code"),
    ("POST", "/api/oauth/token"): ("token", "kimi-code"),
    ("POST", "/oauth/token"): ("token", "chatgpt"),
    ("GET", "/device"): ("device_page", ""),
}


class MockProviderHandler(BaseHTTPRequestHandler):
    server_version = "pawork-mock/0.1"
    protocol_version = "HTTP/1.1"

    @staticmethod
    def _clean_variant(value):
        variant = (value or "text").strip().lower()
        return variant if re.fullmatch(r"[a-z0-9_-]+", variant) else "text"

    def do_GET(self) -> None:  # noqa: N802
        self._dispatch("GET")

    def do_POST(self) -> None:  # noqa: N802
        self._dispatch("POST")

    def _dispatch(self, method: str) -> None:
        try:
            split = urlsplit(self.path)
            query = {key: values[-1] for key, values in parse_qs(split.query).items()}
            self._scenario = None
            if split.path == "/__control":
                self.handle_control(method)
                return
            oauth_route = OAUTH_ROUTES.get((method, split.path))
            if oauth_route is not None:
                kind, channel = oauth_route
                if kind == "device_page":
                    self.handle_oauth_device_page(query)
                elif kind == "device":
                    self.handle_oauth_device(channel)
                else:
                    self.handle_oauth_token(channel)
                return
            channel = self._persona_channel()
            handler_name = ROUTES.get((method, split.path))
            if channel is None:
                self._send_json(
                    401, {"error": {"message": "missing or unknown mock persona token"}}
                )
                return
            if handler_name is None:
                self._send_json(
                    404, {"error": {"message": f"no route {method} {split.path} for {channel}"}}
                )
                return
            body = self._json_body() if method == "POST" else {}
            self._scenario = self._resolve_scenario(body)
            if self._scenario is not None and self._scenario.kind == "http_error":
                self._send_scenario_error(self._scenario)
                return
            getattr(self, handler_name)(channel, query, body)
        except (BrokenPipeError, ConnectionResetError):
            self.close_connection = True
        except Exception:
            self.log_error("handler failed:\n%s", traceback.format_exc())
            try:
                self._send_json(500, {"error": {"message": "mock handler failure"}})
            except Exception:
                self.close_connection = True

    def _persona_channel(self):
        auth = self.headers.get("Authorization", "")
        token = ""
        if auth.lower().startswith("bearer "):
            token = auth[len("bearer ") :].strip()
        if token in PERSONA_TOKENS:
            return PERSONA_TOKENS[token]
        issued = MOCK_ISSUED_TOKEN.match(token)
        if issued is not None and issued.group(1) in PERSONA_CHANNELS:
            return issued.group(1)
        api_key = (self.headers.get("x-api-key") or "").strip()
        return PERSONA_TOKENS.get(api_key)

    def _json_body(self) -> dict:
        try:
            length = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            length = 0
        raw = self.rfile.read(length) if length > 0 else b""
        try:
            value = json.loads(raw or b"{}")
        except ValueError:
            return {}
        return value if isinstance(value, dict) else {}

    def _form_body(self) -> dict:
        """读取 application/x-www-form-urlencoded 请求体为 {key: value}。"""
        try:
            length = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            length = 0
        raw = self.rfile.read(length) if length > 0 else b""
        values = parse_qs(raw.decode("utf-8", "replace"), keep_blank_values=True)
        return {key: items[-1] for key, items in values.items()}

    def _send_json(self, status: int, payload: dict) -> None:
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self._send_bytes(status, body, "application/json")

    def _send_bytes(self, status: int, body: bytes, content_type: str) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _resolve_scenario(self, body):
        """关键字（若有）优先于全局场景；未知名字逐级回落。"""
        scenarios = self.server.mock_config.scenarios
        if not scenarios:
            return None
        names = []
        if body:
            keyword = scenario_keyword(body)
            if keyword:
                names.append(keyword)
        active = self.server.current_scenario()
        if active:
            names.append(active)
        for name in names:
            scenario = scenarios.get(name.upper())
            if scenario is not None:
                return scenario
        return None

    def _send_scenario_error(self, scenario) -> None:
        body = json.dumps(
            {"error": {"message": f"mock scenario {scenario.name}"}}, ensure_ascii=False
        ).encode("utf-8")
        self.send_response(scenario.status)
        if scenario.retry_after:
            self.send_header("Retry-After", str(scenario.retry_after))
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _fixture_or_json(self, channel: str, kind: str, query: dict, fallback) -> None:
        explicit = query.get("fixture")
        fixture = find_fixture(self.server.mock_config.fixtures_root, channel, kind, "", explicit)
        if fixture is not None:
            self.log_message("fixture hit %s", fixture)
            self._send_bytes(200, fixture.read_bytes(), "application/json")
            return
        fallback()

    def _send_stream(self, payload: bytes, interval_ms=None) -> None:
        config = self.server.mock_config
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        step = config.chunk_bytes
        pacing_ms = config.chunk_interval_ms if interval_ms is None else interval_ms
        delay = pacing_ms / 1000.0
        for offset in range(0, len(payload), step):
            self.wfile.write(payload[offset : offset + step])
            self.wfile.flush()
            if delay and offset + step < len(payload):
                time.sleep(delay)

    def _stream_fixture_or(self, channel: str, kind: str, model: str, query: dict, fallback) -> None:
        scenario = getattr(self, "_scenario", None)
        if scenario is not None and scenario.kind == "sse" and kind in scenario.transports:
            name = scenario.fixture
            payload_file = self.server.mock_config.scenarios_dir / name
            # 只接受纯 basename，防止路径穿越。
            if name and name == Path(name).name and payload_file.is_file():
                self.log_message("scenario hit %s", scenario.name)
                self._send_stream(payload_file.read_bytes())
                return
        interval_override = None
        if scenario is not None and scenario.kind == "slow":
            interval_override = scenario.interval_ms
            query_interval = query.get("interval_ms")
            if query_interval is not None:
                try:
                    interval_override = min(60000.0, max(0.0, float(query_interval)))
                except ValueError:
                    pass
        explicit = query.get("fixture")
        variant = self._clean_variant(query.get("variant"))
        fixture = find_fixture(
            self.server.mock_config.fixtures_root, channel, kind, model, explicit, variant
        )
        payload = fixture.read_bytes() if fixture is not None else fallback()
        if fixture is not None:
            self.log_message("fixture hit %s", fixture)
        self._send_stream(payload, interval_override)

    # -- 端点 handler（persona 已解析） ---------------------------------------

    def handle_control(self, method: str) -> None:
        """MOCK-4/5 控制端点：GET 读当前值与清单；POST 切换 / 复位。

        scenario 语义与 MOCK-4 完全一致；body 含 "oauth" 键时额外切换
        OAuth 轮询剧本（此时 scenario 仅在显式给出时才处理，避免设剧本
        顺带复位全局场景）。
        """
        scenarios = self.server.mock_config.scenarios
        available = sorted(scenario.name for scenario in scenarios.values())
        if method == "GET":
            self._send_json(
                200,
                {
                    "scenario": self.server.current_scenario(),
                    "available": available,
                    "oauth": self.server.oauth_scripts_snapshot(),
                },
            )
            return
        if method != "POST":
            self._send_json(
                405, {"error": {"message": "/__control supports GET and POST only"}}
            )
            return
        body = self._json_body()
        if "oauth" in body and not self._apply_oauth_control(body["oauth"]):
            return
        if "oauth" not in body or "scenario" in body:
            if not self._apply_scenario_control(body.get("scenario")):
                return
        self._send_json(
            200,
            {
                "scenario": self.server.current_scenario(),
                "available": available,
                "oauth": self.server.oauth_scripts_snapshot(),
            },
        )

    def _apply_scenario_control(self, value) -> bool:
        """应用 scenario 控制值；错误响应已发送时返回 False。"""
        if value is None:
            self.server.set_scenario(None)
        elif isinstance(value, str):
            scenario = self.server.mock_config.scenarios.get(value.upper())
            if scenario is None:
                available = sorted(
                    scenario.name for scenario in self.server.mock_config.scenarios.values()
                )
                self._send_json(
                    404,
                    {
                        "error": {"message": f"unknown scenario {value!r}"},
                        "available": available,
                    },
                )
                return False
            self.server.set_scenario(scenario.name)
        else:
            self._send_json(
                400, {"error": {"message": "scenario must be a scenario name or null"}}
            )
            return False
        return True

    def _apply_oauth_control(self, value) -> bool:
        """应用 oauth 剧本控制值；错误响应已发送时返回 False。"""
        if value is None:
            self.server.reset_oauth_scripts()
            return True
        if not isinstance(value, dict):
            self._send_json(
                400,
                {
                    "error": {
                        "message": "oauth must be a channel->script mapping or null"
                    },
                    "oauth_channels": list(OAUTH_CHANNELS),
                    "oauth_scripts": sorted(OAUTH_SCRIPTS),
                },
            )
            return False
        scripts = {}
        for channel, script in value.items():
            if channel not in OAUTH_CHANNELS or not isinstance(script, str):
                self._send_json(
                    400,
                    {
                        "error": {"message": f"invalid oauth channel {channel!r}"},
                        "oauth_channels": list(OAUTH_CHANNELS),
                        "oauth_scripts": sorted(OAUTH_SCRIPTS),
                    },
                )
                return False
            if not oauth_script_allowed(channel, script):
                self._send_json(
                    400,
                    {
                        "error": {"message": f"invalid oauth script {script!r} for {channel}"},
                        "oauth_channels": list(OAUTH_CHANNELS),
                        "oauth_scripts": sorted(
                            OAUTH_TOKEN_ONLY_SCRIPTS
                            if channel == "chatgpt"
                            else OAUTH_SCRIPTS
                        ),
                    },
                )
                return False
            scripts[channel] = script
        self.server.set_oauth_scripts(scripts)
        return True

    def handle_models(self, channel: str, query: dict, body: dict) -> None:
        if channel in {"anthropic", "xai"}:
            # anthropic 静态目录无 HTTP；xai 目录在 /language-models。
            self._send_json(404, {"error": {"message": f"{channel} has no /models endpoint"}})
            return

        def fallback() -> None:
            if channel == "chatgpt":
                payload = {"models": CHATGPT_CATALOG}
            else:
                payload = {"data": [{"id": model_id} for model_id in CATALOG_DATA[channel]]}
            self._send_json(200, payload)

        self._fixture_or_json(channel, "models", query, fallback)

    def handle_language_models(self, channel: str, query: dict, body: dict) -> None:
        if channel != "xai":
            self._send_json(
                404, {"error": {"message": f"{channel} has no /language-models endpoint"}}
            )
            return

        def fallback() -> None:
            self._send_json(200, {"models": XAI_CATALOG})

        self._fixture_or_json(channel, "language-models", query, fallback)

    def handle_usage(self, channel: str, query: dict, body: dict) -> None:
        if channel != "opencode-go":
            self._send_json(404, {"error": {"message": f"{channel} has no /usage endpoint"}})
            return

        def fallback() -> None:
            self._send_json(200, build_usage_payload())

        self._fixture_or_json(channel, "usage", query, fallback)

    def handle_chat(self, channel: str, query: dict, body: dict) -> None:
        model = str(body.get("model") or "")
        if channel in {"chatgpt", "anthropic"}:
            self._send_json(
                404, {"error": {"message": f"{channel} has no /chat/completions endpoint"}}
            )
            return
        if channel == "xai" and model in XAI_RESPONSES_MODELS:
            self._send_json(
                404, {"error": {"message": f"{model} uses /responses on xai"}}
            )
            return
        if channel == "opencode-go":
            if model in OPENCODE_MESSAGES_MODELS:
                self._send_json(
                    400, {"error": {"message": f"{model} is Messages-only on opencode-go"}}
                )
                return
            if model in OPENCODE_RESPONSES_MODELS:
                self._send_json(
                    404, {"error": {"message": f"{model} uses /responses on opencode-go"}}
                )
                return
            if model not in OPENCODE_CHAT_MODELS:
                self._send_json(
                    400,
                    {"error": {"message": f"{model or '<empty>'} is not registered on opencode-go"}},
                )
                return
        if channel == "qwen-token-plan":
            if model not in QWEN_TOKEN_PLAN_MODELS_SET:
                self._send_json(
                    400,
                    {
                        "error": {
                            "message": f"{model or '<empty>'} is not registered on qwen-token-plan"
                        }
                    },
                )
                return

        def fallback() -> bytes:
            return build_chat_sse(channel, model)

        self._stream_fixture_or(channel, "chat", model, query, fallback)

    def handle_responses(self, channel: str, query: dict, body: dict) -> None:
        model = str(body.get("model") or "")
        if channel == "xai":
            if model not in XAI_RESPONSES_MODELS:
                self._send_json(
                    404,
                    {"error": {"message": f"{model or '<empty>'} uses /chat/completions on xai"}},
                )
                return
        elif channel == "opencode-go":
            if model in OPENCODE_MESSAGES_MODELS:
                self._send_json(
                    400, {"error": {"message": f"{model} is Messages-only on opencode-go"}}
                )
                return
            if model not in OPENCODE_RESPONSES_MODELS:
                self._send_json(
                    404,
                    {
                        "error": {
                            "message": f"{model or '<empty>'} uses /chat/completions on opencode-go"
                        }
                    },
                )
                return
        elif channel != "chatgpt":
            self._send_json(
                404, {"error": {"message": f"{channel} has no /responses endpoint"}}
            )
            return

        def fallback() -> bytes:
            return build_responses_sse(channel, model)

        self._stream_fixture_or(channel, "responses", model, query, fallback)

    def handle_messages(self, channel: str, query: dict, body: dict) -> None:
        if channel != "anthropic":
            self._send_json(
                404, {"error": {"message": f"{channel} has no /v1/messages endpoint"}}
            )
            return
        model = str(body.get("model") or "")

        def fallback() -> bytes:
            return build_messages_sse(channel, model)

        self._stream_fixture_or(channel, "messages", model, query, fallback)

    # -- OAuth 端点（MOCK-5，§2.1；persona 之前路由，无 Bearer） ---------------

    def handle_oauth_device_page(self, query: dict) -> None:
        user_code = html.escape(str(query.get("user_code") or "----"))
        page = (
            '<!doctype html><html><head><meta charset="utf-8">'
            "<title>Pawork mock device verification</title></head>"
            '<body style="font-family: ui-monospace, monospace; padding: 2rem">'
            "<h1>Pawork mock device verification</h1>"
            f"<p>user_code: <strong>{user_code}</strong></p>"
            "<p>这是本地 mock 的人工验证页；剧本由 /__control 控制。</p>"
            "</body></html>"
        )
        self._send_bytes(200, page.encode("utf-8"), "text/html; charset=utf-8")

    def handle_oauth_device(self, channel: str) -> None:
        """Device Flow 设备码端点（xai / kimi-code）。"""
        form = self._form_body()
        if not form.get("client_id"):
            self._send_json(
                200, {"error": "invalid_request", "error_description": "missing client_id"}
            )
            return
        device_code, user_code = self.server.oauth_new_device(channel)
        host = self.headers.get("Host") or "127.0.0.1"
        base = f"http://{host}"
        scope = form.get("scope") or ""
        self._send_json(
            200,
            {
                "device_code": device_code,
                "user_code": user_code,
                "verification_uri": f"{base}/device",
                "verification_uri_complete": f"{base}/device?user_code={user_code}",
                "expires_in": 900,
                "interval": 1,
                **({"scope": scope} if scope else {}),
            },
        )

    def handle_oauth_token(self, channel: str) -> None:
        """token 端点：device 轮询 / refresh / PKCE authorization_code。

        成功与错误响应均为 JSON；出现 error 字段即失败（HTTP 200 也算），
        形状对齐 crates/auth/src/oauth.rs exchange_token 的解析。
        """
        form = self._form_body()
        grant = form.get("grant_type") or ""
        script = self.server.oauth_script(channel)
        if grant == DEVICE_GRANT:
            if channel == "chatgpt":
                self._send_json(
                    200,
                    {
                        "error": "unsupported_grant_type",
                        "error_description": "chatgpt uses pkce authorization_code",
                    },
                )
                return
            device_code = form.get("device_code") or ""
            if not self.server.oauth_device_exists(channel, device_code):
                self._send_json(
                    200,
                    {"error": "invalid_grant", "error_description": "unknown mock device_code"},
                )
                return
            poll = self.server.oauth_next_poll(device_code)
            error = device_poll_error(script, poll)
            if error is not None:
                self._send_json(200, {"error": error})
                return
            self._send_json(200, self._oauth_token_payload(channel, form))
            return
        if grant == "refresh_token":
            if not form.get("refresh_token"):
                self._send_json(
                    200,
                    {"error": "invalid_request", "error_description": "missing refresh_token"},
                )
                return
            if script == "invalid_grant":
                self._send_json(
                    200,
                    {"error": "invalid_grant", "error_description": "mock script"},
                )
                return
            rotate = script != "refresh_no_rotation"
            # refresh 不换发 id_token：chatgpt 的 account id 保留客户端 meta
            # 原值，不随 refresh 轮转（对齐 §2.6 缺省保留语义）。
            self._send_json(
                200,
                self._oauth_token_payload(
                    channel, form, rotate=rotate, include_id_token=False
                ),
            )
            return
        if grant == "authorization_code":
            if channel != "chatgpt":
                self._send_json(
                    200,
                    {
                        "error": "unsupported_grant_type",
                        "error_description": f"{channel} uses device flow",
                    },
                )
                return
            if script == "invalid_grant":
                self._send_json(
                    200,
                    {"error": "invalid_grant", "error_description": "mock script"},
                )
                return
            self._send_json(200, self._oauth_token_payload(channel, form))
            return
        self._send_json(
            200,
            {
                "error": "unsupported_grant_type",
                "error_description": f"unsupported grant_type {grant!r}",
            },
        )

    def _oauth_token_payload(
        self, channel: str, form: dict, rotate: bool = True, include_id_token: bool = True
    ) -> dict:
        """成功 token 响应；rotate=False 时缺 refresh_token 与 expires_in，
        对齐「refresh 响应缺省 → 客户端保留旧值」的验证形状。"""
        sequence = self.server.oauth_next_sequence()
        payload = {
            "access_token": f"mock-{channel}-access-{sequence}",
            "token_type": "Bearer",
        }
        if rotate:
            payload["refresh_token"] = f"mock-{channel}-refresh-{sequence}"
            payload["expires_in"] = 3600
        scope = form.get("scope")
        if scope:
            payload["scope"] = scope
        if channel == "chatgpt" and include_id_token:
            payload["id_token"] = build_mock_id_token(sequence)
        return payload


class MockProviderServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address, handler, config: MockConfig):
        super().__init__(address, handler)
        self.mock_config = config
        self._scenario_lock = threading.Lock()
        self._scenario_name = None
        self._oauth_lock = threading.Lock()
        self._oauth_scripts = {}
        self._oauth_devices = {}
        self._oauth_sequence = 0

    def current_scenario(self):
        with self._scenario_lock:
            return self._scenario_name

    def set_scenario(self, name):
        with self._scenario_lock:
            self._scenario_name = name

    # -- MOCK-5 OAuth 剧本状态（线程安全） -----------------------------------

    def oauth_script(self, channel: str) -> str:
        with self._oauth_lock:
            return self._oauth_scripts.get(channel, DEFAULT_OAUTH_SCRIPT)

    def oauth_scripts_snapshot(self) -> dict:
        with self._oauth_lock:
            return {
                channel: self._oauth_scripts.get(channel, DEFAULT_OAUTH_SCRIPT)
                for channel in OAUTH_CHANNELS
            }

    def set_oauth_scripts(self, mapping: dict) -> None:
        with self._oauth_lock:
            self._oauth_scripts.update(mapping)

    def reset_oauth_scripts(self) -> None:
        """复位剧本并清空 device 轮询状态。

        复位（{"oauth":null}）后旧 device_code 变为 unknown（invalid_grant），
        换剧本前必须重新走 device/code，避免旧 device_code 的 polls 计数
        串入新剧本。
        """
        with self._oauth_lock:
            self._oauth_scripts.clear()
            self._oauth_devices.clear()

    def oauth_new_device(self, channel: str):
        """签发新 device_code / user_code；state 上限 256 条防无界增长。"""
        with self._oauth_lock:
            self._oauth_sequence += 1
            sequence = self._oauth_sequence
            raw = secrets.token_hex(4).upper()
            device_code = f"mock-device-{sequence}-{secrets.token_hex(6)}"
            self._oauth_devices[device_code] = {"channel": channel, "polls": 0}
            while len(self._oauth_devices) > 256:
                oldest = next(iter(self._oauth_devices))
                del self._oauth_devices[oldest]
            return device_code, f"{raw[:4]}-{raw[4:]}"

    def oauth_device_exists(self, channel: str, device_code: str) -> bool:
        with self._oauth_lock:
            entry = self._oauth_devices.get(device_code)
            return entry is not None and entry["channel"] == channel

    def oauth_next_poll(self, device_code: str) -> int:
        with self._oauth_lock:
            entry = self._oauth_devices[device_code]
            entry["polls"] += 1
            return entry["polls"]

    def oauth_next_sequence(self) -> int:
        with self._oauth_lock:
            self._oauth_sequence += 1
            return self._oauth_sequence


def main(argv=None) -> int:
    default_root = Path(__file__).resolve().parents[2] / "fixtures" / "mock"
    parser = argparse.ArgumentParser(description="Pawork provider mock server (MOCK-3)")
    parser.add_argument("--host", default=os.environ.get("MOCK_HOST", "127.0.0.1"))
    parser.add_argument("--port", type=int, default=int(os.environ.get("MOCK_PORT", "8787")))
    parser.add_argument(
        "--fixtures-root",
        default=os.environ.get("MOCK_FIXTURES_ROOT", str(default_root)),
    )
    parser.add_argument(
        "--chunk-interval-ms",
        type=float,
        default=float(os.environ.get("MOCK_CHUNK_INTERVAL_MS", "0")),
    )
    parser.add_argument(
        "--chunk-bytes",
        type=int,
        default=int(os.environ.get("MOCK_CHUNK_BYTES", "1024")),
    )
    args = parser.parse_args(argv)

    fixtures_root = Path(args.fixtures_root).resolve()
    config = MockConfig(
        fixtures_root=fixtures_root,
        chunk_bytes=args.chunk_bytes,
        chunk_interval_ms=args.chunk_interval_ms,
        scenarios=load_scenarios(fixtures_root),
    )
    server = MockProviderServer((args.host, args.port), MockProviderHandler, config)
    host, port = server.server_address[:2]
    print(
        f"pawork mock server listening on http://{host}:{port} "
        f"(fixtures: {config.fixtures_root}, chunk: {config.chunk_bytes}B/"
        f"{config.chunk_interval_ms}ms, scenarios: {len(config.scenarios)})",
        flush=True,
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
