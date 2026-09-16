//! System web view and restricted DOM controls. No GPUI, Core, protocol or page-to-host bridge.
#![allow(unexpected_cfgs)] // objc 0.2's macros expand their legacy cargo-clippy cfg at crate scope.

mod dom;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::BrowserView;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BrowserState {
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub error: Option<String>,
}

/// Normalize an address without executing scripts, opening files, or searching.
pub fn normalize_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() || input.starts_with(['/', '\\']) || input.chars().any(char::is_control) {
        return Err("请输入有效的网页地址 / Enter a website address".into());
    }
    let has_scheme = input.contains("://");
    let candidate = if has_scheme {
        input.to_string()
    } else {
        // A colon in a bare host is only valid for a numeric port (or IPv6).
        let authority = input.split(['/', '?', '#']).next().unwrap_or(input);
        if !authority.starts_with('[') {
            if let Some((_, port)) = authority.split_once(':') {
                if port.is_empty() || !port.bytes().all(|c| c.is_ascii_digit()) {
                    return Err("仅支持 HTTP(S) 网页 / Only HTTP(S) addresses are supported".into());
                }
            }
        }
        format!("https://{input}")
    };
    let mut url = url::Url::parse(&candidate)
        .map_err(|_| "网页地址无效 / Invalid website address".to_string())?;
    if !is_web_url(&url) {
        return Err(
            "仅支持无内嵌凭据的 HTTP(S) 地址 / Use HTTP(S) without embedded credentials".into(),
        );
    }
    if !has_scheme {
        let local = match url.host() {
            Some(url::Host::Domain(host)) => host == "localhost" || host.ends_with(".localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        if local {
            let _ = url.set_scheme("http");
        }
    }
    Ok(url.into())
}

pub(crate) fn is_web_url(url: &url::Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

#[cfg(any(target_os = "macos", test))]
fn navigation_allowed(address: &str) -> bool {
    address == "about:blank" || url::Url::parse(address).is_ok_and(|url| is_web_url(&url))
}

#[cfg(not(target_os = "macos"))]
mod unsupported {
    use super::BrowserState;
    /// System backend is currently available on macOS only.
    pub struct BrowserView(std::marker::PhantomData<std::rc::Rc<()>>);
    impl BrowserView {
        pub fn new(_: raw_window_handle::WindowHandle<'_>) -> Result<Self, String> {
            Err("浏览器首版仅支持 macOS / Browser is currently available on macOS".into())
        }
        pub fn navigate(&self, _: &str) -> Result<(), String> {
            Err("Unsupported platform".into())
        }
        pub fn state(&self) -> BrowserState {
            BrowserState::default()
        }
        pub fn back(&self) {}
        pub fn forward(&self) {}
        pub fn reload(&self) {}
        pub fn stop(&self) {}
        pub fn set_bounds(&self, _: f64, _: f64, _: f64, _: f64) {}
        pub fn set_visible(&self, _: bool) {}
        pub fn is_focused(&self) -> bool {
            false
        }
        pub fn focus_parent(&self) {}
        pub fn read_page(&self, callback: impl FnOnce(Result<String, String>) + 'static) {
            callback(Err(
                "浏览器首版仅支持 macOS / Browser is currently available on macOS".into(),
            ));
        }
        pub fn click(&self, _: &str, callback: impl FnOnce(Result<String, String>) + 'static) {
            callback(Err("Unsupported platform".into()));
        }
        pub fn type_text(
            &self,
            _: &str,
            _: &str,
            callback: impl FnOnce(Result<String, String>) + 'static,
        ) {
            callback(Err("Unsupported platform".into()));
        }
    }
}
#[cfg(not(target_os = "macos"))]
pub use unsupported::BrowserView;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_normalization_preserves_navigation_and_local_preview() {
        for (input, expected) in [
            (
                " example.com/path?q=1#part ",
                "https://example.com/path?q=1#part",
            ),
            ("localhost:3000", "http://localhost:3000/"),
            ("127.0.0.1:5173/preview", "http://127.0.0.1:5173/preview"),
            ("[::1]:8080", "http://[::1]:8080/"),
            ("https://localhost:3000", "https://localhost:3000/"),
            ("http://example.com", "http://example.com/"),
        ] {
            assert_eq!(normalize_url(input).unwrap(), expected);
            assert!(navigation_allowed(expected));
        }
    }

    #[test]
    fn navigation_rejects_local_files_scripts_credentials_and_custom_schemes() {
        for input in [
            "",
            "javascript:alert(1)",
            "data:text/html,hi",
            "file:///etc/hosts",
            "pawork://open",
            "mailto:me@example.com",
            "https://user:secret@example.com",
            "http://",
            "https://exa mple.com",
            "https://example.com/\nfile",
            "/etc/hosts",
        ] {
            assert!(normalize_url(input).is_err(), "accepted {input:?}");
        }
        for input in [
            "file:///etc/hosts",
            "javascript:alert(1)",
            "data:text/html,hi",
            "mailto:a@b.com",
            "http://user:pass@localhost",
            "about:config",
        ] {
            assert!(!navigation_allowed(input), "accepted navigation {input:?}");
        }
        assert!(navigation_allowed("about:blank"));
    }
}
