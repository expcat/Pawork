//! CU-15 bridge integration: drive the real `pawork_browser_bridge` binary
//! with a scripted fake extension on native messaging stdio while the real
//! Desktop-side client talks to its socket. Chrome itself is the only
//! mocked boundary.
#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const EXTENSION_ORIGIN: &str = "chrome-extension://hgalnnlhankacinahkjmflinpobpanjh/";
/// Stand-in shared with the app tool test. Not a Chrome or Edge capture.
const FIXTURE_JPEG_BASE64: &str = "/9j/2Q==";
const SECRET_MARKER: &str = "s3cret-value";

fn write_frame(mut writer: impl Write, message: &Value) {
    let body = serde_json::to_vec(message).unwrap();
    writer
        .write_all(&(body.len() as u32).to_le_bytes())
        .unwrap();
    writer.write_all(&body).unwrap();
    writer.flush().unwrap();
}

fn read_frame(mut reader: impl Read) -> Option<Value> {
    let mut prefix = [0u8; 4];
    reader.read_exact(&mut prefix).ok()?;
    let length = u32::from_le_bytes(prefix) as usize;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScreenshotScript {
    NotActive,
    Minimized,
    Fixture,
    Oversize,
}

impl Default for ScreenshotScript {
    fn default() -> Self {
        Self::NotActive
    }
}

#[derive(Clone)]
struct FakeElement {
    selector: &'static str,
    tag: &'static str,
    input_type: &'static str,
    text: &'static str,
}

#[derive(Default)]
struct FakeState {
    seen: Vec<(String, Value)>,
    /// sol 审查⑥：归属按任务隔离——task -> 该任务创建的标签集合。
    owned: HashMap<String, HashSet<i64>>,
    /// navigate 是否模拟重定向逃逸（最终落在其它 origin）。
    redirect: bool,
    /// 已创建的 Agent 标签（tabs 列表需要如实列出）。
    created: Option<i64>,
    /// 各标签的当前 URL。11/12 只在从未导航时回初始地址；成功 navigate（含用户标签、含重定向）覆盖它。
    urls: HashMap<i64, String>,
    /// 页面元素替身。空表时 click / type 与 sw.js 一样找不到可见元素。
    elements: Vec<FakeElement>,
    /// 置位后，已绑定标签的 DOM 动作和 navigate 预检返回 site_changed。
    drift_origin: Option<String>,
    /// 默认拒绝截图：这个 harness 没有窗口，不能冒充真实像素。
    screenshot: ScreenshotScript,
    clicks: Vec<String>,
    typed: Vec<(String, String)>,
}

fn initial_url(id: i64) -> &'static str {
    match id {
        11 => "https://a.test/one",
        12 => "https://a.test/two",
        _ => "https://a.test/agent",
    }
}

fn current_url(state: &FakeState, id: i64) -> String {
    state
        .urls
        .get(&id)
        .cloned()
        .unwrap_or_else(|| initial_url(id).to_string())
}

/// sw.js readPage: tag plus ':' plus the input type, e.g. input:text.
fn element_kind(element: &FakeElement) -> String {
    let tag = element.tag.to_ascii_lowercase();
    if element.input_type.is_empty() {
        tag
    } else {
        format!("{tag}:{}", element.input_type)
    }
}

fn extension_error(request: &Value, error: impl Into<String>) -> Value {
    json!({
        "kind": "response",
        "id": request["id"],
        "ok": false,
        "error": error.into(),
    })
}

fn fake_extension(request: &Value, state: &mut FakeState) -> Value {
    state.seen.push((
        request["action"].as_str().unwrap_or("").to_string(),
        request["params"].clone(),
    ));
    let params = &request["params"];
    let task = params["task"].as_str().unwrap_or_default().to_string();
    let owned_now = state.owned.get(&task).cloned().unwrap_or_default();
    let data = match request["action"].as_str().unwrap_or("") {
        "tabs" => {
            let mut tabs = vec![
                json!({"id": 11, "url": current_url(state, 11), "title": "One", "active": true, "owned": owned_now.contains(&11)}),
                json!({"id": 12, "url": current_url(state, 12), "title": "Two", "active": false, "owned": owned_now.contains(&12)}),
            ];
            if let Some(id) = state.created {
                tabs.push(json!({"id": id, "url": current_url(state, id), "title": "Agent", "active": false, "owned": owned_now.contains(&id)}));
            }
            json!({"tabs": tabs})
        }
        "tab_state" => {
            let id = params["tab"].as_i64().unwrap_or(11);
            json!({
                "id": params["tab"],
                "url": current_url(state, id),
                "title": "One",
                "active": true,
                "status": "complete",
            })
        }
        "read" => {
            if let Some(actual) = state.drift_origin.clone() {
                return extension_error(request, format!("site_changed: {actual}"));
            }
            let tab = params["tab"].as_i64().unwrap_or(-1);
            json!({
                "url": current_url(state, tab),
                "title": "One",
                "text": "hello page",
                "inputs": state
                    .elements
                    .iter()
                    .filter(|element| element.tag == "INPUT" || element.tag == "TEXTAREA")
                    .map(|element| json!({
                        "selector": element.selector,
                        "kind": element_kind(element),
                        "name": "",
                        "placeholder": "",
                        "value_preview": "",
                    }))
                    .collect::<Vec<_>>(),
                "buttons": state
                    .elements
                    .iter()
                    .filter(|element| element.tag == "BUTTON")
                    .map(|element| json!({"selector": element.selector, "text": element.text}))
                    .collect::<Vec<_>>(),
            })
        }
        "navigate" => {
            // 对齐真实 sw.js：未绑定路径（require_owned）必须过本任务
            // 归属检查，别的任务建 / 绑定的标签不能借道。
            if params["require_owned"].as_bool().unwrap_or(false)
                && !owned_now.contains(&params["tab"].as_i64().unwrap_or(-1))
            {
                return extension_error(
                    request,
                    "not agent-owned tab: attach it before navigating",
                );
            }
            if params.get("bound_origin").and_then(Value::as_str).is_some() {
                if let Some(actual) = state.drift_origin.clone() {
                    return extension_error(request, format!("site_changed: {actual}"));
                }
            }
            let landed = if state.redirect {
                "https://b.test/escaped".to_string()
            } else {
                params["url"]
                    .as_str()
                    .unwrap_or("https://a.test/after")
                    .to_string()
            };
            let tab = params["tab"].as_i64().unwrap_or(-1);
            state.urls.insert(tab, landed.clone());
            json!({
                "tab": params["tab"],
                "url": landed,
                "title": "After",
                "status": "complete",
            })
        }
        "new_tab" => {
            let url = params["url"]
                .as_str()
                .unwrap_or("https://a.test/agent")
                .to_string();
            state.created = Some(77);
            state.urls.insert(77, url.clone());
            state.owned.entry(task).or_default().insert(77);
            json!({"tab": 77, "url": url, "title": "Agent"})
        }
        "click" | "type" | "screenshot" => {
            let outcome = match request["action"].as_str().unwrap_or("") {
                "click" => dom_click(request, state),
                "type" => dom_type(request, state),
                _ => dom_screenshot(request, state),
            };
            if outcome["kind"] == "response" {
                return outcome;
            }
            outcome
        }
        "close_tab" => {
            let id = params["tab"].as_i64().unwrap_or(-1);
            if !owned_now.contains(&id) {
                return extension_error(
                    request,
                    "not agent-owned: the agent may only close tabs this task created",
                );
            }
            let url = current_url(state, id);
            state.owned.get_mut(&task).unwrap().remove(&id);
            if state.created == Some(id) {
                state.created = None;
            }
            state.urls.remove(&id);
            json!({"closed": params["tab"], "url": url})
        }
        "close_owned" => {
            let closed: Vec<i64> = owned_now.iter().copied().collect();
            state.owned.remove(&task);
            if state.created.is_some_and(|id| closed.contains(&id)) {
                if let Some(id) = state.created {
                    state.urls.remove(&id);
                }
                state.created = None;
            }
            json!({"closed": closed})
        }
        _ => json!({}),
    };
    json!({"kind": "response", "id": request["id"], "ok": true, "data": data})
}

fn site_drift(request: &Value, state: &FakeState) -> Option<Value> {
    state
        .drift_origin
        .clone()
        .map(|actual| extension_error(request, format!("site_changed: {actual}")))
}

fn dom_click(request: &Value, state: &mut FakeState) -> Value {
    if let Some(error) = site_drift(request, state) {
        return error;
    }
    let selector = request["params"]["selector"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let Some(element) = state
        .elements
        .iter()
        .find(|element| element.selector == selector)
        .cloned()
    else {
        return extension_error(request, format!("no visible element matches {selector}"));
    };
    if element.tag == "INPUT" && element.input_type == "password" {
        return extension_error(request, "password input clicks are not allowed");
    }
    if element.tag == "INPUT" && element.input_type == "file" {
        return extension_error(request, "file input clicks are not allowed");
    }
    state.clicks.push(selector.clone());
    json!({"clicked": selector, "tag": element.tag, "text": element.text})
}

fn dom_type(request: &Value, state: &mut FakeState) -> Value {
    if let Some(error) = site_drift(request, state) {
        return error;
    }
    let params = &request["params"];
    let selector = params["selector"].as_str().unwrap_or("").to_string();
    let text = params["text"].as_str().unwrap_or("").to_string();
    let Some(element) = state
        .elements
        .iter()
        .find(|element| element.selector == selector)
        .cloned()
    else {
        return extension_error(request, format!("no visible element matches {selector}"));
    };
    if element.tag != "INPUT" && element.tag != "TEXTAREA" {
        return extension_error(
            request,
            format!(
                "selector targets a {}; only inputs and textareas can receive text",
                element.tag
            ),
        );
    }
    if element.input_type == "password" {
        return extension_error(request, "password fields are not allowed");
    }
    if element.input_type == "file" {
        return extension_error(request, "file fields are not allowed");
    }
    state.typed.push((selector.clone(), text.clone()));
    json!({"typed": selector, "tag": element.tag, "value_length": text.encode_utf16().count()})
}

fn dom_screenshot(request: &Value, state: &mut FakeState) -> Value {
    if let Some(error) = site_drift(request, state) {
        return error;
    }
    let url = current_url(state, request["params"]["tab"].as_i64().unwrap_or(-1));
    match state.screenshot {
        ScreenshotScript::NotActive => extension_error(
            request,
            "tab is not the active tab of its window; make it visible first (no window focus change is allowed)",
        ),
        ScreenshotScript::Minimized => {
            extension_error(request, "browser window is minimized; screenshots need a visible window")
        }
        ScreenshotScript::Fixture => json!({
            "jpeg_base64": FIXTURE_JPEG_BASE64,
            "url": url,
            "title": "One",
            "synthetic": true,
        }),
        ScreenshotScript::Oversize => json!({
            "jpeg_base64": "A".repeat(683 * 1024 + 1),
            "url": url,
            "title": "One",
        }),
    }
}

struct Bridge {
    child: Child,
    data_dir: std::path::PathBuf,
    stdin: Arc<Mutex<std::process::ChildStdin>>,
    state: Arc<Mutex<FakeState>>,
    _stdout_frames: Receiver<Value>,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Bridge {
    fn send_extension(&self, message: &Value) {
        write_frame(&mut *self.stdin.lock().unwrap(), message);
    }

    fn seen_params(&self, action: &str) -> Option<Value> {
        self.state
            .lock()
            .unwrap()
            .seen
            .iter()
            .rev()
            .find(|(name, _)| name == action)
            .map(|(_, params)| params.clone())
    }
}

fn spawn_bridge(hello_enabled: bool) -> Bridge {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_path_buf();
    std::mem::forget(dir);
    spawn_bridge_at(&data_dir, hello_enabled)
}

fn spawn_bridge_at(data_dir: &Path, hello_enabled: bool) -> Bridge {
    let data_dir = data_dir.to_path_buf();
    let binary = env!("CARGO_BIN_EXE_pawork_browser_bridge");
    let mut child = Command::new(binary)
        .arg(EXTENSION_ORIGIN)
        .env("PAWORK_DATA_DIR", &data_dir)
        .env("PAWORK_BRIDGE_FORCE_BROWSER", "chrome")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("bridge binary spawns");
    let stdin = Arc::new(Mutex::new(child.stdin.take().unwrap()));
    let stdout = child.stdout.take().unwrap();
    let state = Arc::new(Mutex::new(FakeState::default()));
    let (frame_tx, frame_rx) = mpsc::channel::<Value>();
    {
        let stdin = Arc::clone(&stdin);
        let state = Arc::clone(&state);
        std::thread::spawn(move || {
            let mut stdout = stdout;
            while let Some(frame) = read_frame(&mut stdout) {
                if frame["kind"] == "request" {
                    let response = fake_extension(&frame, &mut state.lock().unwrap());
                    write_frame(&mut *stdin.lock().unwrap(), &response);
                } else if frame["kind"] == "ping" {
                    let _ = frame_tx.send(frame);
                }
            }
        });
    }
    let bridge = Bridge {
        child,
        data_dir,
        stdin,
        state,
        _stdout_frames: frame_rx,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let pid = bridge.child.id();
    while Instant::now() < deadline {
        let published = pawork_browser::ext::discovery::scan_discoveries(&bridge.data_dir)
            .iter()
            .any(|entry| entry.pid == pid);
        if published {
            bridge.send_extension(&json!({
                "kind": "hello",
                "browser": "chrome",
                "extension": "1.0.0-test",
                "enabled": hello_enabled,
            }));
            return bridge;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("bridge never published its discovery entry");
}

fn execute(bridge: &Bridge, action: Value) -> Result<Value, String> {
    let mut action = action;
    // Host 工具盖章的任务作用域（sol 审查⑥）：缺失时桥必须拒绝。
    action["task"] = json!("task-a");
    pawork_browser::ext::client::execute(&bridge.data_dir, &action)
}

fn execute_as(bridge: &Bridge, action: Value, task: &str) -> Result<Value, String> {
    let mut action = action;
    action["task"] = json!(task);
    pawork_browser::ext::client::execute(&bridge.data_dir, &action)
}

#[test]
fn bridge_relay_binds_origins_and_gates_actions() {
    let bridge = spawn_bridge(true);
    let tabs = execute(&bridge, json!({"action": "ext_tabs"})).unwrap();
    assert_eq!(tabs["browser"], "chrome");
    assert!(tabs["generation"]
        .as_str()
        .unwrap()
        .starts_with(&format!("{}-", bridge.child.id())));
    let first = tabs["tabs"][0].clone();
    let handle = first["tab_id"].as_str().unwrap().to_string();
    assert!(handle.starts_with("xt-"));
    assert_eq!(first["origin"], "https://a.test");
    assert_eq!(first["agent_owned"], false);
    assert_eq!(first["attached"], false);
    // 桥把任务作用域透传给扩展（fake 按 task 记归属）。
    assert_eq!(bridge.seen_params("tabs").unwrap()["task"], "task-a");

    let attached = execute(
        &bridge,
        json!({"action": "ext_attach", "tab_id": handle, "origin": "https://a.test"}),
    )
    .unwrap();
    assert_eq!(attached["origin"], "https://a.test");
    assert_eq!(attached["tab_id"].as_str(), Some(handle.as_str()));
    let mismatch = execute(
        &bridge,
        json!({"action": "ext_attach", "tab_id": handle, "origin": "https://other.test"}),
    )
    .unwrap_err();
    assert!(mismatch.contains("origin mismatch"), "{mismatch}");

    let read = execute(&bridge, json!({"action": "ext_read", "tab_id": handle})).unwrap();
    assert_eq!(read["text"], "hello page");
    let read_params = bridge.seen_params("read").unwrap();
    assert_eq!(read_params["expect_origin"], "https://a.test");
    assert_eq!(read_params["tab"], 11);

    let stale = execute(
        &bridge,
        json!({"action": "ext_read", "tab_id": "xt-999-1-1-11"}),
    )
    .unwrap_err();
    assert!(stale.contains("unknown tab handle"), "{stale}");

    let cross = execute(
        &bridge,
        json!({"action": "ext_navigate", "tab_id": handle, "url": "https://other.test/x"}),
    )
    .unwrap_err();
    assert!(cross.contains("cross-origin navigation"), "{cross}");
    // Same-origin navigation is dispatched and keeps the binding.
    let nav = execute(
        &bridge,
        json!({"action": "ext_navigate", "tab_id": handle, "url": "https://a.test/after"}),
    )
    .unwrap();
    assert_eq!(nav["url"], "https://a.test/after");
    assert!(nav.get("site_changed").is_none());
    // sol 审查③：绑定 origin 必须随 navigate 下发（扩展侧预检用）。
    let nav_params = bridge.seen_params("navigate").unwrap();
    assert_eq!(nav_params["bound_origin"], "https://a.test");
    let read_after = execute(&bridge, json!({"action": "ext_read", "tab_id": handle})).unwrap();
    assert_eq!(read_after["text"], "hello page");

    // close_tab on a user tab is refused by the extension side.
    let close = execute(
        &bridge,
        json!({"action": "ext_close_tab", "tab_id": handle}),
    )
    .unwrap_err();
    assert!(close.contains("not agent-owned"), "{close}");

    // Agent-created tabs close normally; ownership comes from the extension.
    let created = execute(
        &bridge,
        json!({"action": "ext_new_tab", "url": "https://a.test/agent"}),
    )
    .unwrap();
    let agent_handle = created["tab_id"].as_str().unwrap().to_string();
    assert!(agent_handle.starts_with("xt-"));
    let closed_agent = execute(
        &bridge,
        json!({"action": "ext_close_tab", "tab_id": agent_handle}),
    )
    .unwrap();
    assert_eq!(closed_agent["closed"].as_str(), Some(agent_handle.as_str()));

    // close reports only handles the bridge itself issued（再建一个标签，
    // close 只回收本任务归属的它）。
    execute(
        &bridge,
        json!({"action": "ext_new_tab", "url": "https://a.test/agent2"}),
    )
    .unwrap();
    let closed = execute(&bridge, json!({"action": "ext_close"})).unwrap();
    assert_eq!(closed["closed"].as_array().unwrap().len(), 1);
    assert!(closed["closed"][0].as_str().unwrap().starts_with("xt-"));

    assert!(execute(&bridge, json!({"action": "ext_evil"})).is_err());

    // 缺任务作用域的动作被桥拒绝。
    let mut no_task = json!({"action": "ext_tabs"});
    if let Some(object) = no_task.as_object_mut() {
        object.remove("task");
    }
    assert!(
        pawork_browser::ext::client::execute(&bridge.data_dir, &no_task)
            .unwrap_err()
            .contains("missing task scope")
    );
}

/// sol 审查④：未绑定（Agent 自建）标签首次 navigate 到 A 后重定向逃逸
/// 到 B 时，必须拒绝且不绑定——不能把审批范围悄悄扩大到 B。
#[test]
fn bridge_refuses_redirect_escape_on_first_navigation() {
    let bridge = spawn_bridge(true);
    // 无 URL 的 new_tab 只建归属、不绑定。
    let created = execute(&bridge, json!({"action": "ext_new_tab"})).unwrap();
    let handle = created["tab_id"].as_str().unwrap().to_string();
    // 未绑定时 read 被拒（attach 前置）。
    assert!(
        execute(&bridge, json!({"action": "ext_read", "tab_id": handle}))
            .unwrap_err()
            .contains("not attached")
    );

    bridge.state.lock().unwrap().redirect = true;
    let escaped = execute(
        &bridge,
        json!({"action": "ext_navigate", "tab_id": handle, "url": "https://a.test/first"}),
    )
    .unwrap_err();
    assert!(escaped.contains("ended on https://b.test"), "{escaped}");
    assert!(
        escaped.contains("instead of the requested https://a.test"),
        "{escaped}"
    );
    // 未绑定：逃逸后的 read 仍被拒。
    assert!(
        execute(&bridge, json!({"action": "ext_read", "tab_id": handle}))
            .unwrap_err()
            .contains("not attached")
    );

    // 落回请求 origin 时正常绑定。
    bridge.state.lock().unwrap().redirect = false;
    let nav = execute(
        &bridge,
        json!({"action": "ext_navigate", "tab_id": handle, "url": "https://a.test/first"}),
    )
    .unwrap();
    assert_eq!(nav["origin"], "https://a.test");
    let read = execute(&bridge, json!({"action": "ext_read", "tab_id": handle})).unwrap();
    assert_eq!(read["text"], "hello page");
}

/// sol 审查⑥：归属按任务隔离——任务 B 不能关任务 A 建的标签，
/// close 也只清理发起任务自己的标签。
#[test]
fn bridge_ownership_is_task_scoped() {
    let bridge = spawn_bridge(true);
    let created = execute_as(
        &bridge,
        json!({"action": "ext_new_tab", "url": "https://a.test/agent-a"}),
        "task-a",
    )
    .unwrap();
    let handle = created["tab_id"].as_str().unwrap().to_string();

    // 任务 B 的 tabs 视角里该标签不属于自己。
    let tabs_b = execute_as(&bridge, json!({"action": "ext_tabs"}), "task-b").unwrap();
    let entry_b = tabs_b["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["url"] == "https://a.test/agent-a")
        .unwrap();
    assert_eq!(entry_b["agent_owned"], false);

    // 任务 B 不能关任务 A 的标签。
    let refused = execute_as(
        &bridge,
        json!({"action": "ext_close_tab", "tab_id": handle}),
        "task-b",
    )
    .unwrap_err();
    assert!(refused.contains("not agent-owned"), "{refused}");

    // 任务 B 的 close 清理不了任务 A 的标签。
    let closed_b = execute_as(&bridge, json!({"action": "ext_close"}), "task-b").unwrap();
    assert_eq!(closed_b["closed"].as_array().unwrap().len(), 0);
    // 标签仍在：任务 A 的 tabs 视角里它仍归属自己。
    let tabs_a = execute_as(&bridge, json!({"action": "ext_tabs"}), "task-a").unwrap();
    let entry_a = tabs_a["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["url"] == "https://a.test/agent-a")
        .unwrap();
    assert_eq!(entry_a["agent_owned"], true);

    // 任务 A 自己可以关。
    let closed = execute_as(
        &bridge,
        json!({"action": "ext_close_tab", "tab_id": handle}),
        "task-a",
    )
    .unwrap();
    assert_eq!(closed["closed"].as_str(), Some(handle.as_str()));
}

/// sol 复审②：站点绑定按 (task, tab) 隔离——任务 A attach 后，任务 B
/// 不附带自己的 attach 就不能 read；navigate 也借不到 A 的绑定跳过
/// 本任务归属检查。
#[test]
fn bridge_bindings_are_task_scoped() {
    let bridge = spawn_bridge(true);
    let tabs = execute_as(&bridge, json!({"action": "ext_tabs"}), "task-a").unwrap();
    let handle = tabs["tabs"][0]["tab_id"].as_str().unwrap().to_string();

    execute_as(
        &bridge,
        json!({"action": "ext_attach", "tab_id": handle, "origin": "https://a.test"}),
        "task-a",
    )
    .unwrap();

    // 任务 B 无本任务绑定：read 拒绝。
    let read_b = execute_as(
        &bridge,
        json!({"action": "ext_read", "tab_id": handle}),
        "task-b",
    )
    .unwrap_err();
    assert!(read_b.contains("not attached"), "{read_b}");

    // 任务 B 的 navigate 走未绑定路径，必须过本任务归属检查（tab 11
    // 是用户标签，B 未建也未 attach）。
    let nav_b = execute_as(
        &bridge,
        json!({"action": "ext_navigate", "tab_id": handle, "url": "https://a.test/b"}),
        "task-b",
    )
    .unwrap_err();
    assert!(nav_b.contains("not agent-owned"), "{nav_b}");

    // tabs 视角的 attached 同样按任务：B 看到未绑定。
    let tabs_b = execute_as(&bridge, json!({"action": "ext_tabs"}), "task-b").unwrap();
    let entry_b = tabs_b["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["url"] == "https://a.test/one")
        .unwrap();
    assert_eq!(entry_b["attached"], false);

    // 任务 A 的绑定不受 B 的失败动作影响。
    let read_a = execute_as(
        &bridge,
        json!({"action": "ext_read", "tab_id": handle}),
        "task-a",
    )
    .unwrap();
    assert_eq!(read_a["text"], "hello page");

    // 任务 B 自己 attach 后可以 read：拒绝来自作用域隔离，不是故障。
    execute_as(
        &bridge,
        json!({"action": "ext_attach", "tab_id": handle, "origin": "https://a.test"}),
        "task-b",
    )
    .unwrap();
    let read_b_after = execute_as(
        &bridge,
        json!({"action": "ext_read", "tab_id": handle}),
        "task-b",
    )
    .unwrap();
    assert_eq!(read_b_after["text"], "hello page");
}

#[test]
fn bridge_requires_authentication_and_enabled_extension() {
    // No extension hello yet: authentication is still checkable first.
    let bridge = spawn_bridge(false);
    let discovery = pawork_browser::ext::discovery::scan_discoveries(&bridge.data_dir)
        .pop()
        .unwrap();
    let mut stream = std::os::unix::net::UnixStream::connect(discovery.socket_path()).unwrap();
    write_frame(
        &mut stream,
        &json!({"kind": "hello", "token": "wrong-token", "client": "probe"}),
    );
    let reply = read_frame(&mut stream).expect("hello reply");
    assert_eq!(reply["ok"], false);
    // Extension reported disabled: the client surfaces the enable guidance
    // without dispatching the action.
    let disabled = execute(&bridge, json!({"action": "ext_tabs"})).unwrap_err();
    assert!(disabled.contains("not enabled"), "{disabled}");
}

fn install_form(bridge: &Bridge) {
    bridge.state.lock().unwrap().elements = vec![
        FakeElement {
            selector: "#q",
            tag: "INPUT",
            input_type: "text",
            text: "",
        },
        FakeElement {
            selector: "#go",
            tag: "BUTTON",
            input_type: "",
            text: "Go",
        },
        FakeElement {
            selector: "#pw",
            tag: "INPUT",
            input_type: "password",
            text: "",
        },
    ];
}

fn redact_value(value: &Value) -> Value {
    match value {
        Value::String(text) if text.contains(SECRET_MARKER) => json!("<redacted>"),
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        Value::Object(map) => {
            let mut redacted = serde_json::Map::new();
            for (key, item) in map {
                if key == "token" {
                    let len = item.as_str().map(|text| text.len()).unwrap_or(0);
                    redacted.insert(key.clone(), json!(format!("<redacted len={len}>")));
                } else {
                    redacted.insert(key.clone(), redact_value(item));
                }
            }
            Value::Object(redacted)
        }
        other => other.clone(),
    }
}

struct AcceptanceLog {
    path: Option<std::path::PathBuf>,
    steps: Vec<Value>,
}

impl AcceptanceLog {
    fn new(stem: &str) -> Self {
        let path = std::env::var("PAWORK_CU15_ACCEPTANCE_EVIDENCE_DIR")
            .ok()
            .map(|dir| dir.trim().to_string())
            .filter(|dir| !dir.is_empty())
            .map(|dir| {
                let dir = std::path::PathBuf::from(dir);
                std::fs::create_dir_all(&dir).unwrap();
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_nanos())
                    .unwrap_or(0);
                dir.join(format!("{stem}-{nanos}.json"))
            });
        let log = Self {
            path,
            steps: Vec::new(),
        };
        log.flush();
        if let Some(path) = &log.path {
            eprintln!("cu15-evidence {}", path.display());
        }
        log
    }

    fn push(&mut self, value: Value) {
        self.steps.push(redact_value(&value));
        self.flush();
    }

    fn flush(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let body = serde_json::to_vec_pretty(&json!({
            "layer": "simulated-manual-acceptance",
            "real_chrome_or_edge": false,
            "gpui_started": false,
            "entry": "pawork_browser::ext::client::execute",
            "desktop_route_note": "same function DesktopController::external_browser_request calls after the ext_ prefix branch",
            "screenshot_note": "fixture /9j/2Q== is not DOM pixels; activation and navigation races remain pending on real Chrome",
            "steps": self.steps,
        }))
        .unwrap();
        std::fs::write(path, body).unwrap();
    }
}

fn run_step(
    log: &mut AcceptanceLog,
    bridge: &Bridge,
    name: &str,
    action: Value,
    task: &str,
) -> Result<Value, String> {
    let before = bridge.state.lock().unwrap().seen.len();
    let result = execute_as(bridge, action, task);
    let seen = bridge.state.lock().unwrap().seen.clone();
    let extension_call = if seen.len() > before {
        seen.last().cloned().map(|(action, params)| {
            json!({
                "action": action,
                "params": params,
            })
        })
    } else {
        None
    };
    log.push(json!({
        "step": name,
        "task": task,
        "ok": result.is_ok(),
        "result": match &result {
            Ok(value) => value.clone(),
            Err(error) => json!(error),
        },
        "extension_call": extension_call,
    }));
    result
}

fn urls_of(tabs: &Value) -> Vec<String> {
    tabs["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tab| tab["url"].as_str().unwrap().to_string())
        .collect()
}

/// Simulated manual acceptance: real bridge binary, real Desktop client,
/// fake extension. Not a Chrome/Edge session and not a GPUI window.
#[test]
fn simulated_acceptance_runs_tab_flow_and_unbinds_on_site_change() {
    let mut log = AcceptanceLog::new("flow");
    let bridge = spawn_bridge(true);
    let tabs = run_step(
        &mut log,
        &bridge,
        "tabs",
        json!({"action": "ext_tabs"}),
        "task-a",
    )
    .unwrap();
    assert_eq!(tabs["browser"], "chrome");
    let user = tabs["tabs"][0]["tab_id"].as_str().unwrap().to_string();
    assert_eq!(tabs["tabs"][0]["agent_owned"], false);
    assert_eq!(tabs["tabs"][0]["attached"], false);
    assert_eq!(tabs["tabs"][1]["url"], "https://a.test/two");

    let created = run_step(
        &mut log,
        &bridge,
        "new_tab",
        json!({"action": "ext_new_tab", "url": "https://a.test/form"}),
        "task-a",
    )
    .unwrap();
    let agent = created["tab_id"].as_str().unwrap().to_string();
    assert!(agent.starts_with("xt-"));
    assert_eq!(created["origin"], "https://a.test");
    assert_eq!(created["url"], "https://a.test/form");

    let nav = run_step(
        &mut log,
        &bridge,
        "navigate",
        json!({"action": "ext_navigate", "tab_id": agent, "url": "https://a.test/after"}),
        "task-a",
    )
    .unwrap();
    assert_eq!(nav["url"], "https://a.test/after");
    assert!(nav.get("site_changed").is_none());
    assert_eq!(
        bridge.seen_params("navigate").unwrap()["bound_origin"],
        "https://a.test"
    );

    install_form(&bridge);
    log.push(json!({"step": "install-form-elements", "selectors": ["#q", "#go", "#pw"]}));
    let read = run_step(
        &mut log,
        &bridge,
        "read",
        json!({"action": "ext_read", "tab_id": agent}),
        "task-a",
    )
    .unwrap();
    assert_eq!(read["text"], "hello page");
    assert_eq!(read["url"], "https://a.test/after");
    assert_eq!(read["inputs"][0]["selector"], "#q");
    assert_eq!(read["inputs"][0]["kind"], "input:text");
    assert!(read["inputs"][0].get("type").is_none());
    assert_eq!(read["buttons"][0]["selector"], "#go");
    assert_eq!(read["buttons"][0]["text"], "Go");
    assert_eq!(
        bridge.seen_params("read").unwrap()["expect_origin"],
        "https://a.test"
    );

    let typed = run_step(
        &mut log,
        &bridge,
        "type",
        json!({"action": "ext_type", "tab_id": agent, "selector": "#q", "text": "hello"}),
        "task-a",
    )
    .unwrap();
    assert_eq!(typed["typed"], "#q");
    assert_eq!(typed["value_length"], 5);
    assert_eq!(
        bridge.seen_params("type").unwrap()["expect_origin"],
        "https://a.test"
    );
    let password = run_step(
        &mut log,
        &bridge,
        "type-password",
        json!({"action": "ext_type", "tab_id": agent, "selector": "#pw", "text": SECRET_MARKER}),
        "task-a",
    )
    .unwrap_err();
    assert!(password.contains("password fields"), "{password}");
    assert!(!password.contains(SECRET_MARKER));

    let clicked = run_step(
        &mut log,
        &bridge,
        "click",
        json!({"action": "ext_click", "tab_id": agent, "selector": "#go"}),
        "task-a",
    )
    .unwrap();
    assert_eq!(clicked["clicked"], "#go");
    assert_eq!(clicked["tag"], "BUTTON");
    assert_eq!(
        bridge.seen_params("click").unwrap()["expect_origin"],
        "https://a.test"
    );
    let password_click = run_step(
        &mut log,
        &bridge,
        "click-password",
        json!({"action": "ext_click", "tab_id": agent, "selector": "#pw"}),
        "task-a",
    )
    .unwrap_err();
    assert!(
        password_click.contains("password input clicks"),
        "{password_click}"
    );
    assert_eq!(
        bridge.state.lock().unwrap().typed,
        vec![("#q".to_string(), "hello".to_string())]
    );
    assert_eq!(bridge.state.lock().unwrap().clicks, vec!["#go".to_string()]);

    let inactive = run_step(
        &mut log,
        &bridge,
        "screenshot-not-active",
        json!({"action": "ext_screenshot", "tab_id": agent}),
        "task-a",
    )
    .unwrap_err();
    assert!(inactive.contains("not the active tab"), "{inactive}");
    bridge.state.lock().unwrap().screenshot = ScreenshotScript::Minimized;
    let minimized = run_step(
        &mut log,
        &bridge,
        "screenshot-minimized",
        json!({"action": "ext_screenshot", "tab_id": agent}),
        "task-a",
    )
    .unwrap_err();
    assert!(minimized.contains("minimized"), "{minimized}");
    bridge.state.lock().unwrap().screenshot = ScreenshotScript::Fixture;
    let shot = run_step(
        &mut log,
        &bridge,
        "screenshot-fixture",
        json!({"action": "ext_screenshot", "tab_id": agent}),
        "task-a",
    )
    .unwrap();
    assert_eq!(shot["jpeg_base64"], FIXTURE_JPEG_BASE64);
    assert_eq!(shot["url"], "https://a.test/after");
    assert_eq!(shot["synthetic"], true);
    assert!(shot["jpeg_base64"].as_str().unwrap().len() < 32);
    bridge.state.lock().unwrap().screenshot = ScreenshotScript::Oversize;
    let oversize = run_step(
        &mut log,
        &bridge,
        "screenshot-oversize",
        json!({"action": "ext_screenshot", "tab_id": agent}),
        "task-a",
    )
    .unwrap_err();
    assert!(oversize.contains("size budget"), "{oversize}");

    let tabs_b = run_step(
        &mut log,
        &bridge,
        "tabs-other-task",
        json!({"action": "ext_tabs"}),
        "task-b",
    )
    .unwrap();
    let foreign = tabs_b["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["tab_id"] == agent)
        .unwrap();
    assert_eq!(foreign["url"], "https://a.test/after");
    assert_eq!(foreign["agent_owned"], false);
    let refused = run_step(
        &mut log,
        &bridge,
        "close-tab-other-task",
        json!({"action": "ext_close_tab", "tab_id": agent}),
        "task-b",
    )
    .unwrap_err();
    assert!(refused.contains("not agent-owned"), "{refused}");
    let closed_b = run_step(
        &mut log,
        &bridge,
        "close-other-task",
        json!({"action": "ext_close"}),
        "task-b",
    )
    .unwrap();
    assert_eq!(closed_b["closed"].as_array().unwrap().len(), 0);

    let closed_agent = run_step(
        &mut log,
        &bridge,
        "close-tab-owner",
        json!({"action": "ext_close_tab", "tab_id": agent}),
        "task-a",
    )
    .unwrap();
    assert_eq!(closed_agent["closed"], agent);
    assert_eq!(closed_agent["url"], "https://a.test/after");
    let after = run_step(
        &mut log,
        &bridge,
        "tabs-after-agent-close",
        json!({"action": "ext_tabs"}),
        "task-a",
    )
    .unwrap();
    let after_urls = urls_of(&after);
    assert!(after_urls.iter().any(|url| url == "https://a.test/one"));
    assert!(after_urls.iter().any(|url| url == "https://a.test/two"));
    assert!(!after_urls.iter().any(|url| url.contains("/form")));
    let closed_none = run_step(
        &mut log,
        &bridge,
        "close-after-agent-tab-gone",
        json!({"action": "ext_close"}),
        "task-a",
    )
    .unwrap();
    assert_eq!(closed_none["closed"].as_array().unwrap().len(), 0);
    let user_refused = run_step(
        &mut log,
        &bridge,
        "close-user-tab",
        json!({"action": "ext_close_tab", "tab_id": user}),
        "task-a",
    )
    .unwrap_err();
    assert!(user_refused.contains("not agent-owned"), "{user_refused}");

    let again = run_step(
        &mut log,
        &bridge,
        "new-tab-for-close",
        json!({"action": "ext_new_tab", "url": "https://a.test/agent"}),
        "task-a",
    )
    .unwrap();
    let closed = run_step(
        &mut log,
        &bridge,
        "close-owned",
        json!({"action": "ext_close"}),
        "task-a",
    )
    .unwrap();
    assert_eq!(closed["closed"].as_array().unwrap().len(), 1);
    assert_eq!(closed["closed"][0], again["tab_id"]);
    let still = run_step(
        &mut log,
        &bridge,
        "tabs-user-remains",
        json!({"action": "ext_tabs"}),
        "task-a",
    )
    .unwrap();
    let still_urls = urls_of(&still);
    assert!(still_urls.iter().any(|url| url == "https://a.test/one"));
    assert!(still_urls.iter().any(|url| url == "https://a.test/two"));
    assert!(!still_urls.iter().any(|url| url.contains("/agent")));

    run_step(
        &mut log,
        &bridge,
        "attach-task-a",
        json!({"action": "ext_attach", "tab_id": user, "origin": "https://a.test"}),
        "task-a",
    )
    .unwrap();
    run_step(
        &mut log,
        &bridge,
        "attach-task-b",
        json!({"action": "ext_attach", "tab_id": user, "origin": "https://a.test"}),
        "task-b",
    )
    .unwrap();
    bridge.state.lock().unwrap().drift_origin = Some("https://b.test".into());
    let drifted = run_step(
        &mut log,
        &bridge,
        "read-site-changed",
        json!({"action": "ext_read", "tab_id": user}),
        "task-a",
    )
    .unwrap_err();
    assert!(drifted.contains("attach the new origin again"), "{drifted}");
    let other_unbound = run_step(
        &mut log,
        &bridge,
        "read-other-task-unbound",
        json!({"action": "ext_read", "tab_id": user}),
        "task-b",
    )
    .unwrap_err();
    assert!(other_unbound.contains("not attached"), "{other_unbound}");

    bridge.state.lock().unwrap().drift_origin = None;
    run_step(
        &mut log,
        &bridge,
        "reattach-before-redirect",
        json!({"action": "ext_attach", "tab_id": user, "origin": "https://a.test"}),
        "task-a",
    )
    .unwrap();
    bridge.state.lock().unwrap().redirect = true;
    let escaped = run_step(
        &mut log,
        &bridge,
        "navigate-redirect-unbinds",
        json!({"action": "ext_navigate", "tab_id": user, "url": "https://a.test/stay"}),
        "task-a",
    )
    .unwrap();
    assert_eq!(escaped["site_changed"], true);
    assert_eq!(escaped["origin"], "https://b.test");
    assert_eq!(escaped["url"], "https://b.test/escaped");
    let after_escape = run_step(
        &mut log,
        &bridge,
        "read-after-redirect",
        json!({"action": "ext_read", "tab_id": user}),
        "task-a",
    )
    .unwrap_err();
    assert!(after_escape.contains("not attached"), "{after_escape}");

    // 关掉 redirect 开关并不会把标签送回原站。真实扩展仍停在 b.test，
    // 用旧的 a.test 授权重新 attach 必须 origin mismatch。
    bridge.state.lock().unwrap().redirect = false;
    let stale_origin = run_step(
        &mut log,
        &bridge,
        "reattach-old-origin-after-redirect",
        json!({"action": "ext_attach", "tab_id": user, "origin": "https://a.test"}),
        "task-a",
    )
    .unwrap_err();
    assert!(stale_origin.contains("origin mismatch"), "{stale_origin}");
    assert!(stale_origin.contains("https://b.test"), "{stale_origin}");

    // 用户标签不是本任务创建的，解绑后客户端不能再把它导航回去。
    // 后续 a.test 场景要继续，就显式让 fixture 回到该站并记下来。
    let returned_tab = bridge.seen_params("tab_state").unwrap()["tab"]
        .as_i64()
        .unwrap();
    bridge
        .state
        .lock()
        .unwrap()
        .urls
        .insert(returned_tab, "https://a.test/one".to_string());
    log.push(json!({
        "step": "fixture-return-to-a-test",
        "tab": returned_tab,
        "url": "https://a.test/one",
        "note": "redirect left the user tab on https://b.test/escaped; the fixture is set back to a.test before the later same-origin steps",
    }));
    let rebound = run_step(
        &mut log,
        &bridge,
        "reattach-before-cross-origin",
        json!({"action": "ext_attach", "tab_id": user, "origin": "https://a.test"}),
        "task-a",
    )
    .unwrap();
    assert_eq!(rebound["url"], "https://a.test/one");
    let before_cross = bridge.state.lock().unwrap().seen.len();
    let cross = run_step(
        &mut log,
        &bridge,
        "navigate-cross-origin",
        json!({"action": "ext_navigate", "tab_id": user, "url": "https://other.test/x"}),
        "task-a",
    )
    .unwrap_err();
    assert!(cross.contains("cross-origin"), "{cross}");
    assert_eq!(bridge.state.lock().unwrap().seen.len(), before_cross);
    let still_bound = run_step(
        &mut log,
        &bridge,
        "read-binding-kept",
        json!({"action": "ext_read", "tab_id": user}),
        "task-a",
    )
    .unwrap();
    assert_eq!(still_bound["text"], "hello page");
    let actions = bridge
        .state
        .lock()
        .unwrap()
        .seen
        .iter()
        .map(|(action, _)| action.clone())
        .collect::<Vec<_>>();
    log.push(json!({"step": "extension-sequence", "actions": actions}));
}

/// Kill the bridge, prove the published socket/token die with it, then a
/// new bridge in the same discovery directory rejects the old handle.
#[test]
fn bridge_disconnect_republishes_socket_and_rejects_old_handles() {
    let mut log = AcceptanceLog::new("restart");
    let mut bridge = spawn_bridge(true);
    let data_dir = bridge.data_dir.clone();
    let tabs = execute(&bridge, json!({"action": "ext_tabs"})).unwrap();
    let handle = tabs["tabs"][0]["tab_id"].as_str().unwrap().to_string();
    let generation = tabs["generation"].as_str().unwrap().to_string();
    let pid = bridge.child.id();
    let discovery = pawork_browser::ext::discovery::scan_discoveries(&data_dir)
        .into_iter()
        .find(|entry| entry.pid == pid)
        .unwrap();
    let entry_path = data_dir
        .join("external-browser")
        .join(format!("chrome-{pid}.json"));
    let file_mode = std::fs::metadata(&entry_path).unwrap().permissions().mode() & 0o777;
    let dir_mode = std::fs::metadata(data_dir.join("external-browser"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600);
    assert_eq!(dir_mode, 0o700);
    assert_eq!(discovery.token.len(), 64);
    assert!(Path::new(&discovery.socket).exists());
    let old_token = discovery.token.clone();
    log.push(json!({
        "step": "socket-published",
        "pid": pid,
        "generation": generation,
        "socket": discovery.socket,
        "token_len": discovery.token.len(),
        "file_mode": format!("{file_mode:o}"),
        "dir_mode": format!("{dir_mode:o}"),
    }));

    let mut stream = std::os::unix::net::UnixStream::connect(discovery.socket_path()).unwrap();
    write_frame(
        &mut stream,
        &json!({"kind": "hello", "token": "wrong-token", "client": "probe"}),
    );
    let reply = read_frame(&mut stream).expect("hello reply");
    assert_eq!(reply["ok"], false);
    log.push(json!({"step": "wrong-token", "ok": false}));
    let still = execute(&bridge, json!({"action": "ext_tabs"})).unwrap();
    assert_eq!(still["generation"], generation);

    bridge.child.kill().unwrap();
    let status = bridge.child.wait().unwrap();
    assert_eq!(status.signal(), Some(9));
    drop(bridge);
    let dead = pawork_browser::ext::client::execute(
        &data_dir,
        &json!({"action": "ext_tabs", "task": "task-a"}),
    )
    .unwrap_err();
    assert!(
        dead.contains("not enabled") || dead.contains("not connected"),
        "{dead}"
    );
    log.push(json!({
        "step": "bridge-killed",
        "signal": status.signal(),
        "client_error": dead,
    }));

    let bridge = spawn_bridge_at(&data_dir, true);
    let stale = execute(&bridge, json!({"action": "ext_read", "tab_id": handle})).unwrap_err();
    assert!(stale.contains("unknown tab handle"), "{stale}");
    let renewed = execute(&bridge, json!({"action": "ext_tabs"})).unwrap();
    let new_generation = renewed["generation"].as_str().unwrap().to_string();
    assert_ne!(new_generation, generation);
    let renewed_discovery = pawork_browser::ext::discovery::scan_discoveries(&data_dir)
        .into_iter()
        .find(|entry| entry.pid == bridge.child.id())
        .unwrap();
    assert_ne!(renewed_discovery.pid, pid);
    assert_eq!(renewed_discovery.token.len(), 64);
    assert!(old_token != renewed_discovery.token);
    log.push(json!({
        "step": "reconnected",
        "old_handle_rejected": stale,
        "new_pid": renewed_discovery.pid,
        "new_generation": new_generation,
        "new_socket": renewed_discovery.socket,
        "token_len": renewed_discovery.token.len(),
        "token_changed": true,
    }));
}
