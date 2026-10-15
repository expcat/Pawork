//! Controlled channel to the Firefox guest inside the isolated desktop.
//!
//! The production socket is fixed at 127.0.0.1:5906. Nothing in this module
//! accepts a model-supplied port, path, or token. The shared secret is the
//! process environment variable `PAWORK_ISOLATED_GUEST_TOKEN` and must be at
//! least 16 bytes. A bad hello, a missing token, or a broken frame drops the
//! connection; the client never reuses it.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::time::Duration;

pub const GUEST_PORT: u16 = 5906;
pub const ENDPOINT: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), GUEST_PORT);
pub const MIN_TOKEN_BYTES: usize = 16;
pub const FIREFOX_BUNDLE_ID: &str = "guest.browser.firefox";
/// CU-26 Wayland guest. A separate fixed port so a Host never mistakes the
/// X11 Firefox guest for the Wayland seat guest or the other way round.
pub const WAYLAND_GUEST_PORT: u16 = 5907;
pub const WAYLAND_ENDPOINT: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), WAYLAND_GUEST_PORT);
/// Wayland toplevels are named `guest.wayland.<app_id>`.
pub const WAYLAND_BUNDLE_PREFIX: &str = "guest.wayland.";
const MAX_FRAME: usize = 1024 * 1024;
const IO_BUDGET: Duration = Duration::from_secs(20);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GuestError {
    #[error("guest identity rejected: {0}")]
    Identity(&'static str),
    #[error("guest disconnected: {0}")]
    Disconnected(&'static str),
    #[error("guest protocol rejected: {0}")]
    Protocol(&'static str),
    #[error("guest unavailable: {0}")]
    Unavailable(String),
}

/// Identity-only snapshot. Titles and accessibility trees are absent on purpose:
/// discovery, resolve, and liveness may run before a target grant.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GuestMetadata {
    pub apps: Vec<GuestAppInfo>,
    pub rejected: Vec<RejectedWindow>,
    /// Connection epoch mixed into every window generation. A new connection
    /// advances it, so a generation issued on the previous connection cannot match.
    pub epoch: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GuestAppInfo {
    pub bundle_id: String,
    pub name: String,
    pub pid: u32,
    pub start_token: u64,
    pub windows: Vec<GuestWindow>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GuestWindow {
    pub window_id: u64,
    pub generation: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub class: String,
    pub instance: String,
    /// Higher paints above lower. Used to reject an occluded capture.
    pub stack: u32,
    /// Direct X11 parent. Used only to exclude this window's own frame.
    pub parent: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RejectedWindow {
    pub window_id: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub class: String,
    pub instance: String,
    pub stack: u32,
    /// Direct X11 parent. A child or parent of the target is its own chrome.
    pub parent: u64,
    /// False when origin or size could not be read. Capture must refuse it.
    pub geometry_known: bool,
    /// Human-readable rejection. This is never an AppKit classification.
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GuestTree {
    pub window_id: u64,
    pub max_depth: u32,
    pub max_nodes: u32,
    pub max_text: usize,
    pub truncation: GuestTruncation,
    pub nodes: Vec<GuestNode>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuestTruncation {
    pub depth: bool,
    pub nodes: bool,
    pub text: bool,
    pub names: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GuestNode {
    pub path: Vec<u32>,
    pub depth: u32,
    pub role: String,
    pub name: Option<String>,
    pub name_truncated: bool,
    pub name_unreadable: bool,
    pub enabled: Option<bool>,
    pub focused: Option<bool>,
    pub frame: Option<[f64; 4]>,
    pub actions: Vec<String>,
    pub children: u32,
    pub depth_limited: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Pressed {
    pub window_id: u64,
    pub generation: u64,
    pub tree_revision: u64,
    pub path: Vec<u32>,
    pub role: String,
    pub action: String,
}

/// Read one already-identified window. The guest must root the tree at that
/// window and refuse to substitute the application or another window.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TreeQuery {
    pub window_id: u64,
    pub generation: u64,
}

/// Press one element of a tree that was just read. The guest re-reads that
/// window and dispatches only when the generation, revision, and path still match.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PressQuery {
    pub window_id: u64,
    pub generation: u64,
    pub tree_revision: u64,
    pub path: Vec<u32>,
}

/// Capture one already-identified window (CU-26). The guest refuses unless
/// the generation still matches and nothing else paints over the window.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureQuery {
    pub window_id: u64,
    pub generation: u64,
}

/// One window cut from a fresh whole-output frame. The geometry is the
/// window rect the guest re-read after the frame; `jpeg` is base64.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuestCapture {
    pub window_id: u64,
    pub generation: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub image_width: u32,
    pub image_height: u32,
    pub jpeg: String,
}

/// Seat input against one window. Click points are window-local pixels;
/// the guest adds the window origin it re-read just before sending.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SeatInput {
    Click {
        x: u32,
        y: u32,
    },
    Text {
        text: String,
    },
    Key {
        key: String,
        shift: bool,
        control: bool,
        alt: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputQuery {
    pub window_id: u64,
    pub generation: u64,
    pub input: SeatInput,
}

/// `complete` means every planned step reached the seat (a sync round-trip),
/// not that the target reacted. A partial receipt left nothing pressed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputReceipt {
    pub window_id: u64,
    pub generation: u64,
    pub steps_sent: u32,
    pub steps_total: u32,
    pub complete: bool,
}

pub trait GuestApp {
    /// Called once after the handshake, before any operation on this connection.
    fn begin_session(&mut self) {}
    fn metadata(&mut self) -> Result<GuestMetadata, GuestError>;
    fn tree(&mut self, query: &TreeQuery) -> Result<GuestTree, GuestError>;
    /// stop is true when the client has gone away. Check it again after the
    /// version re-read and before the action. A true result means do not act.
    fn press(&mut self, query: &PressQuery, stop: &dyn Fn() -> bool)
        -> Result<Pressed, GuestError>;
    fn capture(
        &mut self,
        _query: &CaptureQuery,
        _stop: &dyn Fn() -> bool,
    ) -> Result<GuestCapture, GuestError> {
        Err(GuestError::Unavailable(
            "this guest does not capture windows".into(),
        ))
    }
    /// Same stop contract as [Self::press]; between steps a true stop ends
    /// the sequence with a partial receipt and nothing left pressed.
    fn input(
        &mut self,
        _query: &InputQuery,
        _stop: &dyn Fn() -> bool,
    ) -> Result<InputReceipt, GuestError> {
        Err(GuestError::Unavailable(
            "this guest does not send seat input".into(),
        ))
    }
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(BASE64[(n >> shift & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub(crate) fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let last = index + 1 == bytes.len() / 4;
        let pad = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for byte in &chunk[..4 - pad] {
            let value = BASE64.iter().position(|item| item == byte)? as u32;
            n = n << 6 | value;
        }
        n <<= 6 * pad as u32;
        let decoded = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&decoded[..3 - pad]);
    }
    Some(out)
}

pub fn ax_nodes(nodes: &[GuestNode]) -> Vec<crate::target::AxTreeNode> {
    nodes
        .iter()
        .map(|node| crate::target::AxTreeNode {
            path: node.path.clone(),
            depth: node.depth,
            role: node.role.clone(),
            name: node.name.clone(),
            name_truncated: node.name_truncated,
            name_unreadable: node.name_unreadable,
            enabled: node.enabled,
            focused: node.focused,
            frame: node.frame,
            actions: node.actions.clone(),
            children: node.children,
            depth_limited: node.depth_limited,
        })
        .collect()
}

pub fn tree_revision(tree: &GuestTree) -> u64 {
    crate::target::ax_tree_revision(
        &ax_nodes(&tree.nodes),
        crate::target::AxTreeTruncation {
            depth: tree.truncation.depth,
            nodes: tree.truncation.nodes,
            text: tree.truncation.text,
            names: tree.truncation.names,
        },
    )
}

pub fn token_from_env() -> Result<String, GuestError> {
    require_token(std::env::var("PAWORK_ISOLATED_GUEST_TOKEN").ok().as_deref())
}

pub fn require_token(token: Option<&str>) -> Result<String, GuestError> {
    let Some(token) = token else {
        return Err(GuestError::Identity("missing guest token"));
    };
    if token.len() < MIN_TOKEN_BYTES
        || token
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(GuestError::Identity(
            "guest token must be at least 16 non-space bytes",
        ));
    }
    Ok(token.to_string())
}

fn discard(stream: &mut TcpStream) {
    let _ = stream.shutdown(Shutdown::Both);
}

fn arm(stream: &TcpStream) -> Result<(), GuestError> {
    stream
        .set_nodelay(true)
        .map_err(|_| GuestError::Disconnected("socket setup failed"))?;
    stream
        .set_read_timeout(Some(IO_BUDGET))
        .map_err(|_| GuestError::Disconnected("socket setup failed"))?;
    stream
        .set_write_timeout(Some(IO_BUDGET))
        .map_err(|_| GuestError::Disconnected("socket setup failed"))
}

pub fn write_frame(mut stream: impl Write, value: &impl Serialize) -> Result<(), GuestError> {
    let bytes = serde_json::to_vec(value).map_err(|_| GuestError::Protocol("frame is not json"))?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME {
        return Err(GuestError::Protocol("frame length is outside 1..=1MiB"));
    }
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .map_err(|_| GuestError::Disconnected("guest write failed"))?;
    stream
        .write_all(&bytes)
        .map_err(|_| GuestError::Disconnected("guest write failed"))?;
    stream
        .flush()
        .map_err(|_| GuestError::Disconnected("guest write failed"))
}

pub fn read_frame(mut stream: impl Read) -> Result<Value, GuestError> {
    let mut len_buf = [0; 4];
    read_exact(&mut stream, &mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(GuestError::Protocol("frame length is outside 1..=1MiB"));
    }
    let mut bytes = vec![0; len];
    read_exact(&mut stream, &mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|_| GuestError::Protocol("frame is not json"))
}

fn read_exact(stream: &mut impl Read, mut bytes: &mut [u8]) -> Result<(), GuestError> {
    while !bytes.is_empty() {
        match stream.read(bytes) {
            Ok(0) => return Err(GuestError::Disconnected("guest closed the connection")),
            Ok(n) => bytes = &mut bytes[n..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(GuestError::Disconnected("guest read failed")),
        }
    }
    Ok(())
}

fn error_frame(error: &GuestError) -> Value {
    let code = match error {
        GuestError::Identity(_) => "identity",
        GuestError::Disconnected(_) => "disconnected",
        GuestError::Protocol(_) => "protocol",
        GuestError::Unavailable(_) => "unavailable",
    };
    json!({"op":"error","code":code,"message":error.to_string()})
}

#[derive(Debug)]
pub struct GuestLink {
    stream: Option<TcpStream>,
}

impl GuestLink {
    /// Handshake on an already accepted stream. Tests use this so the model
    /// never supplies a port. Production uses [Self::connect].
    pub fn handshake(mut stream: TcpStream, token: &str) -> Result<Self, GuestError> {
        arm(&stream)?;
        let token = match require_token(Some(token)) {
            Ok(token) => token,
            Err(error) => {
                discard(&mut stream);
                return Err(error);
            }
        };
        if let Err(error) = write_frame(&mut stream, &json!({"op":"hello","token":token})) {
            discard(&mut stream);
            return Err(error);
        }
        let reply = match read_frame(&mut stream) {
            Ok(reply) => reply,
            Err(error) => {
                discard(&mut stream);
                return Err(error);
            }
        };
        match reply.get("op").and_then(Value::as_str) {
            Some("hello_ok") => Ok(Self {
                stream: Some(stream),
            }),
            Some("error") if reply.get("code").and_then(Value::as_str) == Some("identity") => {
                discard(&mut stream);
                Err(GuestError::Identity("guest rejected the token"))
            }
            _ => {
                discard(&mut stream);
                Err(GuestError::Protocol("unexpected guest handshake reply"))
            }
        }
    }

    /// Connect to the fixed loopback guest. The token comes from the environment.
    pub fn connect() -> Result<Self, GuestError> {
        let token = token_from_env()?;
        let stream =
            TcpStream::connect_timeout(&ENDPOINT, Duration::from_secs(2)).map_err(|_| {
                GuestError::Disconnected("isolated guest unavailable at 127.0.0.1:5906")
            })?;
        Self::handshake(stream, &token)
    }

    /// Connect to the fixed loopback Wayland guest with the same token rule.
    pub fn connect_wayland() -> Result<Self, GuestError> {
        let token = token_from_env()?;
        let stream = TcpStream::connect_timeout(&WAYLAND_ENDPOINT, Duration::from_secs(2))
            .map_err(|_| GuestError::Disconnected("wayland guest unavailable at 127.0.0.1:5907"))?;
        Self::handshake(stream, &token)
    }

    pub fn capture(&mut self, query: &CaptureQuery) -> Result<GuestCapture, GuestError> {
        if query.window_id == 0 {
            return Err(GuestError::Protocol("capture window is missing"));
        }
        let value = self.round_trip(&json!({"op":"capture","query":query}))?;
        decode_body(&value, "capture")
    }

    /// Same receipt rule as [Self::press_stopping]: once the request is
    /// fully written, a lost or unreadable reply means the input may have
    /// reached the seat.
    pub fn input_stopping(
        &mut self,
        query: &InputQuery,
        stop: &dyn Fn() -> bool,
    ) -> Result<InputReceipt, GuestError> {
        if query.window_id == 0 {
            return Err(GuestError::Protocol("input window is missing"));
        }
        let request = json!({"op":"input","query":query});
        self.dispatch_stopping(&request, "input_ok", "receipt", INPUT_LOST, stop)
    }

    pub fn metadata(&mut self) -> Result<GuestMetadata, GuestError> {
        let value = self.round_trip(&json!({"op":"metadata"}))?;
        decode_body(&value, "metadata")
    }

    pub fn tree(&mut self, query: &TreeQuery) -> Result<GuestTree, GuestError> {
        if query.window_id == 0 {
            return Err(GuestError::Protocol("tree window is missing"));
        }
        let value = self.round_trip(
            &json!({"op":"tree","window_id":query.window_id,"generation":query.generation}),
        )?;
        decode_body(&value, "tree")
    }

    pub fn press(&mut self, query: &PressQuery) -> Result<Pressed, GuestError> {
        self.press_stopping(query, &|| false)
    }

    /// Check stop before the request is written. After a full write, a lost or
    /// unreadable reply is a receipt loss: the action may already have run.
    pub fn press_stopping(
        &mut self,
        query: &PressQuery,
        stop: &dyn Fn() -> bool,
    ) -> Result<Pressed, GuestError> {
        if query.window_id == 0 || query.path.len() > 64 {
            return Err(GuestError::Protocol(
                "press target is not a window element path",
            ));
        }
        let request = json!({
            "op": "press",
            "window_id": query.window_id,
            "generation": query.generation,
            "tree_revision": query.tree_revision,
            "path": query.path,
        });
        self.dispatch_stopping(&request, "press_ok", "pressed", PRESS_LOST, stop)
    }

    fn dispatch_stopping<T: for<'de> Deserialize<'de>>(
        &mut self,
        request: &Value,
        ok_op: &str,
        field: &str,
        lost: &'static str,
        stop: &dyn Fn() -> bool,
    ) -> Result<T, GuestError> {
        if stop() {
            return Err(GuestError::Disconnected(
                "cancelled before the guest action",
            ));
        }
        let write_result = {
            let Some(stream) = self.stream.as_mut() else {
                return Err(GuestError::Disconnected("guest connection already dropped"));
            };
            write_frame(&mut *stream, request)
        };
        if let Err(error) = write_result {
            self.drop_stream();
            return Err(error);
        }
        let read_result = {
            let Some(stream) = self.stream.as_mut() else {
                return Err(GuestError::Disconnected(lost));
            };
            read_dispatch_reply(stream, stop, lost)
        };
        let reply = match read_result {
            Ok(reply) => reply,
            Err(error) => {
                self.drop_stream();
                return Err(error);
            }
        };
        let outcome = match reply.get("op").and_then(Value::as_str) {
            Some(op) if op == ok_op => {
                decode_body(&reply, field).map_err(|_| GuestError::Disconnected(lost))
            }
            Some("error") => Err(guest_error_from_frame(&reply)),
            _ => Err(GuestError::Disconnected(lost)),
        };
        if outcome.is_err() && !matches!(outcome, Err(GuestError::Unavailable(_))) {
            self.drop_stream();
        }
        outcome
    }

    fn round_trip(&mut self, request: &Value) -> Result<Value, GuestError> {
        let Some(stream) = self.stream.as_mut() else {
            return Err(GuestError::Disconnected("guest connection already dropped"));
        };
        let outcome = (|| {
            write_frame(&mut *stream, request)?;
            let reply = read_frame(&mut *stream)?;
            match reply.get("op").and_then(Value::as_str) {
                Some("metadata_ok" | "tree_ok" | "press_ok" | "capture_ok") => Ok(reply),
                Some("error") => Err(guest_error_from_frame(&reply)),
                _ => Err(GuestError::Protocol("unexpected guest reply")),
            }
        })();
        let keep = matches!(outcome, Err(GuestError::Unavailable(_)));
        if outcome.is_err() && !keep {
            if let Some(mut stream) = self.stream.take() {
                discard(&mut stream);
            }
        }
        outcome
    }

    fn drop_stream(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            discard(&mut stream);
        }
    }

    /// Shut the cached socket down but leave it installed. Tests use this
    /// to imitate the guest closing an idle connection.
    #[cfg(test)]
    pub(crate) fn sever(&mut self) {
        if let Some(stream) = self.stream.as_mut() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

fn decode_body<T: for<'de> Deserialize<'de>>(value: &Value, field: &str) -> Result<T, GuestError> {
    serde_json::from_value(
        value
            .get(field)
            .cloned()
            .ok_or(GuestError::Protocol("guest reply is missing its body"))?,
    )
    .map_err(|_| GuestError::Protocol("guest reply body has the wrong shape"))
}

fn guest_error_from_frame(reply: &Value) -> GuestError {
    let code = reply.get("code").and_then(Value::as_str).unwrap_or("");
    let message = reply
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("guest error");
    match code {
        "identity" => GuestError::Identity("guest rejected the token"),
        "protocol" => GuestError::Protocol("guest rejected the frame"),
        "disconnected" => GuestError::Disconnected("guest closed the connection"),
        _ => GuestError::Unavailable(message.to_string()),
    }
}

const PRESS_LOST: &str = "press receipt lost";
const INPUT_LOST: &str = "input receipt lost";

fn read_dispatch_reply(
    stream: &mut TcpStream,
    stop: &dyn Fn() -> bool,
    lost: &'static str,
) -> Result<Value, GuestError> {
    let deadline = std::time::Instant::now() + IO_BUDGET;
    loop {
        if stop() || std::time::Instant::now() >= deadline {
            let _ = stream.shutdown(Shutdown::Both);
            return Err(GuestError::Disconnected(lost));
        }
        stream
            .set_nonblocking(true)
            .map_err(|_| GuestError::Disconnected(lost))?;
        let mut probe = [0u8; 1];
        let ready = match stream.peek(&mut probe) {
            Ok(0) => {
                let _ = stream.set_nonblocking(false);
                return Err(GuestError::Disconnected(lost));
            }
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
            Err(_) => {
                let _ = stream.set_nonblocking(false);
                return Err(GuestError::Disconnected(lost));
            }
        };
        stream
            .set_nonblocking(false)
            .map_err(|_| GuestError::Disconnected(lost))?;
        if ready {
            return read_frame(&mut *stream).map_err(|_| GuestError::Disconnected(lost));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn peer_gone(stream: &TcpStream) -> bool {
    let _ = stream.set_nonblocking(true);
    let mut probe = [0u8; 1];
    let gone = match stream.peek(&mut probe) {
        Ok(0) => true,
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
        Err(_) => true,
        Ok(_) => false,
    };
    let _ = stream.set_nonblocking(false);
    gone
}

impl Drop for GuestLink {
    fn drop(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            discard(&mut stream);
        }
    }
}

/// Serve one client. Identity failure and broken frames close the socket
/// before any desktop operation. Application errors stay on the connection so
/// a later call can retry. Each accepted connection starts a new guest epoch.
pub fn serve_connection(
    mut stream: TcpStream,
    token: &str,
    handler: &mut dyn GuestApp,
) -> Result<(), GuestError> {
    arm(&stream)?;
    let expected = match require_token(Some(token)) {
        Ok(token) => token,
        Err(error) => {
            let _ = write_frame(&mut stream, &error_frame(&error));
            discard(&mut stream);
            return Err(error);
        }
    };
    let hello = match read_frame(&mut stream) {
        Ok(hello) => hello,
        Err(error) => {
            discard(&mut stream);
            return Err(error);
        }
    };
    let presented = hello.get("token").and_then(Value::as_str).unwrap_or("");
    if hello.get("op").and_then(Value::as_str) != Some("hello") || presented != expected {
        let error = GuestError::Identity("guest token rejected");
        let _ = write_frame(&mut stream, &error_frame(&error));
        discard(&mut stream);
        return Err(error);
    }
    if let Err(error) = write_frame(&mut stream, &json!({"op":"hello_ok"})) {
        discard(&mut stream);
        return Err(error);
    }
    handler.begin_session();
    loop {
        let request = match read_frame(&mut stream) {
            Ok(request) => request,
            Err(GuestError::Disconnected(_)) => {
                discard(&mut stream);
                return Ok(());
            }
            Err(error) => {
                let _ = write_frame(&mut stream, &error_frame(&error));
                discard(&mut stream);
                return Err(error);
            }
        };
        let op = request.get("op").and_then(Value::as_str).unwrap_or("");
        let reply = match op {
            "metadata" => match handler.metadata() {
                Ok(metadata) => json!({"op":"metadata_ok","metadata":metadata}),
                Err(error) => error_frame(&error),
            },
            "tree" => {
                let query = TreeQuery {
                    window_id: request
                        .get("window_id")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    generation: request
                        .get("generation")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                };
                match handler.tree(&query) {
                    Ok(tree) => json!({"op":"tree_ok","tree":tree}),
                    Err(error) => error_frame(&error),
                }
            }
            "press" => {
                let Some(raw_path) = request.get("path").and_then(Value::as_array) else {
                    let error = GuestError::Protocol("press path is missing");
                    let _ = write_frame(&mut stream, &error_frame(&error));
                    discard(&mut stream);
                    return Err(error);
                };
                let mut path = Vec::with_capacity(raw_path.len());
                for item in raw_path {
                    let Some(index) = item.as_u64().and_then(|value| u32::try_from(value).ok())
                    else {
                        let error = GuestError::Protocol("press path contains a non-index");
                        let _ = write_frame(&mut stream, &error_frame(&error));
                        discard(&mut stream);
                        return Err(error);
                    };
                    path.push(index);
                }
                let query = PressQuery {
                    window_id: request
                        .get("window_id")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    generation: request
                        .get("generation")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    tree_revision: request
                        .get("tree_revision")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    path,
                };
                let reply = match handler.press(&query, &|| peer_gone(&stream)) {
                    Ok(pressed) => json!({"op":"press_ok","pressed":pressed}),
                    Err(error) => error_frame(&error),
                };
                let _ = stream.set_nonblocking(false);
                reply
            }
            "capture" | "input" => {
                let Some(query) = request.get("query").cloned() else {
                    let error = GuestError::Protocol("guest query is missing");
                    let _ = write_frame(&mut stream, &error_frame(&error));
                    discard(&mut stream);
                    return Err(error);
                };
                let reply = if op == "capture" {
                    match serde_json::from_value::<CaptureQuery>(query) {
                        Ok(query) => match handler.capture(&query, &|| peer_gone(&stream)) {
                            Ok(capture) => json!({"op":"capture_ok","capture":capture}),
                            Err(error) => error_frame(&error),
                        },
                        Err(_) => {
                            let error = GuestError::Protocol("capture query has the wrong shape");
                            let _ = write_frame(&mut stream, &error_frame(&error));
                            discard(&mut stream);
                            return Err(error);
                        }
                    }
                } else {
                    match serde_json::from_value::<InputQuery>(query) {
                        Ok(query) => match handler.input(&query, &|| peer_gone(&stream)) {
                            Ok(receipt) => json!({"op":"input_ok","receipt":receipt}),
                            Err(error) => error_frame(&error),
                        },
                        Err(_) => {
                            let error = GuestError::Protocol("input query has the wrong shape");
                            let _ = write_frame(&mut stream, &error_frame(&error));
                            discard(&mut stream);
                            return Err(error);
                        }
                    }
                };
                let _ = stream.set_nonblocking(false);
                reply
            }
            _ => {
                let error = GuestError::Protocol("unknown guest op");
                let _ = write_frame(&mut stream, &error_frame(&error));
                discard(&mut stream);
                return Err(error);
            }
        };
        if let Err(error) = write_frame(&mut stream, &reply) {
            discard(&mut stream);
            return Err(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    struct Idle {
        called: Arc<AtomicBool>,
    }

    impl GuestApp for Idle {
        fn metadata(&mut self) -> Result<GuestMetadata, GuestError> {
            self.called.store(true, Ordering::SeqCst);
            Err(GuestError::Unavailable("idle".into()))
        }
        fn tree(&mut self, _: &TreeQuery) -> Result<GuestTree, GuestError> {
            self.called.store(true, Ordering::SeqCst);
            Err(GuestError::Unavailable("idle".into()))
        }
        fn press(&mut self, _: &PressQuery, _: &dyn Fn() -> bool) -> Result<Pressed, GuestError> {
            self.called.store(true, Ordering::SeqCst);
            Err(GuestError::Unavailable("idle".into()))
        }
    }

    #[test]
    fn base64_round_trips_and_refuses_malformed_text() {
        for len in 0..7 {
            let bytes: Vec<u8> = (0..len).map(|index| (index * 37 + 250) as u8).collect();
            let text = encode_base64(&bytes);
            assert_eq!(text.len() % 4, 0);
            assert_eq!(decode_base64(&text), Some(bytes));
        }
        assert_eq!(encode_base64(b"Pawork"), "UGF3b3Jr");
        for bad in ["A", "AB=C", "A===", "QQ==QQ==", "@@@@"] {
            assert_eq!(decode_base64(bad), None, "{bad}");
        }
    }

    #[test]
    fn seat_ops_default_to_unavailable_and_a_bad_query_drops_the_connection() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let token = "0123456789abcdef";
        let called = Arc::new(AtomicBool::new(false));
        let server_called = Arc::clone(&called);
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut handler = Idle {
                    called: Arc::clone(&server_called),
                };
                let _ = serve_connection(stream, token, &mut handler);
            }
        });

        let stream = TcpStream::connect(address).unwrap();
        let mut link = GuestLink::handshake(stream, token).unwrap();
        let capture = link
            .capture(&CaptureQuery {
                window_id: 1,
                generation: 1,
            })
            .unwrap_err();
        assert!(
            matches!(capture, GuestError::Unavailable(message) if message.contains("does not capture"))
        );
        let input = link
            .input_stopping(
                &InputQuery {
                    window_id: 1,
                    generation: 1,
                    input: SeatInput::Click { x: 0, y: 0 },
                },
                &|| false,
            )
            .unwrap_err();
        assert!(
            matches!(input, GuestError::Unavailable(message) if message.contains("does not send"))
        );
        let stopped = link
            .input_stopping(
                &InputQuery {
                    window_id: 1,
                    generation: 1,
                    input: SeatInput::Text { text: "a".into() },
                },
                &|| true,
            )
            .unwrap_err();
        assert_eq!(
            stopped,
            GuestError::Disconnected("cancelled before the guest action")
        );
        drop(link);

        let mut stream = TcpStream::connect(address).unwrap();
        arm(&stream).unwrap();
        write_frame(&mut stream, &json!({"op":"hello","token":token})).unwrap();
        assert_eq!(read_frame(&mut stream).unwrap()["op"], json!("hello_ok"));
        write_frame(
            &mut stream,
            &json!({"op":"input","query":{"window_id":1,"generation":1}}),
        )
        .unwrap();
        assert_eq!(read_frame(&mut stream).unwrap()["code"], json!("protocol"));
        let mut probe = [0; 1];
        assert_eq!(stream.read(&mut probe).unwrap(), 0);
        server.join().unwrap();
        assert!(!called.load(Ordering::SeqCst));
    }

    #[test]
    fn bad_or_missing_token_is_rejected_and_the_connection_is_dropped() {
        assert_eq!(ENDPOINT, "127.0.0.1:5906".parse::<SocketAddr>().unwrap());
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let token = "0123456789abcdef".to_string();
        let called = Arc::new(AtomicBool::new(false));
        let server_called = Arc::clone(&called);
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut handler = Idle {
                called: Arc::clone(&server_called),
            };
            let wrong = serve_connection(stream, &token, &mut handler).unwrap_err();
            assert!(matches!(wrong, GuestError::Identity(_)));

            let (stream, _) = listener.accept().unwrap();
            let mut handler = Idle {
                called: server_called,
            };
            let missing = serve_connection(stream, &token, &mut handler).unwrap_err();
            assert!(matches!(missing, GuestError::Disconnected(_)));
        });

        let stream = TcpStream::connect(address).unwrap();
        let wrong = GuestLink::handshake(stream, "not-the-real-token").unwrap_err();
        assert_eq!(wrong, GuestError::Identity("guest rejected the token"));

        let mut stream = TcpStream::connect(address).unwrap();
        arm(&stream).unwrap();
        let missing = GuestLink::handshake(stream.try_clone().unwrap(), "").unwrap_err();
        assert_eq!(
            missing,
            GuestError::Identity("guest token must be at least 16 non-space bytes")
        );
        let mut probe = [0; 1];
        assert_eq!(stream.read(&mut probe).unwrap(), 0);

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let bare = listener.local_addr().unwrap();
        let configured = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            arm(&stream).unwrap();
            let mut handler = Idle {
                called: Arc::new(AtomicBool::new(false)),
            };
            let error =
                serve_connection(stream.try_clone().unwrap(), "short", &mut handler).unwrap_err();
            assert!(matches!(error, GuestError::Identity(_)));
            let mut probe = [0; 1];
            assert_eq!(stream.read(&mut probe).unwrap(), 0);
        });
        let mut stream = TcpStream::connect(bare).unwrap();
        arm(&stream).unwrap();
        write_frame(
            &mut stream,
            &json!({"op":"hello","token":"0123456789abcdef"}),
        )
        .unwrap();
        let reply = read_frame(&mut stream).unwrap();
        assert_eq!(reply["code"], json!("identity"));
        let mut probe = [0; 1];
        assert_eq!(stream.read(&mut probe).unwrap(), 0);
        configured.join().unwrap();
        server.join().unwrap();
        assert!(!called.load(Ordering::SeqCst));
    }
}
