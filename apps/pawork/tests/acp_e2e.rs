//! P17-7 `pawork acp serve` 二进制端到端（进程级）。
//!
//! 真实二进制、真实 stdin/stdout JSON-RPC 帧往返：握手 → session/new
//! （进程把 cwd 自登记为 workspace）→ prompt 结构化错误帧（无凭据 Provider，
//! 证明协议隔离）→ 非法 JSON 回 -32700 Parse error 帧（stdout 纯净）→ EOF
//! 干净退出。Session Registry 复用本实例数据目录的 SQLite `session.db`。

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct AcpProc {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: BufReader<ChildStdout>,
    frames: Vec<Value>,
    data_dir: PathBuf,
    remove_on_drop: bool,
}

impl AcpProc {
    fn spawn(name: &str) -> Self {
        let data = std::env::temp_dir().join(format!("pawork-acp-{}-{name}", std::process::id()));
        Self::spawn_in(data, name, true)
    }

    /// 与既有进程共享同一数据目录（跨进程 resume 复用同一 SQLite
    /// `session.db`；目录清理交由调用方在全部进程结束后进行）。
    fn spawn_shared(data: PathBuf, name: &str) -> Self {
        Self::spawn_in(data, name, false)
    }

    fn spawn_in(data: PathBuf, _name: &str, remove_on_drop: bool) -> Self {
        std::fs::create_dir_all(&data).expect("create data dir");
        let mut child = Command::new(env!("CARGO_BIN_EXE_pawork"))
            .args(["acp", "serve"])
            .env("PAWORK_DATA_DIR", &data)
            .current_dir(&data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn pawork acp serve");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        Self {
            child,
            stdin: Some(stdin),
            reader: BufReader::new(stdout),
            frames: Vec::new(),
            data_dir: data,
            remove_on_drop,
        }
    }

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{line}").expect("write frame");
        stdin.flush().expect("flush stdin");
    }

    /// 读取下一帧（逐行；每行都必须是合法 JSON——stdout 纯净性断言）。
    fn next_frame(&mut self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
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

    /// EOF：关闭 stdin 并等待退出；断言退出码并返回全部帧。
    fn finish(mut self, expected_code: i32) -> Vec<Value> {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match self.child.try_wait().expect("try_wait") {
                Some(status) => {
                    assert_eq!(
                        status.code(),
                        Some(expected_code),
                        "unexpected exit code; frames: {:?}",
                        self.frames
                    );
                    if self.remove_on_drop {
                        let _ = std::fs::remove_dir_all(&self.data_dir);
                    }
                    return std::mem::take(&mut self.frames);
                }
                None => {
                    assert!(
                        Instant::now() < deadline,
                        "process did not exit on EOF; frames: {:?}",
                        self.frames
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
}

impl Drop for AcpProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.remove_on_drop {
            let _ = std::fs::remove_dir_all(&self.data_dir);
        }
    }
}

fn request(id: u64, method: &str, params: Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string()
}

/// 握手 → session/new → prompt 结构化错误 → Parse error 帧 → EOF 干净退出。
#[test]
fn acp_serve_stdio_roundtrip() {
    let mut proc = AcpProc::spawn("roundtrip");
    let cwd = proc.data_dir.to_string_lossy().into_owned();

    // 握手。
    proc.send(&request(
        1,
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {
                "name": "binary-e2e",
                "title": "ACP Binary E2E",
                "version": "1.0.0"
            }
        }),
    ));
    let frame = proc.next_frame();
    assert_eq!(frame["id"], json!(1));
    assert_eq!(frame["result"]["protocolVersion"], json!(1));
    assert_eq!(frame["result"]["agentInfo"]["name"], json!("pawork-acp"));

    // session/new（cwd 由 acp serve 启动时自登记）。
    proc.send(&request(
        2,
        "session/new",
        json!({ "cwd": cwd, "mcpServers": [] }),
    ));
    let frame = proc.next_frame();
    assert_eq!(frame["id"], json!(2));
    let session_id = frame["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    // session/prompt：真实进程无已认证 Provider → 结构化 JSON-RPC 错误帧
    // （协议仍干净，stdout 不被文本污染）。
    proc.send(&request(
        3,
        "session/prompt",
        json!({
            "sessionId": session_id,
            "prompt": [ { "type": "text", "text": "hello from e2e" } ],
        }),
    ));
    let frame = proc.next_frame();
    assert_eq!(frame["id"], json!(3));
    assert!(
        frame.get("error").is_some(),
        "无凭据 Provider 应返回错误帧，got: {frame}"
    );
    assert!(frame["error"]["code"].as_i64().unwrap_or(0) != 0);

    // 协议错误：非法 JSON → -32700 Parse error 帧；stdout 纯净由 next_frame 保证。
    proc.send("this is not json at all\n");
    let frame = proc.next_frame();
    assert_eq!(frame["id"], Value::Null);
    assert_eq!(frame["error"]["code"], json!(-32700));

    // EOF：stdin 关闭后干净退出（退出码 0）。
    let frames = proc.finish(0);
    assert!(frames.len() >= 4, "至少收到握手/会话/错误/解析错误四类帧");
}

/// 真实 SQLite 跨 host/进程 resume：进程 A 创建并 close 会话（记录落盘
/// `session.db`），退出后进程 B 用同一数据目录以 `session/resume` 重新
/// claim 同一会话，随后 prompt 走到真实 dispatch（无已认证 Provider →
/// 结构化 JSON-RPC 错误帧），证明会话记录跨进程持久化且 resume 后在进程 B
/// 内已 attach、可执行。
#[test]
fn acp_serve_resume_across_processes_uses_sqlite_session_db() {
    let data = std::env::temp_dir().join(format!("pawork-acp-resume-{}", std::process::id()));
    let cwd = data.to_string_lossy().into_owned();
    let initialize = request(
        1,
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {
                "name": "binary-resume-a",
                "title": "ACP Binary Resume A",
                "version": "1.0.0"
            }
        }),
    );

    // 进程 A：握手 → session/new → session/close（Disconnected 记录落盘）→ EOF。
    let mut proc_a = AcpProc::spawn_shared(data.clone(), "resume-a");
    proc_a.send(&initialize);
    let frame = proc_a.next_frame();
    assert_eq!(frame["id"], json!(1));
    assert_eq!(frame["result"]["protocolVersion"], json!(1));
    proc_a.send(&request(
        2,
        "session/new",
        json!({ "cwd": cwd, "mcpServers": [] }),
    ));
    let frame = proc_a.next_frame();
    assert_eq!(frame["id"], json!(2));
    let session_id = frame["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    proc_a.send(&request(
        3,
        "session/close",
        json!({ "sessionId": session_id }),
    ));
    let frame = proc_a.next_frame();
    assert_eq!(frame["id"], json!(3));
    assert!(frame.get("result").is_some(), "close 应成功，got: {frame}");
    proc_a.finish(0);

    // 进程 B（同一 data dir / SQLite session.db）：握手 → resume（省略
    // mcpServers/additionalDirectories，按官方 builder 缺省）→ prompt。
    let mut proc_b = AcpProc::spawn_shared(data.clone(), "resume-b");
    proc_b.send(&request(
        1,
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {
                "name": "binary-resume-b",
                "title": "ACP Binary Resume B",
                "version": "1.0.0"
            }
        }),
    ));
    let frame = proc_b.next_frame();
    assert_eq!(frame["id"], json!(1));
    proc_b.send(&request(
        2,
        "session/resume",
        json!({ "sessionId": session_id, "cwd": cwd }),
    ));
    let frame = proc_b.next_frame();
    assert_eq!(frame["id"], json!(2));
    assert_eq!(
        frame["result"],
        json!({}),
        "跨进程 resume 应成功，got: {frame}"
    );
    proc_b.send(&request(
        3,
        "session/prompt",
        json!({
            "sessionId": session_id,
            "prompt": [ { "type": "text", "text": "continue after resume" } ],
        }),
    ));
    let frame = proc_b.next_frame();
    assert_eq!(frame["id"], json!(3));
    assert!(
        frame.get("error").is_some(),
        "resume 后 prompt 应走到真实 dispatch（无凭据 Provider 错误帧），got: {frame}"
    );
    assert!(
        frame["error"]["code"].as_i64().unwrap_or(0) != 0,
        "错误码非零，got: {frame}"
    );
    let frames = proc_b.finish(0);
    assert!(frames.len() >= 3, "至少收到握手/恢复/错误三类帧");
    let _ = std::fs::remove_dir_all(&data);
}
