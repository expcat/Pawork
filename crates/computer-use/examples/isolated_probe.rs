//! Manual isolated acceptance helper. Every JSON line is one requested call.
//! cargo run -p pawork-computer-use --example isolated_probe -- /tmp/shots [scope]
//!
//! A bare action uses the scope argument (default manual-isolated-probe):
//! {"action":"status"}, then {"action":"screenshot"}. Inputs use the returned id.
//! An envelope picks a scope, releases it, or cancels mid-call:
//! {"scope":"run-a","release":true}
//! {"scope":"run-a","cancel_after_ms":40,"action":{"action":"screenshot"}}
//! {"op":"conflict_during_type","owner":"run-a","other":"run-b","observation_id":"...","chars":80,"pause_ms":20}
//! {"op":"release_during_type","owner":"run-a","observation_id":"...","chars":80,"pause_ms":20,"release_after_ms":200}
//! When the output argument is an existing directory, each JPEG is written there.
use pawork_computer_use::{Action, Computer, Output};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

static SHOTS: AtomicU64 = AtomicU64::new(1);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let image_path = std::env::args_os()
        .nth(1)
        .ok_or("provide a screenshot output path")?;
    let default_scope = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "manual-isolated-probe".to_string());
    let computer = Arc::new(Computer::isolated());
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let started = Instant::now();
        let outcome = match serde_json::from_str::<Value>(&line) {
            Ok(value) => dispatch(&computer, Path::new(&image_path), &default_scope, &value),
            Err(error) => json!({"ok": false, "error": error.to_string()}),
        };
        let mut outcome = outcome;
        if let Some(object) = outcome.as_object_mut() {
            object.insert(
                "elapsed_ms".into(),
                json!(started.elapsed().as_millis() as u64),
            );
        }
        println!("{outcome}");
        std::io::stdout().flush()?;
    }
    Ok(())
}

fn dispatch(
    computer: &Arc<Computer>,
    image_path: &Path,
    default_scope: &str,
    value: &Value,
) -> Value {
    if let Some(op) = value.get("op").and_then(Value::as_str) {
        return match op {
            "conflict_during_type" => conflict_during_type(computer, image_path, value),
            "release_during_type" => release_during_type(computer, image_path, value),
            _ => json!({"ok": false, "error": format!("unknown op {op}")}),
        };
    }
    let scope = value
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or(default_scope)
        .to_string();
    if value.get("release") == Some(&Value::Bool(true)) {
        computer.release(&scope);
        return json!({"ok": true, "released": scope});
    }
    let action_value = if value.get("action").and_then(Value::as_str).is_some() {
        value.clone()
    } else if let Some(action) = value.get("action") {
        action.clone()
    } else {
        return json!({"ok": false, "scope": scope, "error": "missing action"});
    };
    let action: Action = match serde_json::from_value(action_value) {
        Ok(action) => action,
        Err(error) => return json!({"ok": false, "scope": scope, "error": error.to_string()}),
    };
    let cancel_after_ms = value.get("cancel_after_ms").and_then(Value::as_u64);
    let cancel_now = value.get("cancel") == Some(&Value::Bool(true));
    let cancel = Arc::new(AtomicBool::new(cancel_now));
    if let Some(ms) = cancel_after_ms {
        let cancel = Arc::clone(&cancel);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(ms));
            cancel.store(true, Ordering::SeqCst);
        });
    }
    let started = Instant::now();
    match computer.execute(&scope, action, &|| cancel.load(Ordering::SeqCst)) {
        Ok(output) => success(image_path, &scope, started.elapsed(), output),
        Err(error) => json!({
            "ok": false,
            "scope": scope,
            "call_ms": started.elapsed().as_millis() as u64,
            "error": error.to_string(),
        }),
    }
}

fn conflict_during_type(computer: &Arc<Computer>, image_path: &Path, value: &Value) -> Value {
    let owner = text_field(value, "owner");
    let other = text_field(value, "other");
    let observation_id = text_field(value, "observation_id");
    let (chars, pause_ms) = pace(value);
    let owner_done = Arc::new(AtomicBool::new(false));
    let worker = {
        let computer = Arc::clone(computer);
        let owner_done = Arc::clone(&owner_done);
        let owner = owner.clone();
        thread::spawn(move || {
            let started = Instant::now();
            let result = run_paced_type(&computer, &owner, &observation_id, chars, pause_ms);
            owner_done.store(true, Ordering::SeqCst);
            (started.elapsed(), result)
        })
    };
    thread::sleep(Duration::from_millis(pause_ms.saturating_mul(2).max(30)));
    let other_started = Instant::now();
    let other_result = computer.execute(&other, status_action(), &|| false);
    let other_ms = other_started.elapsed().as_millis() as u64;
    let owner_running = !owner_done.load(Ordering::SeqCst);
    let (owner_elapsed, owner_result) = worker.join().expect("owner type thread");
    let owner_record = match owner_result {
        Ok(output) => success(image_path, &owner, owner_elapsed, output),
        Err(error) => json!({
            "ok": false,
            "scope": owner,
            "call_ms": owner_elapsed.as_millis() as u64,
            "error": error.to_string(),
        }),
    };
    let other_error = other_result.err().map(|error| error.to_string());
    let expected_conflict =
        format!("isolated desktop is owned by another run ({owner}); it must finish or release it");
    let other_conflict = other_error.as_deref() == Some(expected_conflict.as_str());
    let observation_id = owner_record
        .pointer("/observation/observation_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    json!({
        "ok": owner_running
            && other_conflict
            && flag(&owner_record, "ok")
            && flag(&owner_record, "input_dispatched"),
        "owner": owner,
        "other": other,
        "owner_running_during_other": owner_running,
        "other_ms": other_ms,
        "other_error": other_error,
        "other_conflict": other_conflict,
        "owner_result": owner_record,
        "observation_id": observation_id,
    })
}

fn release_during_type(computer: &Arc<Computer>, image_path: &Path, value: &Value) -> Value {
    let owner = text_field(value, "owner");
    let observation_id = text_field(value, "observation_id");
    let (chars, pause_ms) = pace(value);
    let release_after_ms = value
        .get("release_after_ms")
        .and_then(Value::as_u64)
        .unwrap_or(200);
    let owner_done = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let worker = {
        let computer = Arc::clone(computer);
        let owner_done = Arc::clone(&owner_done);
        let owner = owner.clone();
        thread::spawn(move || {
            let result = run_paced_type(&computer, &owner, &observation_id, chars, pause_ms);
            owner_done.store(true, Ordering::SeqCst);
            result
        })
    };
    thread::sleep(Duration::from_millis(release_after_ms));
    let still_running = !owner_done.load(Ordering::SeqCst);
    computer.release(&owner);
    let owner_result = worker.join().expect("owner type thread");
    let owner_ms = started.elapsed().as_millis() as u64;
    let (owner_error, owner_record) = match owner_result {
        Ok(output) => (
            None,
            Some(success(
                image_path,
                &owner,
                Duration::from_millis(owner_ms),
                output,
            )),
        ),
        Err(error) => (Some(error.to_string()), None),
    };
    let cancelled = owner_error.as_deref() == Some("computer action cancelled");
    json!({
        "ok": still_running && cancelled,
        "owner": owner,
        "released_while_running": still_running,
        "owner_ms": owner_ms,
        "owner_error": owner_error,
        "owner_result": owner_record,
        "input_prefix_proven": false,
        "rfb_proves": "in-flight release only; the first dispatched character is not proven on this desktop",
    })
}

fn run_paced_type(
    computer: &Computer,
    scope: &str,
    observation_id: &str,
    chars: usize,
    pause_ms: u64,
) -> Result<Output, pawork_computer_use::Error> {
    let action = Action::TypeText {
        observation_id: observation_id.to_string(),
        text: "A".repeat(chars),
    };
    computer.execute(scope, action, &|| {
        thread::sleep(Duration::from_millis(pause_ms));
        false
    })
}

fn status_action() -> Action {
    Action::Status {}
}

fn pace(value: &Value) -> (usize, u64) {
    let chars = value.get("chars").and_then(Value::as_u64).unwrap_or(80) as usize;
    let pause_ms = value.get("pause_ms").and_then(Value::as_u64).unwrap_or(15);
    (chars.max(1), pause_ms)
}

fn text_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn success(image_path: &Path, scope: &str, elapsed: Duration, output: Output) -> Value {
    let jpeg = if let Some(bytes) = output.jpeg.as_deref() {
        match store_jpeg(image_path, bytes) {
            Ok(path) => json!({"path": path.display().to_string(), "bytes": bytes.len()}),
            Err(error) => {
                return json!({
                    "ok": false,
                    "scope": scope,
                    "call_ms": elapsed.as_millis() as u64,
                    "error": format!("jpeg write failed: {error}"),
                    "observation": output.observation,
                    "permissions": output.permissions,
                    "input_dispatched": output.input_dispatched,
                    "observation_failure": output.observation_failure,
                });
            }
        }
    } else {
        Value::Null
    };
    json!({
        "ok": true,
        "scope": scope,
        "call_ms": elapsed.as_millis() as u64,
        "observation": output.observation,
        "permissions": output.permissions,
        "input_dispatched": output.input_dispatched,
        "observation_failure": output.observation_failure,
        "jpeg": jpeg,
    })
}

fn flag(value: &Value, key: &str) -> bool {
    matches!(value.get(key), Some(Value::Bool(true)))
}

fn store_jpeg(image_path: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    if image_path.is_dir() {
        let path = image_path.join(format!("{:04}.jpg", SHOTS.fetch_add(1, Ordering::SeqCst)));
        std::fs::write(&path, bytes)?;
        Ok(path)
    } else {
        if let Some(parent) = image_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(image_path, bytes)?;
        Ok(image_path.to_path_buf())
    }
}
