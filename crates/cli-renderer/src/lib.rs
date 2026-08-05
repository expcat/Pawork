//! CLI 输出渲染；JSON 路径保持单行且可稳定解析。

use app_service::ServiceResponse;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

pub fn render(response: &ServiceResponse, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => serde_json::to_string(response).expect("response is serializable"),
        OutputFormat::Text if response.data.is_null() => response.message.clone(),
        OutputFormat::Text => format!("{}\n{}", response.message, response.data),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn json_output_is_single_line_and_parseable() {
        let response = ServiceResponse {
            ok: true,
            kind: "status".into(),
            message: "ready".into(),
            data: json!({ "b": 2, "a": 1 }),
        };
        let output = render(&response, OutputFormat::Json);
        assert!(!output.contains('\n'));
        let decoded: Value = serde_json::from_str(&output).expect("parse JSON output");
        assert_eq!(decoded["kind"], "status");
    }
}
