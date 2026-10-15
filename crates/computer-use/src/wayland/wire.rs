//! Minimal Wayland wire codec and Unix-socket connection (CU-26).
//!
//! Only what the dedicated seat client needs: little-endian 32-bit words,
//! strings, typed / untyped new_id, and file descriptors passed out of band
//! with SCM_RIGHTS. Received descriptors are closed immediately — none of
//! the bound interfaces is expected to send one.

use super::SeatError;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

pub(crate) const DISPLAY_ID: u32 = 1;
const HEADER: usize = 8;
/// libwayland's connection buffer; a larger message is a protocol violation.
pub(crate) const MAX_MESSAGE: usize = 4096;
/// Highest client-side object id (server ids start at 0xff000000).
const MAX_CLIENT_ID: u32 = 0xfeff_ffff;
const MAX_SEND_FDS: usize = 4;
const MAX_RECV_FDS: usize = 28;

/// One outgoing request.
#[derive(Debug)]
pub(crate) struct Message {
    object: u32,
    opcode: u16,
    body: Vec<u8>,
    fds: Vec<RawFd>,
}

impl Message {
    pub(crate) fn new(object: u32, opcode: u16) -> Self {
        Self {
            object,
            opcode,
            body: Vec::new(),
            fds: Vec::new(),
        }
    }

    pub(crate) fn uint(mut self, value: u32) -> Self {
        self.body.extend_from_slice(&value.to_ne_bytes());
        self
    }

    pub(crate) fn int(mut self, value: i32) -> Self {
        self.body.extend_from_slice(&value.to_ne_bytes());
        self
    }

    pub(crate) fn string(mut self, value: &str) -> Self {
        let len = value.len() + 1;
        self.body.extend_from_slice(&(len as u32).to_ne_bytes());
        self.body.extend_from_slice(value.as_bytes());
        self.body.push(0);
        while self.body.len() % 4 != 0 {
            self.body.push(0);
        }
        self
    }

    /// The descriptor must stay open until [Connection::send] returns.
    pub(crate) fn fd(mut self, fd: RawFd) -> Self {
        self.fds.push(fd);
        self
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, SeatError> {
        let size = HEADER + self.body.len();
        if size > MAX_MESSAGE || self.fds.len() > MAX_SEND_FDS {
            return Err(SeatError::Protocol(format!(
                "request {}#{} exceeds the wire bounds",
                self.object, self.opcode
            )));
        }
        let mut out = Vec::with_capacity(size);
        out.extend_from_slice(&self.object.to_ne_bytes());
        out.extend_from_slice(&(((size as u32) << 16) | self.opcode as u32).to_ne_bytes());
        out.extend_from_slice(&self.body);
        Ok(out)
    }
}

/// One incoming event; arguments are read in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Event {
    pub(crate) object: u32,
    pub(crate) opcode: u16,
    pub(crate) body: Vec<u8>,
}

impl Event {
    pub(crate) fn args(&self) -> Args<'_> {
        Args {
            body: &self.body,
            pos: 0,
        }
    }
}

pub(crate) struct Args<'a> {
    body: &'a [u8],
    pos: usize,
}

impl Args<'_> {
    pub(crate) fn uint(&mut self) -> Result<u32, SeatError> {
        let end = self.pos + 4;
        let bytes = self
            .body
            .get(self.pos..end)
            .ok_or_else(|| SeatError::Protocol("event argument truncated".into()))?;
        self.pos = end;
        Ok(u32::from_ne_bytes(bytes.try_into().expect("four bytes")))
    }

    pub(crate) fn int(&mut self) -> Result<i32, SeatError> {
        self.uint().map(|value| value as i32)
    }

    pub(crate) fn string(&mut self) -> Result<String, SeatError> {
        let len = self.uint()? as usize;
        if len == 0 {
            return Ok(String::new());
        }
        let padded = len.div_ceil(4) * 4;
        let raw = self
            .body
            .get(self.pos..self.pos + padded)
            .ok_or_else(|| SeatError::Protocol("event string truncated".into()))?;
        if raw[len - 1] != 0 {
            return Err(SeatError::Protocol(
                "event string is not NUL-terminated".into(),
            ));
        }
        self.pos += padded;
        String::from_utf8(raw[..len - 1].to_vec())
            .map_err(|_| SeatError::Protocol("event string is not UTF-8".into()))
    }
}

/// Splits one complete message off the front of `buffer`. None means more
/// bytes are needed; a malformed header is an error, never a resync.
pub(crate) fn split_event(buffer: &[u8]) -> Result<Option<(Event, usize)>, SeatError> {
    if buffer.len() < HEADER {
        return Ok(None);
    }
    let object = u32::from_ne_bytes(buffer[0..4].try_into().expect("four bytes"));
    let word = u32::from_ne_bytes(buffer[4..8].try_into().expect("four bytes"));
    let size = (word >> 16) as usize;
    if size < HEADER || size > MAX_MESSAGE || size % 4 != 0 {
        return Err(SeatError::Protocol(format!("invalid event size {size}")));
    }
    if buffer.len() < size {
        return Ok(None);
    }
    Ok(Some((
        Event {
            object,
            opcode: (word & 0xffff) as u16,
            body: buffer[HEADER..size].to_vec(),
        },
        size,
    )))
}

/// A blocking client connection with monotonic id allocation (ids are never
/// reused; a seat connection is short-lived relative to the id space).
pub(crate) struct Connection {
    stream: UnixStream,
    inbox: Vec<u8>,
    next_id: u32,
}

impl Connection {
    pub(crate) fn open(path: &Path, io_timeout: Duration) -> Result<Self, SeatError> {
        let stream = UnixStream::connect(path)
            .map_err(|error| SeatError::NoDisplay(format!("{}: {error}", path.display())))?;
        Self::from_stream(stream, io_timeout)
    }

    pub(crate) fn from_stream(stream: UnixStream, io_timeout: Duration) -> Result<Self, SeatError> {
        stream
            .set_read_timeout(Some(io_timeout))
            .and_then(|_| stream.set_write_timeout(Some(io_timeout)))
            .map_err(io)?;
        Ok(Self {
            stream,
            inbox: Vec::new(),
            next_id: DISPLAY_ID + 1,
        })
    }

    pub(crate) fn allocate(&mut self) -> Result<u32, SeatError> {
        if self.next_id > MAX_CLIENT_ID {
            return Err(SeatError::Protocol("client object ids exhausted".into()));
        }
        let id = self.next_id;
        self.next_id += 1;
        Ok(id)
    }

    pub(crate) fn send(&mut self, message: &Message) -> Result<(), SeatError> {
        let bytes = message.encode()?;
        let mut sent = send_with_fds(self.stream.as_raw_fd(), &bytes, &message.fds).map_err(io)?;
        while sent < bytes.len() {
            sent += send_with_fds(self.stream.as_raw_fd(), &bytes[sent..], &[]).map_err(io)?;
        }
        Ok(())
    }

    pub(crate) fn next_event(&mut self) -> Result<Event, SeatError> {
        loop {
            if let Some((event, used)) = split_event(&self.inbox)? {
                self.inbox.drain(..used);
                return Ok(event);
            }
            let mut chunk = [0u8; MAX_MESSAGE];
            let read = recv_dropping_fds(self.stream.as_raw_fd(), &mut chunk).map_err(io)?;
            if read == 0 {
                return Err(SeatError::Io("compositor closed the connection".into()));
            }
            self.inbox.extend_from_slice(&chunk[..read]);
        }
    }
}

fn io(error: std::io::Error) -> SeatError {
    match error.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
            SeatError::Io("compositor did not answer within the I/O budget".into())
        }
        _ => SeatError::Io(error.to_string()),
    }
}

fn send_with_fds(socket: RawFd, data: &[u8], fds: &[RawFd]) -> std::io::Result<usize> {
    let mut iov = libc::iovec {
        iov_base: data.as_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };
    // SAFETY: msghdr is plain old data; every pointer set below outlives
    // the sendmsg call.
    let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
    header.msg_iov = &mut iov;
    header.msg_iovlen = 1;
    let payload = std::mem::size_of_val(fds);
    // SAFETY: CMSG_SPACE is a pure size computation.
    let space = unsafe { libc::CMSG_SPACE(payload as u32) } as usize;
    let mut control = vec![0u64; space.div_ceil(8)];
    if !fds.is_empty() {
        header.msg_control = control.as_mut_ptr().cast();
        header.msg_controllen = space as _;
        // SAFETY: the control buffer is aligned and sized by CMSG_SPACE for
        // exactly `fds.len()` descriptors.
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&header);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(payload as u32) as _;
            std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(cmsg).cast(), fds.len());
        }
    }
    loop {
        // SAFETY: header points at live iov / control buffers.
        let sent = unsafe { libc::sendmsg(socket, &header, libc::MSG_NOSIGNAL) };
        if sent >= 0 {
            return Ok(sent as usize);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn recv_dropping_fds(socket: RawFd, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    // SAFETY: CMSG_SPACE is a pure size computation.
    let space =
        unsafe { libc::CMSG_SPACE((MAX_RECV_FDS * std::mem::size_of::<RawFd>()) as u32) } as usize;
    let mut control = vec![0u64; space.div_ceil(8)];
    // SAFETY: msghdr is plain old data; the buffers outlive recvmsg.
    let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
    header.msg_iov = &mut iov;
    header.msg_iovlen = 1;
    header.msg_control = control.as_mut_ptr().cast();
    header.msg_controllen = space as _;
    let read = loop {
        // SAFETY: header points at live buffers sized above.
        let read = unsafe { libc::recvmsg(socket, &mut header, libc::MSG_CMSG_CLOEXEC) };
        if read >= 0 {
            break read as usize;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    // SAFETY: the kernel filled `header` for this control buffer; each
    // SCM_RIGHTS payload holds descriptors this process now owns.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&header);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let bytes = (*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                for index in 0..bytes / std::mem::size_of::<RawFd>() {
                    drop(OwnedFd::from_raw_fd(data.add(index).read_unaligned()));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&header, cmsg);
        }
    }
    Ok(read)
}
