//! Element handles and observation post-processing for the fixed DOM scripts.
//!
//! Semantics mirror the CU-02 target contract: handles are bound to a page
//! generation (bumped on every provisional navigation), the observed URL and a
//! DOM revision (a `MutationObserver` counter installed by the read script).
//! Navigation, reload, SPA URL changes and DOM mutations all invalidate older
//! handles; each handle is consumed at most once and expires after 60 seconds.
//! Handles are in-memory only and never persisted or replayed.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use serde_json::Value;

use super::dom::MAX_OUTPUT_BYTES;

pub(crate) const HANDLE_TTL: Duration = Duration::from_secs(60);
const MAX_HANDLES: usize = 512;

static BOOT_NS: OnceLock<u64> = OnceLock::new();
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

fn boot_ns() -> u64 {
    *BOOT_NS.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    })
}

fn next_handle_id() -> String {
    // 进程级命名空间（pid + 进程启动纳秒 + 进程级序号）：跨进程与进程内并发
    // 都不撞键；句柄只存内存、不进持久幂等键。
    format!(
        "e-{}-{}-{}",
        std::process::id(),
        boot_ns(),
        NEXT_HANDLE.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Clone, Debug)]
pub(crate) struct HandleEntry {
    pub selector: String,
    pub url: String,
    pub revision: u64,
    pub generation: u64,
    pub consumed: bool,
    pub issued_at: Instant,
}

#[derive(Default)]
pub(crate) struct HandleTable {
    entries: HashMap<String, HandleEntry>,
    order: VecDeque<String>,
}

impl HandleTable {
    pub fn issue(
        &mut self,
        selector: &str,
        url: &str,
        revision: u64,
        generation: u64,
        now: Instant,
    ) -> String {
        while self.entries.len() >= MAX_HANDLES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
        let id = next_handle_id();
        self.entries.insert(
            id.clone(),
            HandleEntry {
                selector: selector.to_string(),
                url: url.to_string(),
                revision,
                generation,
                consumed: false,
                issued_at: now,
            },
        );
        self.order.push_back(id.clone());
        id
    }

    /// Validate and consume a handle. Consumption happens exactly when all
    /// checks pass; a failed dispatch never returns the handle for reuse.
    pub fn take_valid(
        &mut self,
        handle: &str,
        current_generation: u64,
        now: Instant,
    ) -> Result<HandleEntry, String> {
        let Some(entry) = self.entries.get_mut(handle) else {
            return Err("元素句柄未知 / Unknown element handle — read the page again".into());
        };
        if entry.consumed {
            return Err("元素句柄已使用，请重新读取 / Element handle already used — read the page again".into());
        }
        if now.duration_since(entry.issued_at) > HANDLE_TTL {
            return Err("元素句柄已过期，请重新读取 / Element handle expired — read the page again".into());
        }
        if entry.generation != current_generation {
            return Err("页面已导航，句柄失效 / Page navigated — read the page again".into());
        }
        entry.consumed = true;
        Ok(entry.clone())
    }
}

/// Attach generation/revision/viewport metadata and element handles to the
/// raw read-script output, keeping the 64 KiB output budget.
pub(crate) fn finalize_observation(
    mut data: Value,
    url: &str,
    title: &str,
    generation: u64,
    viewport: (f64, f64),
    handles: &mut HandleTable,
    now: Instant,
) -> Result<String, String> {
    let script_url = data
        .get("url")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let revision = data
        .get("dom_revision")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let mut out = super::dom::finalize_page_json(data, url, title)?;
    data = serde_json::from_str(&out)
        .map_err(|_| "无法序列化页面 / Unable to serialize page".to_string())?;
    let obj = data
        .as_object_mut()
        .ok_or_else(|| "网页脚本返回无效 / The page script returned invalid data".to_string())?;
    obj.insert("generation".into(), Value::from(generation));
    obj.insert(
        "viewport".into(),
        serde_json::json!({"width": viewport.0, "height": viewport.1}),
    );
    for key in ["links", "inputs", "buttons"] {
        let Some(list) = obj.get_mut(key).and_then(|value| value.as_array_mut()) else {
            continue;
        };
        for entry in list.iter_mut() {
            let Some(selector) = entry.get("selector").and_then(|value| value.as_str()) else {
                continue;
            };
            let handle = handles.issue(selector, &script_url, revision, generation, now);
            entry
                .as_object_mut()
                .expect("element entry")
                .insert("handle".into(), Value::String(handle));
        }
    }
    loop {
        out = serde_json::to_string(&data)
            .map_err(|_| "无法序列化页面 / Unable to serialize page".to_string())?;
        if out.len() <= MAX_OUTPUT_BYTES {
            return Ok(out);
        }
        let obj = data.as_object_mut().expect("page json object");
        // 句柄注入后仍超预算：先压 text（finalize_page_json 同款策略），
        // text 用尽后才从最长的元素列表尾部弹出，避免微量溢出误删句柄。
        let overflow = out.len() - MAX_OUTPUT_BYTES;
        if let Some(text) = obj.get("text").and_then(|value| value.as_str()) {
            if !text.is_empty() {
                let keep = text.len().saturating_sub(overflow.max(1024));
                let truncated = super::dom::truncate_to_bytes(text, keep);
                obj.insert("text".into(), Value::String(truncated));
                continue;
            }
        }
        let longest = ["links", "inputs", "buttons"]
            .into_iter()
            .filter_map(|key| {
                obj.get(key)
                    .and_then(|value| value.as_array())
                    .map(|list| (key, list.len()))
            })
            .max_by_key(|(_, len)| *len);
        match longest {
            Some((key, len)) if len > 0 => {
                obj.get_mut(key)
                    .and_then(|value| value.as_array_mut())
                    .expect("element list")
                    .pop();
            }
            _ => {
                return Err("页面内容超过 64KiB / Page snapshot exceeds 64KiB".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_lifecycle_consumes_once_and_rejects_stale() {
        let mut table = HandleTable::default();
        let now = Instant::now();
        let handle = table.issue("#ok", "https://example.com/", 3, 1, now);
        let entry = table.take_valid(&handle, 1, now).unwrap();
        assert_eq!(entry.selector, "#ok");
        assert_eq!(entry.revision, 3);
        let used = table.take_valid(&handle, 1, now).unwrap_err();
        assert!(used.contains("already used"));
        let fresh = table.issue("#ok", "https://example.com/", 3, 1, now);
        assert!(table.take_valid(&fresh, 2, now).unwrap_err().contains("navigated"));
        let expired = table.issue("#ok", "https://example.com/", 3, 2, now);
        let later = now + HANDLE_TTL + Duration::from_secs(1);
        assert!(
            table
                .take_valid(&expired, 2, later)
                .unwrap_err()
                .contains("expired")
        );
        assert!(
            table
                .take_valid("e-0-0-999", 2, now)
                .unwrap_err()
                .contains("Unknown")
        );
    }

    #[test]
    fn handle_table_evicts_oldest_at_capacity() {
        let mut table = HandleTable::default();
        let now = Instant::now();
        let first = table.issue("#first", "https://example.com/", 0, 0, now);
        let mut last = first.clone();
        for index in 0..MAX_HANDLES {
            last = table.issue(&format!("#i{index}"), "https://example.com/", 0, 0, now);
        }
        assert!(table.take_valid(&first, 0, now).unwrap_err().contains("Unknown"));
        assert!(table.take_valid(&last, 0, now).is_ok());
    }

    #[test]
    fn finalize_observation_injects_metadata_and_handles_within_budget() {
        let mut table = HandleTable::default();
        let now = Instant::now();
        let data = serde_json::json!({
            "url": "https://example.com/inpage",
            "dom_revision": 7,
            "title": "ignored",
            "text": "测".repeat(40_000),
            "links": [{"selector": "#go", "href": "https://example.com/x", "text": "go", "rect": {"x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0}}],
            "inputs": [{"selector": "#q", "type": "text", "value": "hi"}],
            "buttons": [{"selector": "#apply", "type": "button", "text": "Apply"}]
        });
        let out = finalize_observation(
            data,
            "https://example.com/state",
            "真实标题",
            9,
            (1024.0, 768.0),
            &mut table,
            now,
        )
        .unwrap();
        assert!(out.len() <= MAX_OUTPUT_BYTES, "len={}", out.len());
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["url"], "https://example.com/state");
        assert_eq!(parsed["title"], "真实标题");
        assert_eq!(parsed["generation"], 9);
        assert_eq!(parsed["dom_revision"], 7);
        assert_eq!(parsed["viewport"]["width"], 1024.0);
        let handle = parsed["links"][0]["handle"].as_str().unwrap();
        let entry = table.take_valid(handle, 9, now).unwrap();
        assert_eq!(entry.selector, "#go");
        // 句柄记录脚本侧 location.href（动作脚本按它比较），展示 URL 仍可被 state 覆盖。
        assert_eq!(entry.url, "https://example.com/inpage");
        assert!(parsed["inputs"][0]["handle"].as_str().unwrap().starts_with("e-"));
        assert!(parsed["buttons"][0]["handle"].as_str().unwrap().starts_with("e-"));
    }

    #[test]
    fn finalize_observation_drops_elements_until_handles_fit_budget() {
        let mut table = HandleTable::default();
        let now = Instant::now();
        let link = |index: usize| {
            serde_json::json!({"selector": format!("#l{index}"), "href": "https://example.com/x", "text": "t".repeat(600)})
        };
        let data = serde_json::json!({
            "url": "https://example.com/",
            "dom_revision": 0,
            "text": "",
            "links": (0..80).map(link).collect::<Vec<_>>(),
            "inputs": [],
            "buttons": []
        });
        let out = finalize_observation(
            data,
            "https://example.com/",
            "t",
            0,
            (800.0, 600.0),
            &mut table,
            now,
        )
        .unwrap();
        assert!(out.len() <= MAX_OUTPUT_BYTES, "len={}", out.len());
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let links = parsed["links"].as_array().unwrap();
        assert!(!links.is_empty());
        assert!(links.iter().all(|entry| entry["handle"].is_string()));
    }
}
