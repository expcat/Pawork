//! 测试装配共享件：仅测试编译。OAuth token 端点 wiremock 形状的单一来源
//! （MOCK-7；与 pawork-app 的 testsupport 保持同形，两包各自内联）。

use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// token 端点成功响应 JSON（RFC 6749 TokenSet 形状；expires_in=3600、
/// token_type=Bearer 固定，refresh/scope 按需携带）。
pub(crate) fn token_success_json(
    access_token: &str,
    refresh_token: Option<&str>,
    scope: Option<&str>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "access_token": access_token,
        "expires_in": 3600,
        "token_type": "Bearer",
    });
    if let Some(refresh_token) = refresh_token {
        body["refresh_token"] = serde_json::json!(refresh_token);
    }
    if let Some(scope) = scope {
        body["scope"] = serde_json::json!(scope);
    }
    body
}

/// token 端点标准错误响应 JSON（RFC 6749 §5.2）。
pub(crate) fn token_error_json(error: &str, description: Option<&str>) -> serde_json::Value {
    match description {
        Some(description) => serde_json::json!({
            "error": error,
            "error_description": description,
        }),
        None => serde_json::json!({ "error": error }),
    }
}

/// 无附加匹配的 token 端点 Mock（POST token_path → response）；
/// 调用方按需继续链 .expect(n) / .up_to_n_times(n) 后挂载。
pub(crate) fn token_mock(token_path: &str, response: ResponseTemplate) -> Mock {
    Mock::given(method("POST"))
        .and(path(token_path))
        .respond_with(response)
}
