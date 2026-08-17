# `--json` → Headless 正式协议：映射表与迁移说明

> S10 10a 波 A（2026-08-17）留档对照表；S10 **收口**（2026-08-17）已兑现计划内唯一一次 `--json` breaking：`run` / `chat --prompt --json` 的 stdout 从裸 `AgentEventEnvelope` 升到 `HeadlessResponse`（`type=event|response|error`），去掉 `unstable`。单向 `--json` **没有** hello；`pawork headless --json-stdio` 必须先 hello。
>
> 可测对照表：`pawork_protocol::headless::json_mapping::JSON_TO_HEADLESS_EVENT_MAP`。
> TypeScript 检入物：[`schemas/`](../schemas/)（`core-api` / `gui-protocol` / `headless-json`）。

---

## 1. 三层不要混

| 层 | 类型 | 谁用 | 是否本波改 |
| --- | --- | --- | --- |
| 磁盘 / 重放 | `AgentEventEnvelope`（`schema_version = 1`，session 内 `sequence`） | session-store | 否。磁盘契约不动 |
| 应用 / 多订阅者 | `AppEventEnvelope`（`api_version` + `global_sequence` + `stream`） | GUI 帧、`--json` / headless 的 `event` | 否。S7 已零裁剪 |
| 传输包装 | GUI：`ClientFrame`/`ServerFrame`（u32 LE 长度前缀）；Headless：`HeadlessRequest`/`HeadlessResponse`（JSONL） | Desktop vs SDK / 单向 `--json` | 收口已切 CLI 流式 stdout；typegen / golden 在 10a |

S10 收口的 breaking：**流式 `--json` stdout 从层 1 升到层 2+3**。解析器若 `JSON.parse` 后直接读 `payload` / `sequence`，会在收口后失败。

---

## 2. 现行输出 vs 正式目标

### 2.1 收口后的单向 `--json`（`run` / `chat --prompt`）

`pawork run --json` / `pawork chat --prompt --json`：stdout 每行一个 `HeadlessResponse`，**没有** hello。顶层 `type` ∈ `event` | `response` | `error`。不要按收口前的裸 `AgentEventEnvelope` 解析（那种行带顶层 `schema_version` / `sequence`，没有 `type`）。

```json
{"type":"event","envelope":{"api_version":{"major":1,"minor":1},"instance_id":"…","event_id":"…","global_sequence":1,"stream":{"type":"session","id":"…"},"stream_sequence":1,"timestamp":…,"source":{"type":"core"},"payload":{"type":"assistant_delta","data":{…}}}}
```

纪律：stdout 只承载 JSONL；文本、进度、日志走 stderr。`--json` 或非 TTY 下审批 fail-closed（DenyAll）。

`sessions` / `auth` / `models` / `diff` / `mcp` / `import` 的 `--json` 仍是各自快照 JSON，**不是** 协议帧，也不是 headless `query` 响应。

### 2.2 双向 headless（`pawork headless --json-stdio`）

双向 stdin/stdout JSONL。帧定义在 `pawork_protocol::headless`（V1 `headless-json` 整组迁入）。

请求：`hello` / `command` / `query` / `compat_import` / `compat_history`  
响应：`hello_ack` / `response` / `event` / `compat_*_result` / `error`

`command` / `query` / `event` **直接嵌** `App*Envelope`，不另造 RPC method 名空间。

```json
{"type":"hello","client_name":"sdk","client_version":"0.1.0","supported_api_versions":[{"major":1,"minor":1}],"capabilities":["sessions","runs","streaming"]}
{"type":"hello_ack","instance_id":"…","negotiated":{"major":1,"minor":1},"granted":["sessions","runs","streaming"]}
{"type":"event","envelope":{"api_version":{"major":1,"minor":1},"instance_id":"…","event_id":"…","global_sequence":1,"stream":{"type":"run","id":"…"},"stream_sequence":1,"timestamp":…,"source":{"type":"core"},"payload":{"type":"assistant_delta","data":{…}}}}
```

握手强制：`hello_ack` 之前的 `command`/`query` → `error.kind = not_handshaked`。单帧上限 4MiB。未知 `type` → `unknown_request_type`。

### 2.3 V1 `--json`（历史，仅对照）

V1 `run --json` 打的是**裸** `AppEventEnvelope`（无 `{type:event}` 包装，无 hello）。与现行 V2、与正式 headless 都不同。老 V1 脚本不能假设「已经接近正式协议」。

---

## 3. 信封字段对照（流式 `--json` → `event.envelope`）

| 磁盘 / 收口前裸信封 | 收口后 `HeadlessResponse::event.envelope` | 说明 |
| --- | --- | --- |
| `schema_version` | **不要出现** | 磁盘契约，保持 1 |
| `event_id` | `event_id` | 可对齐 |
| `session_id` / `run_id` | `stream: {type: session\|run, id}`，payload 内也可带 | 线上序改用 `global_sequence` |
| `sequence`（session 内从 1 递增） | `stream_sequence` + `global_sequence` | **breaking**：按 session 从 1 断言会偏 |
| `timestamp` | `timestamp` | unix ms |
| `parent_event_id` | 无 | 留在磁盘；headless 不回放树边 |
| （无） | `api_version` / `instance_id` / `source` | 新必填 |
| `payload` = 整枚 `AgentEvent` | `payload` = 更粗的 `AppEvent` | 见 §4；细事件默认不上 headless |

---

## 4. 事件 tag 对照

机器可读源：`JSON_TO_HEADLESS_EVENT_MAP`。`app_event_tag = None` 表示收口后该条 **不再出现在 `--json` / headless 事件流**（仍在磁盘）。

| 现行 `payload.type` | 正式 `envelope.payload.type` | 备注 |
| --- | --- | --- |
| `run_started` | `run_changed` | `RunState` 起步 |
| `run_completed` / `run_cancelled` / `run_failed` | `run_changed` | 终态；细节在 `state` |
| `assistant_text_delta` | `assistant_delta` | 字段名不同：`data.delta` 仍在，另有 `run_id`/`message_id` |
| `assistant_thinking_delta` | `thinking_delta` | 同上 |
| `tool_call_started` | `tool_started` | 无 arguments delta |
| `tool_output_delta` | `tool_output` | |
| `tool_execution_completed` | `tool_completed` | |
| `tool_approval_requested` | `tool_approval_required` | 远程决议走 `AppCommand::ToolApprove`（收口才接，不再 DenyAll） |
| `diagnostic` | `diagnostic` | tag 相同；字段从 `{code,details}` 变为 `{level,code,message}` |
| `context_prepared` | — | domain-only |
| `provider_request_started` | — | domain-only |
| `usage_updated` | — | 不是 `quota_changed`（控制面） |
| `tool_call_arguments_delta` | — | |
| `tool_approval_responded` | — | 由后续 `tool_*` / 命令回执表达 |
| `tool_execution_started` | — | |
| `message_committed` | — | 时间线投影，不走 Event 帧 |
| `provider_transcript_continued` / `server_tool` / `transcript_envelope` | — | |
| `compaction_*` / `checkpoint_*` | — | |
| `plan` / `goal` / `task` / `automation` / `monitor` / `memory` / `review` | — | S11 域事件；`AppEvent` 无 1:1 |

只出现在 headless/GUI、不会从磁盘细事件长出来的 `AppEvent`（`core_ready`、`workspace_changed`、`session_changed`、`quota_*`、`team`、`gui_client_*` 等）保持原样，不在本表。

---

## 5. stdio 纪律（收口后脚本必须遵守）

1. 按 `\n` 切行；允许剥 `\r`。先读顶层 `type`，未知 type 当协议错误，不要当 domain envelope。
2. 流式 `--json`：每行必有 `type`（`event` / `error` / 收口后可能的 `hello_ack` 不适用于单向 `--json`）。
3. `headless --json-stdio`：客户端第一行必须是 `hello`；Host 先回 `hello_ack` 再接受 `command`/`query`。
4. 日志、进度、URL、审批提示 **永不**进 stdout。
5. GUI / Desktop **禁止** fallback 到 `--json`（已走正式帧）。
6. 不要把 JSON-RPC（`id`/`method`/`params`）或 HTTP/SSE 当成 Pawork 线上格式。

单向 `--json`（脚本）与双向 `headless`（SDK）共用 **同一套** `HeadlessResponse` 词表；差别只是 `--json` 没有 stdin 命令、没有 hello。这是 Codex `exec --json` vs `app-server` 的 Pawork 版，不是两套协议。

---

## 6. 老脚本怎么改（收口波执行时）

1. 解析：`line → JSON → type`。`type == "event"` 时读 `envelope.payload`；`type == "error"` 时读 `kind`/`message`。
2. 把 `payload.type == "assistant_text_delta"` 改成 `envelope.payload.type == "assistant_delta"`（见 §4）。
3. 序号改认 `envelope.global_sequence`，不要用顶层 `sequence`。
4. 需要发命令/审批：改走 `pawork headless --json-stdio`，不要在 `--json` 的 stdin 上发明帧。
5. TypeScript 类型以 [`schemas/headless-json/`](../schemas/headless-json/) 与 [`schemas/core-api/`](../schemas/core-api/) 为准；`versions.d.ts` 的 `API_VERSION` 为 `{major:1,minor:1}`，`SUPPORTED_API_VERSIONS` 含 `1.0` 与 `1.1`。

协议版本与 crate semver 解耦：对照表在 `pawork_protocol::PROTOCOL_CRATE_COMPATIBILITY`，**不进入**握手 JSON。

---

## 7. 本波已落地 / 留给后续

| 项 | 状态 |
| --- | --- |
| headless 帧 + 翻译 + stdio 循环（crate 内） | ✅ `pawork-protocol::headless` |
| `--json` → AppEvent tag 对照表 | ✅ `json_mapping` + 本文 |
| typegen `.d.ts` 检入 | ✅ `schemas/` |
| GUI 帧 golden 补齐 | ✅ |
| `SUPPORTED_API_VERSIONS` 含 1.0 与 1.1 | ✅（ADR-036） |
| CLI `--json` 切输出 / 去 unstable | ✅ 收口：`run` / `chat --prompt --json` 打 `HeadlessResponse`；其它子命令仍是 CLI 便利 JSON |
| `pawork headless --json-stdio` 子命令 | ✅ 收口；握手强制；能力门 Sessions/Runs/Streaming/Compat* |
| SDK 消费 | ✅ 10a 波 B + 收口 `spawn_e2e` 不再 skip |
| Event Hub 扇出 | ✅ 10a 波 B；`--json` / `watch --json` 消费 Hub 事件 |
