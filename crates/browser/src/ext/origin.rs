//! Website-origin normalization for extension-reported URLs: lower-case
//! scheme + host, default ports elided, path / query / fragment dropped,
//! userinfo rejected; hostname limited to domain characters with at least
//! one letter or digit; IPv4 hosts are accepted in canonical form, IPv6
//! literals fail closed. Unlike the CU-16 approval identity, explicit
//! default ports are elided and URLs with paths are accepted.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OriginError {
    Invalid(&'static str),
}

impl fmt::Display for OriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OriginError::Invalid(reason) => write!(f, "invalid origin: {reason}"),
        }
    }
}

/// Normalize a full URL or bare origin into `scheme://host[:port]`.
pub fn normalize_origin(input: &str) -> Result<String, OriginError> {
    let url = url::Url::parse(input).map_err(|_| OriginError::Invalid("not a URL"))?;
    let scheme = url.scheme();
    if !matches!(scheme, "http" | "https") {
        return Err(OriginError::Invalid("only http(s) origins"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(OriginError::Invalid("embedded credentials"));
    }
    let host = match url.host() {
        Some(url::Host::Domain(host)) => host.to_ascii_lowercase(),
        // CU-16 host alphabet admits dotted-quad IPv4 (the local fixture
        // lives on 127.0.0.1); accept it in canonical form.
        Some(url::Host::Ipv4(address)) => address.to_string(),
        // IPv6 literals (and anything else) fail closed, mirroring the
        // CU-16 website-identity rules.
        _ => return Err(OriginError::Invalid("host must be a domain name")),
    };
    if host.is_empty() {
        return Err(OriginError::Invalid("empty host"));
    }
    if !host
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'-')
        || !host.bytes().any(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(OriginError::Invalid("host characters"));
    }
    let port = url.port().unwrap_or_else(|| {
        if scheme == "https" {
            443
        } else {
            80
        }
    });
    if !(1..=65535).contains(&port) {
        return Err(OriginError::Invalid("port"));
    }
    let shown = match (scheme, port) {
        ("http", 80) | ("https", 443) => String::new(),
        _ => format!(":{port}"),
    };
    Ok(format!("{scheme}://{host}{shown}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_scheme_host_port_and_case() {
        assert_eq!(
            normalize_origin("https://Example.COM/a/b?q=1#f").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            normalize_origin("http://localhost:3000/x").unwrap(),
            "http://localhost:3000"
        );
        assert_eq!(
            normalize_origin("http://example.com:80").unwrap(),
            "http://example.com"
        );
        assert_eq!(
            normalize_origin("https://example.com:443").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            normalize_origin("https://sub.domain-1.example").unwrap(),
            "https://sub.domain-1.example"
        );
        assert_eq!(
            normalize_origin("http://127.0.0.1:8933/site/index.html").unwrap(),
            "http://127.0.0.1:8933"
        );
        assert_eq!(
            normalize_origin("https://192.168.10.1").unwrap(),
            "https://192.168.10.1"
        );
    }

    #[test]
    fn rejects_non_http_credentials_ipv6_and_bad_hosts() {
        for bad in [
            "file:///etc/hosts",
            "chrome://settings",
            "ftp://example.com",
            "https://user:secret@example.com/",
            "http://[::1]:8080/",
            "http://example.com:0/",
            "http://example.com:99999/",
            "about:blank",
        ] {
            assert!(normalize_origin(bad).is_err(), "accepted {bad}");
        }
    }
}
