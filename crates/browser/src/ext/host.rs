//! Native messaging bridge host. Chrome / Edge spawn this process per
//! extension connection; it serves a unix socket for Desktop clients,
//! relays fixed-function actions to the extension and enforces the site
//! binding and ownership gates that the approval chain authorized.
//!
//! Trust model: the socket is same-user only and authenticated with a
//! per-process token published in the discovery directory. The extension
//! side is our own fixed code spawned by the browser. The bridge never
//! evaluates arbitrary scripts: it forwards a closed action vocabulary.
#![cfg(unix)]

use std::collections::HashMap;
use std::io::{self, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::discovery::{self, BridgeDiscovery};
use super::origin::normalize_origin;
use super::protocol::{self, wrap_tab_handle};
use super::BrowserKind;

const EXTENSION_ORIGIN: &str = "chrome-extension://hgalnnlhankacinahkjmflinpobpanjh/";
const EXTENSION_WAIT: Duration = Duration::from_secs(20);
const CLIENT_HELLO_WAIT: Duration = Duration::from_secs(2);
const CLIENT_REQUEST_WAIT: Duration = Duration::from_secs(25);
const MAX_READ_JSON_BYTES: usize = 64 * 1024;
const MAX_SCREENSHOT_BASE64_BYTES: usize = 683 * 1024;

pub struct HostConfig {
    pub browser: BrowserKind,
    pub data_dir: std::path::PathBuf,
}

struct Shared {
    browser: BrowserKind,
    generation: String,
    token: String,
    ext_ready: AtomicBool,
    ext_enabled: AtomicBool,
    extension_version: Mutex<String>,
    /// Site bindings keyed by (task, tab id) -> bound origin: a tab is
    /// actionable only for the task that attached or first-navigated it.
    /// Lives exactly as long as this bridge process; a restarted bridge
    /// rejects all old handles.
    bound: Mutex<HashMap<(String, i64), String>>,
    /// Serializes extension calls: one outstanding request at a time.
    ext_call: Mutex<()>,
    pending: Mutex<Option<(String, SyncSender<Value>)>>,
}

impl Shared {
    fn bind(&self, task: &str, tab: i64, origin: String) {
        self.bound
            .lock()
            .unwrap()
            .insert((task.to_string(), tab), origin);
    }

    /// Tab-level unbind: a site change or close invalidates every task's
    /// binding on that tab, not just the caller's.
    fn unbind_tab(&self, tab: i64) {
        self.bound.lock().unwrap().retain(|(_, id), _| *id != tab);
    }

    fn bound_origin(&self, task: &str, tab: i64) -> Option<String> {
        self.bound
            .lock()
            .unwrap()
            .get(&(task.to_string(), tab))
            .cloned()
    }
}

/// Run the bridge until the browser closes the native messaging port.
/// `extension_origin` is the argv the browser passed us; it must match the
/// pinned extension id so another extension cannot use this host.
pub fn serve(config: HostConfig, extension_origin: Option<&str>) -> io::Result<()> {
    if extension_origin != Some(EXTENSION_ORIGIN) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "native host invoked by an unexpected extension origin",
        ));
    }
    let generation = protocol::process_generation();
    let token = protocol::generate_token()?;
    let socket_path = bridge_socket_path(&config.data_dir, config.browser)?;
    std::fs::create_dir_all(discovery::bridge_dir(&config.data_dir))?;
    let listener = UnixListener::bind(&socket_path)?;
    let shared = Arc::new(Shared {
        browser: config.browser,
        generation: generation.clone(),
        token: token.clone(),
        ext_ready: AtomicBool::new(false),
        ext_enabled: AtomicBool::new(false),
        extension_version: Mutex::new(String::new()),
        bound: Mutex::new(HashMap::new()),
        ext_call: Mutex::new(()),
        pending: Mutex::new(None),
    });
    let discovery = BridgeDiscovery {
        browser: config.browser,
        pid: std::process::id(),
        generation: generation.clone(),
        socket: socket_path.to_string_lossy().into_owned(),
        token,
    };
    discovery::write_discovery(&config.data_dir, &discovery)?;
    let running = Arc::new(AtomicBool::new(true));
    // MV3 service workers are reclaimed after ~30 s without events; an
    // inbound port message resets that timer, so an idle bridge emits a
    // heartbeat to keep the extension (and therefore this process) alive.
    {
        let shared = Arc::clone(&shared);
        let running = Arc::clone(&running);
        std::thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(15));
                if shared.pending.lock().unwrap().is_none() {
                    let stdout = io::stdout();
                    let mut handle = stdout.lock();
                    let _ = protocol::write_frame(&mut handle, &json!({"kind": "ping"}));
                }
            }
        });
    }
    // Extension reader: stdin frames until the browser closes the port.
    {
        let shared = Arc::clone(&shared);
        let running = Arc::clone(&running);
        std::thread::spawn(move || {
            let stdin = io::stdin();
            let mut reader = stdin.lock();
            loop {
                match protocol::read_frame(&mut reader) {
                    Ok(Some(message)) => {
                        if let Err(error) = handle_extension_message(&shared, &message) {
                            eprintln!("pawork-bridge: bad extension frame: {error}");
                        }
                    }
                    _ => break,
                }
            }
            shared.ext_ready.store(false, Ordering::SeqCst);
            running.store(false, Ordering::SeqCst);
        });
    }
    // Main thread becomes the accept loop; non-blocking so stdin EOF
    // (browser closed the port) shuts the whole process down promptly.
    listener.set_nonblocking(true)?;
    while running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                // macOS accepted sockets inherit the listener's
                // O_NONBLOCK; the request loop relies on read timeouts,
                // so restore blocking mode explicitly.
                let _ = stream.set_nonblocking(false);
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || serve_client(shared, stream));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => break,
        }
    }
    discovery::remove_discovery(&config.data_dir, &discovery);
    Ok(())
}

fn bridge_socket_path(
    data_dir: &std::path::Path,
    browser: BrowserKind,
) -> io::Result<std::path::PathBuf> {
    let preferred = discovery::bridge_dir(data_dir).join(format!(
        "{}-{}.sock",
        browser.name(),
        std::process::id()
    ));
    if preferred.as_os_str().len() < 100 {
        return Ok(preferred);
    }
    // Keep inside the sockaddr_un sun_path budget when the data dir is deep.
    Ok(std::env::temp_dir().join(format!(
        "pawork-bridge-{}-{}.sock",
        browser.name(),
        std::process::id()
    )))
}

fn handle_extension_message(shared: &Shared, message: &Value) -> Result<(), String> {
    match message["kind"].as_str() {
        Some("hello") => {
            let browser = message["browser"]
                .as_str()
                .and_then(BrowserKind::parse)
                .ok_or("hello without a known browser")?;
            if browser != shared.browser {
                return Err(format!(
                    "extension reports {} but this bridge serves {}",
                    browser.display(),
                    shared.browser.display()
                ));
            }
            if let Some(version) = message["extension"].as_str() {
                *shared.extension_version.lock().unwrap() = version.to_string();
            }
            shared.ext_enabled.store(
                message["enabled"].as_bool().unwrap_or(false),
                Ordering::SeqCst,
            );
            shared.ext_ready.store(true, Ordering::SeqCst);
            Ok(())
        }
        Some("status") => {
            shared.ext_enabled.store(
                message["enabled"].as_bool().unwrap_or(false),
                Ordering::SeqCst,
            );
            Ok(())
        }
        Some("response") => {
            let id = message["id"]
                .as_str()
                .ok_or("response without id")?
                .to_string();
            let pending = shared.pending.lock().unwrap().take();
            if let Some((pending_id, sender)) = pending {
                if pending_id == id {
                    let _ = sender.send(message.clone());
                } else {
                    *shared.pending.lock().unwrap() = Some((pending_id, sender));
                }
            }
            Ok(())
        }
        other => Err(format!("unknown frame {other:?}")),
    }
}

fn serve_client(shared: Arc<Shared>, stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(CLIENT_HELLO_WAIT));
    let _ = stream.set_write_timeout(Some(CLIENT_HELLO_WAIT));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let authenticated = loop {
        match protocol::read_frame(&mut reader) {
            Ok(Some(message)) if message["kind"] == "hello" => {
                break message["token"].as_str() == Some(&shared.token);
            }
            _ => return,
        }
    };
    let hello = if authenticated {
        let extension = shared.extension_version.lock().unwrap().clone();
        json!({
            "kind": "hello",
            "ok": true,
            "browser": shared.browser.name(),
            "generation": shared.generation,
            "extension": extension,
            "enabled": shared.ext_enabled.load(Ordering::SeqCst),
        })
    } else {
        json!({"kind": "hello", "ok": false, "error": "authentication failed"})
    };
    if protocol::write_frame(&mut writer, &hello).is_err() {
        return;
    }
    if !authenticated {
        return;
    }
    // Authenticated clients may hold the connection open between actions;
    // give requests the full broker budget.
    let raw = reader.into_inner();
    let _ = raw.set_read_timeout(Some(CLIENT_REQUEST_WAIT));
    let mut reader = BufReader::new(raw);
    loop {
        match protocol::read_frame(&mut reader) {
            Ok(Some(message)) => {
                let id = message["id"].as_str().unwrap_or_default().to_string();
                let result = if message["kind"] == "request" {
                    handle_client_request(
                        &shared,
                        message["action"].as_str().unwrap_or(""),
                        message.get("params").unwrap_or(&Value::Null),
                    )
                } else {
                    Err("expected a request frame".to_string())
                };
                let frame = match result {
                    Ok(data) => json!({"kind": "response", "id": id, "ok": true, "data": data}),
                    Err(error) => {
                        json!({"kind": "response", "id": id, "ok": false, "error": error})
                    }
                };
                if protocol::write_frame(&mut writer, &frame).is_err() {
                    return;
                }
            }
            _ => return,
        }
    }
}

fn extension_gate(shared: &Shared) -> Result<(), String> {
    if !shared.ext_ready.load(Ordering::SeqCst) {
        return Err("extension is not connected; reconnect it from the toolbar popup".into());
    }
    if !shared.ext_enabled.load(Ordering::SeqCst) {
        return Err(
            "bridge is disabled; enable Pawork control in the extension toolbar popup".into(),
        );
    }
    Ok(())
}

fn call_extension(shared: &Shared, action: &str, params: Value) -> Result<Value, String> {
    let _guard = shared.ext_call.lock().unwrap();
    // sol 审查⑨：enabled 复查放在取得串行锁之后——排队期间用户关闭
    // 开关时，不得再派发。扩展侧 onHostMessage 还有第二道复查。
    extension_gate(shared)?;
    let id = format!(
        "r{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    );
    let (sender, receiver) = mpsc::sync_channel(1);
    *shared.pending.lock().unwrap() = Some((id.clone(), sender));
    let request = json!({"kind": "request", "id": id, "action": action, "params": params});
    {
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        if let Err(error) = protocol::write_frame(&mut handle, &request) {
            *shared.pending.lock().unwrap() = None;
            return Err(format!("bridge could not reach the extension: {error}"));
        }
    }
    match receiver.recv_timeout(EXTENSION_WAIT) {
        Ok(response) => {
            if response["ok"] == true {
                Ok(response.get("data").cloned().unwrap_or(Value::Null))
            } else {
                Err(response["error"]
                    .as_str()
                    .unwrap_or("extension action failed")
                    .to_string())
            }
        }
        Err(_) => {
            *shared.pending.lock().unwrap() = None;
            Err("extension did not respond in time".into())
        }
    }
}

/// Tab-scoped extension call that also keeps bridge binding honest:
/// site_changed / tab_gone failures unbind the tab before surfacing.
fn call_tab_scoped(
    shared: &Shared,
    tab: i64,
    action: &str,
    params: Value,
) -> Result<Value, String> {
    call_extension(shared, action, params).map_err(|error| {
        if error.starts_with("site_changed:") {
            shared.unbind_tab(tab);
            "the tab navigated to another origin; attach the new origin again".to_string()
        } else if error.starts_with("tab_gone:") {
            shared.unbind_tab(tab);
            "tab was closed".to_string()
        } else {
            error
        }
    })
}

fn handle_client_request(shared: &Shared, action: &str, params: &Value) -> Result<Value, String> {
    let unwrap = |shared: &Shared| -> Result<i64, String> {
        let handle = params["tab_id"].as_str().ok_or("missing tab_id")?;
        protocol::unwrap_tab_handle(&shared.generation, handle).ok_or_else(|| {
            "unknown tab handle; the bridge restarted or never issued it".to_string()
        })
    };
    // sol 审查⑥：任务作用域由 Host 工具盖章（run_id），Desktop 只原样
    // 中继；归属（谁建的标签 / 谁能关）按任务隔离，不是整浏览器全局。
    let task = params["task"]
        .as_str()
        .filter(|task| !task.is_empty() && task.len() <= 128)
        .ok_or("missing task scope")?;
    match action {
        "ext_tabs" => {
            let data = call_extension(shared, "tabs", json!({"task": task}))?;
            let mut tabs = Vec::new();
            if let Some(entries) = data["tabs"].as_array() {
                for entry in entries {
                    let id = entry["id"].as_i64().unwrap_or_default();
                    let origin = entry["url"]
                        .as_str()
                        .and_then(|url| normalize_origin(url).ok());
                    // sol 复审②：attached 视角按任务——别的任务 attach
                    // 过不代表本任务可操作。
                    let attached = shared.bound_origin(task, id).is_some();
                    let handle = origin
                        .as_ref()
                        .map(|_| wrap_tab_handle(&shared.generation, id));
                    tabs.push(json!({
                        "tab_id": handle,
                        "url": entry["url"],
                        "title": entry["title"],
                        "origin": origin,
                        "active": entry["active"].as_bool().unwrap_or(false),
                        "agent_owned": entry["owned"].as_bool().unwrap_or(false),
                        "attached": attached,
                    }));
                }
            }
            Ok(json!({
                "browser": shared.browser.name(),
                "generation": shared.generation,
                "tabs": tabs,
            }))
        }
        "ext_attach" => {
            let tab = unwrap(shared)?;
            let requested = normalize_origin(params["origin"].as_str().ok_or("missing origin")?)
                .map_err(|error| format!("invalid origin: {error}"))?;
            let state = call_extension(shared, "tab_state", json!({"tab": tab}))?;
            let actual = state["url"]
                .as_str()
                .and_then(|url| normalize_origin(url).ok())
                .ok_or("tab has no attachable origin")?;
            if actual != requested {
                return Err(format!(
                    "origin mismatch: the tab is at {actual}, the approval covered {requested}"
                ));
            }
            shared.bind(task, tab, actual.clone());
            Ok(json!({
                "tab_id": wrap_tab_handle(&shared.generation, tab),
                "origin": actual,
                "url": state["url"],
                "title": state["title"],
            }))
        }
        "ext_new_tab" => {
            let url = params.get("url").and_then(Value::as_str);
            let origin = match url {
                Some(address) => Some(
                    normalize_origin(address).map_err(|error| format!("invalid url: {error}"))?,
                ),
                None => None,
            };
            let mut data = call_extension(shared, "new_tab", json!({ "url": url, "task": task }))?;
            let tab = data["tab"].as_i64().ok_or("extension returned no tab id")?;
            if let Some(origin) = origin {
                // Bind the declared origin immediately; a redirect escape
                // surfaces on the first tab-scoped action and unbinds.
                shared.bind(task, tab, origin.clone());
                data["origin"] = json!(origin);
            }
            data["tab_id"] = json!(wrap_tab_handle(&shared.generation, tab));
            Ok(data)
        }
        "ext_navigate" => {
            let tab = unwrap(shared)?;
            let address = params["url"].as_str().ok_or("missing url")?;
            let target =
                normalize_origin(address).map_err(|error| format!("invalid url: {error}"))?;
            // sol 复审②：绑定按 (task, tab) 查——本任务没 attach 过就
            // 走未绑定路径（require_owned 归属检查），借不到别的任务
            // 的绑定。
            let bound = shared.bound_origin(task, tab);
            if let Some(origin) = &bound {
                if *origin != target {
                    return Err(format!(
                        "cross-origin navigation is not allowed: tab is bound to {origin}, target is {target}; open a new tab instead"
                    ));
                }
            }
            // sol 审查③：把绑定 origin 传给扩展做导航前复核（此前扩展侧
            // 检查永远不执行）；走 call_tab_scoped 让 site_changed / tab_gone
            // 统一解绑。
            let mut data = call_tab_scoped(
                shared,
                tab,
                "navigate",
                json!({
                    "tab": tab,
                    "url": address,
                    "require_owned": bound.is_none(),
                    "bound_origin": bound,
                    "task": task,
                }),
            )?;
            let resolved_origin = data["url"]
                .as_str()
                .and_then(|address| normalize_origin(address).ok());
            match (bound, resolved_origin) {
                // sol 审查④：只绑定请求目标 origin；重定向逃逸到其它
                // origin 时拒绝且不绑定，不能悄悄扩大审批的网站范围。
                (None, Some(resolved)) if resolved == target => {
                    shared.bind(task, tab, target.clone());
                    data["origin"] = json!(target);
                }
                (None, Some(resolved)) => {
                    return Err(format!(
                        "navigation ended on {resolved} instead of the requested {target}; it was not attached"
                    ));
                }
                (None, None) => {
                    return Err(
                        "navigation ended without an attachable web origin; it was not attached"
                            .into(),
                    );
                }
                (Some(expected), Some(resolved)) if resolved != expected => {
                    shared.unbind_tab(tab);
                    data["site_changed"] = json!(true);
                    data["origin"] = json!(resolved);
                }
                (Some(_), None) => {
                    shared.unbind_tab(tab);
                    data["site_changed"] = json!(true);
                }
                _ => {}
            }
            data["tab_id"] = json!(wrap_tab_handle(&shared.generation, tab));
            Ok(data)
        }
        "ext_read" | "ext_click" | "ext_type" | "ext_screenshot" => {
            let tab = unwrap(shared)?;
            let Some(origin) = shared.bound_origin(task, tab) else {
                return Err("tab is not attached; attach it (and its origin) first".into());
            };
            let mut request = json!({"tab": tab, "expect_origin": origin});
            if let Some(selector) = params.get("selector").and_then(Value::as_str) {
                request["selector"] = json!(selector);
            }
            if let Some(text) = params.get("text").and_then(Value::as_str) {
                request["text"] = json!(text);
            }
            let extension_action = match action {
                "ext_read" => "read",
                "ext_click" => "click",
                "ext_type" => "type",
                _ => "screenshot",
            };
            let mut data = call_tab_scoped(shared, tab, extension_action, request)?;
            match action {
                "ext_read" => {
                    if data.to_string().len() > MAX_READ_JSON_BYTES {
                        return Err("page observation exceeds the 64 KiB budget".into());
                    }
                }
                "ext_screenshot" => {
                    let jpeg = data["jpeg_base64"].as_str().unwrap_or("");
                    if jpeg.len() > MAX_SCREENSHOT_BASE64_BYTES {
                        return Err("screenshot exceeds the size budget".into());
                    }
                }
                _ => {}
            }
            data["tab_id"] = json!(wrap_tab_handle(&shared.generation, tab));
            Ok(data)
        }
        "ext_close_tab" => {
            let tab = unwrap(shared)?;
            let data = call_extension(shared, "close_tab", json!({"tab": tab, "task": task}))?;
            shared.unbind_tab(tab);
            Ok(json!({
                "closed": params["tab_id"],
                "url": data["url"],
            }))
        }
        "ext_close" => {
            let data = call_extension(shared, "close_owned", json!({"task": task}))?;
            let mut handles = Vec::new();
            if let Some(closed) = data["closed"].as_array() {
                for id in closed.iter().filter_map(Value::as_i64) {
                    shared.unbind_tab(id);
                    handles.push(json!(wrap_tab_handle(&shared.generation, id)));
                }
            }
            Ok(json!({"closed": handles}))
        }
        other => Err(format!("unsupported action {other:?}")),
    }
}

#[cfg(target_os = "macos")]
pub fn detect_parent_browser() -> Option<BrowserKind> {
    let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let parent = unsafe { libc::getppid() };
    let length = unsafe {
        libc::proc_pidpath(
            parent,
            path.as_mut_ptr().cast::<libc::c_void>(),
            path.len() as u32,
        )
    };
    if length <= 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&path[..length as usize]).into_owned();
    if text.contains("Google Chrome.app") {
        Some(BrowserKind::Chrome)
    } else if text.contains("Microsoft Edge.app") {
        Some(BrowserKind::Edge)
    } else {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub fn detect_parent_browser() -> Option<BrowserKind> {
    None
}
