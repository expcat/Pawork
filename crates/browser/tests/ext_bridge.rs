//! CU-15 bridge integration: drive the real `pawork_browser_bridge` binary
//! with a scripted fake extension on native messaging stdio while the real
//! Desktop-side client talks to its socket. Chrome itself is the only
//! mocked boundary.
#![cfg(unix)]

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const EXTENSION_ORIGIN: &str = "chrome-extension://hgalnnlhankacinahkjmflinpobpanjh/";

fn write_frame(mut writer: impl Write, message: &Value) {
    let body = serde_json::to_vec(message).unwrap();
    writer.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
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

#[derive(Default)]
struct FakeState {
    seen: Vec<(String, Value)>,
    /// sol 审查⑥：归属按任务隔离——task -> 该任务创建的标签集合。
    owned: HashMap<String, HashSet<i64>>,
    /// navigate 是否模拟重定向逃逸（最终落在其它 origin）。
    redirect: bool,
    /// 已创建的 Agent 标签（tabs 列表需要如实列出）。
    created: Option<i64>,
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
                json!({"id": 11, "url": "https://a.test/one", "title": "One", "active": true, "owned": owned_now.contains(&11)}),
                json!({"id": 12, "url": "https://a.test/two", "title": "Two", "active": false, "owned": owned_now.contains(&12)}),
            ];
            if let Some(id) = state.created {
                tabs.push(json!({"id": id, "url": "https://a.test/agent", "title": "Agent", "active": false, "owned": owned_now.contains(&id)}));
            }
            json!({"tabs": tabs})
        }
        "tab_state" => json!({
            "id": params["tab"],
            "url": "https://a.test/one",
            "title": "One",
            "active": true,
            "status": "complete",
        }),
        "read" => json!({
            "url": "https://a.test/one",
            "title": "One",
            "text": "hello page",
        }),
        "navigate" => {
            // 对齐真实 sw.js：未绑定路径（require_owned）必须过本任务
            // 归属检查，别的任务建 / 绑定的标签不能借道。
            if params["require_owned"].as_bool().unwrap_or(false)
                && !owned_now.contains(&params["tab"].as_i64().unwrap_or(-1))
            {
                return json!({
                    "kind": "response",
                    "id": request["id"],
                    "ok": false,
                    "error": "not agent-owned tab: attach it before navigating",
                });
            }
            json!({
                "tab": params["tab"],
                "url": if state.redirect { "https://b.test/escaped" } else { params["url"].as_str().unwrap_or("https://a.test/after") },
                "title": "After",
                "status": "complete",
            })
        }
        "new_tab" => {
            state.created = Some(77);
            state.owned.entry(task).or_default().insert(77);
            json!({"tab": 77, "url": "https://a.test/agent", "title": "Agent"})
        }
        "close_tab" => {
            if !owned_now.contains(&params["tab"].as_i64().unwrap_or(-1)) {
                return json!({
                    "kind": "response",
                    "id": request["id"],
                    "ok": false,
                    "error": "not agent-owned: the agent may only close tabs this task created",
                });
            }
            state.owned.get_mut(&task).unwrap().remove(&params["tab"].as_i64().unwrap_or(-1));
            json!({"closed": params["tab"], "url": "https://a.test/agent"})
        }
        "close_owned" => {
            let closed: Vec<i64> = owned_now.iter().copied().collect();
            state.owned.remove(&task);
            json!({"closed": closed})
        }
        _ => json!({}),
    };
    json!({"kind": "response", "id": request["id"], "ok": true, "data": data})
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
    while Instant::now() < deadline {
        if !pawork_browser::ext::discovery::scan_discoveries(&bridge.data_dir).is_empty() {
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
    assert!(tabs["generation"].as_str().unwrap().starts_with(&format!(
        "{}-",
        bridge.child.id()
    )));
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
    execute(&bridge, json!({"action": "ext_new_tab", "url": "https://a.test/agent2"})).unwrap();
    let closed = execute(&bridge, json!({"action": "ext_close"})).unwrap();
    assert_eq!(closed["closed"].as_array().unwrap().len(), 1);
    assert!(
        closed["closed"][0]
            .as_str()
            .unwrap()
            .starts_with("xt-")
    );

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
    assert!(execute(&bridge, json!({"action": "ext_read", "tab_id": handle}))
        .unwrap_err()
        .contains("not attached"));

    bridge.state.lock().unwrap().redirect = true;
    let escaped = execute(
        &bridge,
        json!({"action": "ext_navigate", "tab_id": handle, "url": "https://a.test/first"}),
    )
    .unwrap_err();
    assert!(escaped.contains("ended on https://b.test"), "{escaped}");
    assert!(escaped.contains("instead of the requested https://a.test"), "{escaped}");
    // 未绑定：逃逸后的 read 仍被拒。
    assert!(execute(&bridge, json!({"action": "ext_read", "tab_id": handle}))
        .unwrap_err()
        .contains("not attached"));

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
    let created = execute_as(&bridge, json!({"action": "ext_new_tab", "url": "https://a.test/agent-a"}), "task-a").unwrap();
    let handle = created["tab_id"].as_str().unwrap().to_string();

    // 任务 B 的 tabs 视角里该标签不属于自己。
    let tabs_b = execute_as(&bridge, json!({"action": "ext_tabs"}), "task-b").unwrap();
    let entry_b = tabs_b["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["url"] == "https://a.test/agent")
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
        .find(|entry| entry["url"] == "https://a.test/agent")
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
    let read_b = execute_as(&bridge, json!({"action": "ext_read", "tab_id": handle}), "task-b")
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
    let read_a = execute_as(&bridge, json!({"action": "ext_read", "tab_id": handle}), "task-a").unwrap();
    assert_eq!(read_a["text"], "hello page");

    // 任务 B 自己 attach 后可以 read：拒绝来自作用域隔离，不是故障。
    execute_as(
        &bridge,
        json!({"action": "ext_attach", "tab_id": handle, "origin": "https://a.test"}),
        "task-b",
    )
    .unwrap();
    let read_b_after =
        execute_as(&bridge, json!({"action": "ext_read", "tab_id": handle}), "task-b").unwrap();
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
