//! CU-15 external Chrome / Edge background control channel.
//!
//! Topology: the model-facing `external_browser` tool in pawork-app enqueues
//! opaque `ext_*` actions through the existing GUI 1.18 browser broker.
//! Desktop claims them, routes any `ext_` action here, and calls
//! [`client::execute`] which discovers the live bridge process, connects to
//! its unix socket and relays the action. The bridge process is spawned by
//! the browser itself through native messaging and talks to our fixed
//! function browser extension. No arbitrary script evaluation, no CDP, no
//! Node / V8 in the host or build chain.
//!
//! - `origin` normalizes website identities (CU-16 semantics).
//! - `protocol` owns frame encoding and tab-handle generations.
//! - `discovery` owns the per-user discovery directory.
//! - `host` is the native messaging host server (one per browser process).
//! - `client` is the stateless Desktop-side entry point.
pub mod client;
pub mod discovery;
pub mod host;
pub mod origin;
pub mod protocol;

/// Data-dir resolution for the bridge (spawned by the browser, so it cannot
/// inherit a path from Desktop): `PAWORK_DATA_DIR` overrides, else
/// `$HOME/.pawork`, else the temp dir. Matches pawork-app / Desktop only on
/// Unix with a non-empty `HOME`: no Windows `LOCALAPPDATA`, an empty `HOME`
/// counts as missing, and no degrade event is emitted.
pub fn default_data_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("PAWORK_DATA_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return std::path::PathBuf::from(trimmed);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            return std::path::PathBuf::from(home).join(".pawork");
        }
    }
    std::env::temp_dir().join("pawork")
}

/// The browser family a bridge serves. Identity comes from the parent
/// process path on macOS plus the extension's own report; both must agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserKind {
    Chrome,
    Edge,
}

impl BrowserKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Edge => "edge",
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Edge => "Edge",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "chrome" => Some(Self::Chrome),
            "edge" => Some(Self::Edge),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    /// sol 审查①：没有 nativeMessaging 权限，真实 Chrome / Edge 的
    /// connectNative 直接失败，fake 扩展测试覆盖不到——静态钉住
    /// manifest 权限集，防止无意回退。
    #[test]
    fn extension_manifest_declares_native_messaging() {
        let manifest =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("extension/manifest.json");
        let body = std::fs::read_to_string(manifest).expect("extension manifest exists");
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let permissions = value["permissions"].as_array().expect("permissions array");
        for required in ["tabs", "scripting", "storage", "alarms", "nativeMessaging"] {
            assert!(
                permissions.iter().any(|item| item == required),
                "manifest missing permission {required}"
            );
        }
        // sol 复审①：captureVisibleTab 的权限检查只认 <all_urls> 或当次
        // 用户手势授予的 activeTab；后台自动化没有手势，activeTab 语义
        // 不成立，http/https host 权限不满足检查口径。
        let hosts = value["host_permissions"]
            .as_array()
            .expect("host_permissions array");
        assert!(
            hosts.iter().any(|item| item == "<all_urls>"),
            "manifest must declare <all_urls> so captureVisibleTab is permitted"
        );
    }
}
