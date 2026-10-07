//! Desktop-side bridge client. Stateless per action: scan the discovery
//! directory, prove each candidate bridge alive with an authenticated
//! hello, enforce the single-enabled-browser rule, then relay one action.
//! Bindings and tab handles live in the bridge process, so reconnecting
//! Desktop (or making one-off connections) never widens authorization.
#![cfg(unix)]

use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use super::discovery::{self, BridgeDiscovery};
use super::protocol;

struct LiveBridge {
    discovery: BridgeDiscovery,
    enabled: bool,
    stream: UnixStream,
}

/// Execute one `ext_*` action against the user-selected external browser.
/// Returns the action data or a user-facing error string.
pub fn execute(data_dir: &Path, action: &Value) -> Result<Value, String> {
    let mut stream = connect_selected(data_dir)?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(25)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let id = format!(
        "c{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    );
    let request = json!({
        "kind": "request",
        "id": id,
        "action": action["action"],
        "params": strip_action(action),
    });
    protocol::write_frame(&mut stream, &request)
        .map_err(|error| format!("bridge write failed: {error}"))?;
    let response = protocol::read_frame(&mut BufReader::new(stream))
        .map_err(|error| format!("bridge read failed: {error}"))?
        .ok_or_else(|| "bridge closed the connection".to_string())?;
    if response["ok"] == true {
        return Ok(response.get("data").cloned().unwrap_or(Value::Null));
    }
    Err(response["error"]
        .as_str()
        .unwrap_or("external browser action failed")
        .to_string())
}

fn strip_action(action: &Value) -> Value {
    let mut params = action.clone();
    if let Some(object) = params.as_object_mut() {
        object.remove("action");
    }
    params
}

fn connect_selected(data_dir: &Path) -> Result<UnixStream, String> {
    let mut alive: Vec<LiveBridge> = Vec::new();
    for discovery in discovery::scan_discoveries(data_dir) {
        let Ok(mut stream) = UnixStream::connect(discovery.socket_path()) else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        let hello = json!({
            "kind": "hello",
            "token": discovery.token,
            "client": "pawork-desktop",
        });
        if protocol::write_frame(&mut stream, &hello).is_err() {
            continue;
        }
        // Only the hello reply is expected before we send a request, so
        // dropping the reader leaves no buffered frames behind.
        let reply = match protocol::read_frame(&mut BufReader::new(&stream)) {
            Ok(Some(reply)) => reply,
            _ => continue,
        };
        if reply["ok"] != true {
            continue;
        }
        if reply["generation"].as_str() != Some(discovery.generation.as_str()) {
            continue;
        }
        let enabled = reply["enabled"].as_bool().unwrap_or(false);
        alive.push(LiveBridge {
            discovery,
            enabled,
            stream,
        });
    }
    let mut enabled: Vec<LiveBridge> = alive
        .into_iter()
        .filter(|bridge| bridge.enabled)
        .collect();
    match enabled.len() {
        0 => Err(
            "external browser bridge not enabled: enable Pawork control in the extension toolbar popup".into(),
        ),
        1 => Ok(enabled.remove(0).stream),
        _ => {
            let names: Vec<&str> = enabled
                .iter()
                .map(|bridge| bridge.discovery.browser.display())
                .collect();
            Err(format!(
                "multiple external browsers are enabled ({}); keep exactly one enabled in the extension popups",
                names.join(" + ")
            ))
        }
    }
}
