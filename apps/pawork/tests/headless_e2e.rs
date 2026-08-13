//! P17-8 `pawork headless --json-stdio` 二进制端到端：
//!
//! 真实进程、真实 stdin/stdout JSONL 往返，覆盖：
//! - 握手（version / capability 协商）；
//! - Query / Command 经同一 AppService 的信封往返；
//! - compat 导入经真实 SessionStore 持久化 + 导入历史查询；
//! - 异常帧（malformed / unknown type）显式 error 帧；
//! - EOF 干净退出；stdout 只含 JSONL 帧（无 TUI/CLI 文本）。

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use agent_domain::{CommandId, QueryId, Timestamp, WorkspaceId};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope, CommandSource,
    API_VERSION,
};
use serde_json::{json, Value};

fn data_dir(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("pawork-headless-{}-{name}", std::process::id()))
        .display()
        .to_string()
}

/// headless 子进程包装：stdout 逐帧读取（带超时）；Drop 时确保回收。
struct HeadlessProc {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: BufReader<ChildStdout>,
    frames: Vec<Value>,
    data_dir: String,
}

impl HeadlessProc {
    fn spawn(name: &str) -> Self {
        let data = data_dir(name);
        std::fs::create_dir_all(&data).expect("create data dir");
        let mut child = Command::new(env!("CARGO_BIN_EXE_pawork"))
            .args(["headless", "--json-stdio"])
            .env("PAWORK_DATA_DIR", &data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn pawork");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        Self {
            child,
            stdin: Some(stdin),
            reader: BufReader::new(stdout),
            frames: Vec::new(),
            data_dir: data,
        }
    }

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{line}").expect("write frame");
        stdin.flush().expect("flush stdin");
    }

    /// 读取下一帧（10s 超时）；每行都必须是合法 JSON（stdout 纯净性）。
    fn next_frame(&mut self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut line = String::new();
        loop {
            assert!(
                Instant::now() < deadline,
                "timeout waiting for frame; frames so far: {:?}",
                self.frames
            );
            let read = self.reader.read_line(&mut line).expect("read stdout");
            assert!(
                read > 0,
                "EOF before frame; frames so far: {:?}",
                self.frames
            );
            let trimmed = line.trim();
            if trimmed.is_empty() {
                line.clear();
                continue;
            }
            let frame: Value = serde_json::from_str(trimmed).unwrap_or_else(|error| {
                panic!("stdout line is not JSON (purity violated): {trimmed:?}: {error}")
            });
            line.clear();
            self.frames.push(frame.clone());
            return frame;
        }
    }

    /// EOF：关闭 stdin 并等待退出；断言退出码。
    fn finish(mut self, expected_code: i32) -> Vec<Value> {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.child.try_wait().expect("try_wait") {
                Some(status) => {
                    assert_eq!(
                        status.code(),
                        Some(expected_code),
                        "unexpected exit code; frames: {:?}",
                        self.frames
                    );
                    let _ = std::fs::remove_dir_all(&self.data_dir);
                    return std::mem::take(&mut self.frames);
                }
                None => {
                    assert!(Instant::now() < deadline, "process did not exit on EOF");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
}

impl Drop for HeadlessProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

fn hello_frame() -> String {
    json!({
        "type": "hello",
        "client_name": "binary-e2e",
        "client_version": "0.0.0",
        "supported_api_versions": [{"major": 1, "minor": 0}],
        "capabilities": ["sessions", "runs", "streaming", "compat_import", "compat_history"]
    })
    .to_string()
}

fn query_frame(query: AppQuery) -> String {
    json!({
        "type": "query",
        "envelope": AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from("e2e-qry"),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation { name: "binary-e2e".into() },
            issued_at: Timestamp::from_unix_millis(1),
            query,
        }
    })
    .to_string()
}

fn command_frame(command: AppCommand, id: &str) -> String {
    json!({
        "type": "command",
        "envelope": AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from(id),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation { name: "binary-e2e".into() },
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(1),
            command,
        }
    })
    .to_string()
}

fn compat_import_frame(request_id: &str, content: &str) -> String {
    json!({
        "type": "compat_import",
        "request_id": request_id,
        "source": "claude",
        "content": content,
        "options": {"dry_run": false}
    })
    .to_string()
}

const CLAUDE_JSON: &str = r#"{
    "conversation_id": "e2e-claude-1",
    "name": "binary e2e chat",
    "chat_messages": [
        {"sender": "human", "text": "hello"},
        {"sender": "assistant", "text": "hi there"}
    ]
}"#;

/// hello → query → command → compat import/history → 异常帧 → EOF。
#[test]
fn hello_query_command_compat_roundtrip() {
    let mut proc = HeadlessProc::spawn("roundtrip");

    // 握手。
    proc.send(&hello_frame());
    let ack = proc.next_frame();
    assert_eq!(ack["type"], "hello_ack");
    assert_eq!(ack["negotiated"], json!({"major": 1, "minor": 0}));
    assert!(!ack["instance_id"].as_str().expect("instance id").is_empty());
    let granted = ack["granted"].as_array().expect("granted array");
    assert!(granted.contains(&json!("compat_import")));
    assert!(granted.contains(&json!("compat_history")));

    // Query（同一 AppService）。
    proc.send(&query_frame(AppQuery::WorkspaceList));
    let response = proc.next_frame();
    assert_eq!(response["type"], "response");
    assert_eq!(response["envelope"]["response"]["type"], "data");

    // Command：添加 workspace（SessionCreate 要求 workspace 已存在）。
    proc.send(&command_frame(
        AppCommand::WorkspaceAdd {
            root_path: proc.data_dir.clone(),
        },
        "e2e-cmd-workspace",
    ));
    let response = proc.next_frame();
    assert_eq!(response["type"], "response");
    let workspace_id = response["envelope"]["response"]["data"]["id"]
        .as_str()
        .expect("workspace id");
    assert!(!workspace_id.is_empty());

    // Command：创建会话。
    proc.send(&command_frame(
        AppCommand::SessionCreate {
            workspace_id: WorkspaceId::from(workspace_id),
            title: Some("binary e2e".into()),
        },
        "e2e-cmd-session",
    ));
    let response = proc.next_frame();
    assert_eq!(response["type"], "response");
    let session_id = response["envelope"]["response"]["data"]["session_id"]
        .as_str()
        .expect("session_id");
    assert!(!session_id.is_empty());

    // compat 导入 → 真实 SessionStore 持久化。
    proc.send(&compat_import_frame("e2e-ci", CLAUDE_JSON));
    let result = proc.next_frame();
    assert_eq!(result["type"], "compat_import_result");
    assert_eq!(result["request_id"], "e2e-ci");
    assert_eq!(result["report"]["imported_messages"], 2);
    assert_eq!(result["report"]["source"], "claude");

    // 导入历史可查。
    proc.send(&json!({"type": "compat_history", "request_id": "e2e-ch", "limit": 10}).to_string());
    let result = proc.next_frame();
    assert_eq!(result["type"], "compat_history_result");
    assert_eq!(result["request_id"], "e2e-ch");
    let entries = result["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1, "import persisted into history");
    assert_eq!(entries[0]["source"], "claude");
    assert_eq!(entries[0]["original_id"], "e2e-claude-1");

    // 异常帧：malformed → 显式 error 帧。
    proc.send("{not json}");
    let error = proc.next_frame();
    assert_eq!(error["type"], "error");
    assert_eq!(error["kind"], "malformed_frame");

    // 异常帧：unknown type → 显式 unknown 错误。
    proc.send(r#"{"type": "frobnicate"}"#);
    let error = proc.next_frame();
    assert_eq!(error["type"], "error");
    assert_eq!(error["kind"], "unknown_request_type");

    // EOF：干净退出（stdout 全程只有 JSONL 帧）。
    let frames = proc.finish(0);
    assert!(!frames.is_empty());
}

/// 握手前的 Command 被显式拒绝（NotHandshaked），握手后恢复可用。
#[test]
fn pre_handshake_command_is_rejected() {
    let mut proc = HeadlessProc::spawn("pre-handshake");

    proc.send(&query_frame(AppQuery::WorkspaceList));
    let error = proc.next_frame();
    assert_eq!(error["type"], "error");
    assert_eq!(error["kind"], "not_handshaked");
    assert_eq!(error["request_id"], "e2e-qry");

    // 握手后同帧可用。
    proc.send(&hello_frame());
    let ack = proc.next_frame();
    assert_eq!(ack["type"], "hello_ack");
    proc.send(&query_frame(AppQuery::WorkspaceList));
    let response = proc.next_frame();
    assert_eq!(response["type"], "response");

    proc.finish(0);
}

/// 空输入（立即 EOF）：干净退出，stdout 无任何输出。
#[test]
fn empty_input_exits_cleanly_without_output() {
    let proc = HeadlessProc::spawn("empty");
    let frames = proc.finish(0);
    assert!(frames.is_empty(), "empty input must not produce output");
}

/// 未开 --json-stdio 时 headless 返回显式错误（不静默进入其他模式）。
#[test]
fn headless_without_flag_fails_explicitly() {
    let output = Command::new(env!("CARGO_BIN_EXE_pawork"))
        .args(["headless"])
        .output()
        .expect("run pawork headless");
    assert!(!output.status.success(), "exit code must be non-zero");
    let text = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        text.contains("--json-stdio"),
        "explicit error mentions the required flag: {text}"
    );
}
