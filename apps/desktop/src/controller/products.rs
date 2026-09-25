//! 产品面板使用鉴权客户端；所有网络等待仍在 Tokio runtime。
use super::*;

impl DesktopController {
    pub fn supports_product_panels(&self) -> bool {
        self.current_client()
            .is_some_and(|client| client.api_version().minor >= 24)
    }
    pub fn product_request(
        &self,
        request: Result<AppCommand, AppQuery>,
    ) -> tokio::task::JoinHandle<Result<serde_json::Value, String>> {
        let client = self.current_client();
        self.runtime.spawn(async move {
            let client = client.ok_or_else(|| "Not connected".to_string())?;
            if client.api_version().minor < 24 {
                return Err("Requires Host API 1.24; restart the Host after upgrading".into());
            }
            let response = match request {
                Ok(command) => {
                    client
                        .command(command, command_source(), actor_identity())
                        .await
                }
                Err(query) => {
                    client
                        .query(query, command_source(), actor_identity())
                        .await
                }
            }
            .map_err(|e| e.to_string())?;
            match response.response {
                AppResponse::Data(value) => Ok(value),
                AppResponse::Error(error) => Err(error.message),
                _ => Err("Unexpected product response".into()),
            }
        })
    }
}
