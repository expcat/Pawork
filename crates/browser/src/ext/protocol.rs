//! Frame encoding shared by the native messaging host (stdio and socket)
//! and the Desktop client: u32 little-endian length prefix + UTF-8 JSON,
//! the same shape Chrome uses for native messaging itself.

use std::io::{self, Read, Write};

use serde_json::Value;

/// Hard cap for a single frame in either direction. Chrome allows up to
/// 1 MiB from the host to the extension; read results stay far below and
/// screenshots are capped at 512 KiB of JPEG before base64.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Encode and write one frame.
pub fn write_frame(mut writer: impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bridge frame exceeds 1 MiB cap",
        ));
    }
    writer.write_all(&(body.len() as u32).to_le_bytes())?;
    writer.write_all(&body)?;
    writer.flush()
}

/// Read one frame. Returns `Ok(None)` on clean EOF.
pub fn read_frame(mut reader: impl Read) -> io::Result<Option<Value>> {
    let mut prefix = [0u8; 4];
    let mut filled = 0;
    while filled < prefix.len() {
        match reader.read(&mut prefix[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated frame prefix",
                ));
            }
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    let length = u32::from_le_bytes(prefix) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bridge frame length out of range",
        ));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Process-unique bridge generation: `<pid>-<startup nanos>`.
pub fn process_generation() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{}-{}", std::process::id(), nanos, sequence)
}

/// Wrap a numeric browser tab id in a generation-bound handle. Handles
/// die with the bridge process that issued them; a restarted bridge
/// rejects every old handle.
pub fn wrap_tab_handle(generation: &str, tab_id: i64) -> String {
    format!("xt-{generation}-{tab_id}")
}

/// Unwrap a tab handle for `generation`. `None` means malformed or stale.
pub fn unwrap_tab_handle(generation: &str, handle: &str) -> Option<i64> {
    let rest = handle.strip_prefix("xt-")?;
    let (prefix, tab) = rest.rsplit_once('-')?;
    if prefix != generation {
        return None;
    }
    tab.parse::<i64>().ok()
}

/// Generate a hex authentication token from the OS entropy pool.
pub fn generate_token() -> io::Result<String> {
    use std::io::Read;
    let mut source = std::fs::File::open("/dev/urandom")?;
    let mut bytes = [0u8; 32];
    source.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frames_round_trip_and_oversize_is_rejected() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, &json!({"kind": "hello"})).unwrap();
        let decoded = read_frame(buffer.as_slice()).unwrap().unwrap();
        assert_eq!(decoded["kind"], "hello");
        assert!(read_frame(&buffer[..2]).is_err());
        let oversize = serde_json::Value::String("x".repeat(MAX_FRAME_BYTES + 8));
        assert!(write_frame(&mut Vec::new(), &oversize).is_err());
    }

    #[test]
    fn tab_handles_bind_to_the_issuing_generation() {
        let generation = process_generation();
        let handle = wrap_tab_handle(&generation, 42);
        assert!(handle.starts_with("xt-"));
        assert_eq!(unwrap_tab_handle(&generation, &handle), Some(42));
        let other = process_generation();
        assert_eq!(unwrap_tab_handle(&other, &handle), None);
        assert_eq!(unwrap_tab_handle(&generation, "tab-42"), None);
        assert_eq!(unwrap_tab_handle(&generation, "xt-bogus"), None);
    }
}
