//! Sway IPC: compositor-authoritative window identity and the dedicated
//! seat check (CU-26).
//!
//! Window pids come from the compositor's own client credentials; a window
//! is keyed by (compositor instance, con id) and sway never reuses a con id
//! within one compositor process, so a compositor restart — the only way an
//! id can come back — clears every generation.

use super::SeatError;
use crate::target::ProcessInstance;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

const MAGIC: &[u8; 6] = b"i3-ipc";
const MAX_REPLY: usize = 4 * 1024 * 1024;
pub(crate) const GET_OUTPUTS: u32 = 3;
pub(crate) const GET_TREE: u32 = 4;
pub(crate) const GET_INPUTS: u32 = 100;
pub(crate) const GET_SEATS: u32 = 101;
/// Input devices the wlroots virtual pointer / keyboard protocols create.
/// Anything else on the seat (libinput, nested-backend or headless test
/// devices) means the seat is not Pawork's alone.
const VIRTUAL_INPUTS: [&str; 2] = ["0:0:wlr_virtual_pointer_v1", "0:0:wlr_virtual_keyboard_v1"];

pub(crate) fn encode_request(kind: u32, payload: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(14 + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(payload.as_bytes());
    out
}

pub(crate) fn read_reply(reader: &mut impl Read, kind: u32) -> Result<Value, SeatError> {
    let mut header = [0u8; 14];
    reader.read_exact(&mut header).map_err(ipc)?;
    if &header[..6] != MAGIC {
        return Err(SeatError::Ipc("reply magic mismatch".into()));
    }
    let len = u32::from_ne_bytes(header[6..10].try_into().expect("four bytes")) as usize;
    let reply_kind = u32::from_ne_bytes(header[10..14].try_into().expect("four bytes"));
    if reply_kind != kind {
        return Err(SeatError::Ipc(format!(
            "reply type {reply_kind} for request {kind}"
        )));
    }
    if len > MAX_REPLY {
        return Err(SeatError::Ipc(format!(
            "reply of {len} bytes exceeds the bound"
        )));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).map_err(ipc)?;
    serde_json::from_slice(&payload).map_err(|error| SeatError::Ipc(error.to_string()))
}

fn ipc(error: std::io::Error) -> SeatError {
    SeatError::Ipc(error.to_string())
}

/// The compositor process behind the IPC socket, from SO_PEERCRED.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CompositorInstance {
    pub pid: u32,
    pub start_token: u64,
}

pub struct SwayIpc {
    stream: UnixStream,
    compositor: CompositorInstance,
}

impl SwayIpc {
    /// Connects to `SWAYSOCK`; a missing variable is a refusal, not a search.
    pub fn connect_from_env(io_timeout: Duration) -> Result<Self, SeatError> {
        let path = std::env::var_os("SWAYSOCK")
            .ok_or_else(|| SeatError::Ipc("SWAYSOCK is not set".into()))?;
        Self::connect(Path::new(&path), io_timeout)
    }

    pub fn connect(path: &Path, io_timeout: Duration) -> Result<Self, SeatError> {
        let stream = UnixStream::connect(path)
            .map_err(|error| SeatError::Ipc(format!("{}: {error}", path.display())))?;
        stream
            .set_read_timeout(Some(io_timeout))
            .and_then(|_| stream.set_write_timeout(Some(io_timeout)))
            .map_err(ipc)?;
        let pid = peer_pid(&stream)?;
        let compositor = CompositorInstance {
            pid,
            start_token: process_start(pid)
                .ok_or_else(|| SeatError::Ipc("compositor start time unreadable".into()))?,
        };
        Ok(Self { stream, compositor })
    }

    pub fn compositor(&self) -> CompositorInstance {
        self.compositor
    }

    pub fn request(&mut self, kind: u32, payload: &str) -> Result<Value, SeatError> {
        self.stream
            .write_all(&encode_request(kind, payload))
            .map_err(ipc)?;
        read_reply(&mut self.stream, kind)
    }

    pub fn windows(&mut self) -> Result<Vec<SwayWindow>, SeatError> {
        let tree = self.request(GET_TREE, "")?;
        windows(&tree)
    }

    /// Refuses unless the compositor drives exactly one headless output and
    /// one seat whose only input devices are wlroots virtual devices; returns
    /// the input identifiers it checked.
    pub fn require_dedicated(&mut self) -> Result<Vec<String>, SeatError> {
        let outputs = self.request(GET_OUTPUTS, "")?;
        let inputs = self.request(GET_INPUTS, "")?;
        let seats = self.request(GET_SEATS, "")?;
        check_dedicated(&outputs, &inputs, &seats)
    }
}

fn peer_pid(stream: &UnixStream) -> Result<u32, SeatError> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred / len are live, correctly sized out-parameters.
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if status != 0 || cred.pid <= 0 {
        return Err(SeatError::Ipc("compositor credentials unavailable".into()));
    }
    Ok(cred.pid as u32)
}

/// Process start time in clock ticks (field 22 of /proc/<pid>/stat): with
/// the pid it identifies one process instance across pid reuse.
pub fn process_start(pid: u32) -> Option<u64> {
    parse_start_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// The command name may contain spaces and parentheses, so fields are
/// counted after the last ')'; starttime is the 20th of those.
pub(crate) fn parse_start_ticks(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(19)?.parse().ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwayRect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwayWindow {
    pub con_id: u64,
    pub pid: u32,
    pub app_id: String,
    pub rect: SwayRect,
    pub visible: bool,
    pub focused: bool,
}

/// Native Wayland toplevels in tree order. Xwayland windows and toplevels
/// without a pid or app_id are refused: neither has the identity the
/// handle contract needs, and the dedicated guest runs no X server.
pub(crate) fn windows(tree: &Value) -> Result<Vec<SwayWindow>, SeatError> {
    let mut out = Vec::new();
    visit(tree, 0, &mut out)?;
    Ok(out)
}

fn visit(node: &Value, depth: usize, out: &mut Vec<SwayWindow>) -> Result<(), SeatError> {
    if depth > 64 {
        return Err(SeatError::Ipc("tree deeper than 64 levels".into()));
    }
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    if matches!(kind, "con" | "floating_con") && node.get("pid").is_some() {
        match node.get("shell").and_then(Value::as_str) {
            Some("xdg_shell") => {}
            other => {
                return Err(SeatError::NotDedicated(format!(
                    "window with shell {other:?} on the seat"
                )))
            }
        }
        out.push(window(node)?);
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node.get(key).and_then(Value::as_array) {
            for child in children {
                visit(child, depth + 1, out)?;
            }
        }
    }
    Ok(())
}

fn window(node: &Value) -> Result<SwayWindow, SeatError> {
    let field = |name: &str| SeatError::Ipc(format!("toplevel without a valid {name}"));
    let rect = node.get("rect").ok_or_else(|| field("rect"))?;
    let coord = |name: &str| {
        rect.get(name)
            .and_then(Value::as_i64)
            .ok_or_else(|| field(name))
    };
    let pid = node
        .get("pid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0 && *pid <= u32::MAX as u64)
        .ok_or_else(|| field("pid"))? as u32;
    let app_id = node
        .get("app_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| field("app_id"))?;
    Ok(SwayWindow {
        con_id: node
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| field("id"))?,
        pid,
        app_id: app_id.to_string(),
        rect: SwayRect {
            x: coord("x")?,
            y: coord("y")?,
            width: coord("width")?,
            height: coord("height")?,
        },
        visible: node
            .get("visible")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        focused: node
            .get("focused")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

pub(crate) fn check_dedicated(
    outputs: &Value,
    inputs: &Value,
    seats: &Value,
) -> Result<Vec<String>, SeatError> {
    let refuse = |reason: String| Err(SeatError::NotDedicated(reason));
    let outputs = outputs.as_array().map(Vec::as_slice).unwrap_or_default();
    let names: Vec<&str> = outputs
        .iter()
        .map(|output| {
            output
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .collect();
    if names.len() != 1 || !names[0].starts_with("HEADLESS-") {
        return refuse(format!(
            "outputs {names:?}, need exactly one headless output"
        ));
    }
    let seats = seats.as_array().map(Vec::as_slice).unwrap_or_default();
    if seats.len() != 1 {
        return refuse(format!("{} seats, need exactly one", seats.len()));
    }
    let mut checked = Vec::new();
    for input in inputs.as_array().map(Vec::as_slice).unwrap_or_default() {
        let identifier = input
            .get("identifier")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !VIRTUAL_INPUTS.contains(&identifier) {
            return refuse(format!("input device {identifier:?} on the seat"));
        }
        checked.push(identifier.to_string());
    }
    Ok(checked)
}

/// Window generations for one compositor instance. A con id first seen gets
/// a fresh generation, a vanished one is forgotten, and a new compositor
/// instance drops every entry — old handles can never match again.
#[derive(Debug, Default)]
pub struct WindowLedger {
    compositor: Option<CompositorInstance>,
    generations: HashMap<u64, u64>,
    next: u64,
}

impl WindowLedger {
    pub fn sync(
        &mut self,
        compositor: CompositorInstance,
        windows: &[SwayWindow],
    ) -> Vec<(SwayWindow, ProcessInstance, u64)> {
        if self.compositor != Some(compositor) {
            self.compositor = Some(compositor);
            self.generations.clear();
        }
        self.generations
            .retain(|con, _| windows.iter().any(|window| window.con_id == *con));
        let mut out = Vec::with_capacity(windows.len());
        for window in windows {
            let Some(start_token) = process_start(window.pid) else {
                continue;
            };
            let generation = *self.generations.entry(window.con_id).or_insert_with(|| {
                self.next += 1;
                self.next
            });
            out.push((
                window.clone(),
                ProcessInstance {
                    pid: window.pid,
                    start_token,
                },
                generation,
            ));
        }
        out
    }
}
