//! Explicit user file operations, always routed through the authenticated Host.
use super::*;

#[derive(Clone, Debug)]
pub enum FileOperation {
    List,
    Read,
    Save { content: String, revision: String },
}

impl DesktopController {
    pub fn workspace_file_operation(
        &self,
        workspace_id: String,
        path: String,
        epoch: u64,
        operation: FileOperation,
    ) {
        let client = self.current_client();
        // 未连接（含测试替身）时无事件通道，直接放弃；UI 侧已按连接态 gate。
        let Some(events) = self.try_event_sender() else {
            return;
        };
        self.runtime.spawn(async move {
            let result = async {
                let client = client.ok_or_else(|| "Not connected".to_string())?;
                if client.api_version().minor < 19 {
                    return Err("File editing requires Host API 1.19 or later".into());
                }
                let response = match &operation {
                    FileOperation::Save { content, revision } => {
                        let command = serde_json::from_value(json!({
                            "method": "workspace_file_write", "params": {
                                "workspace_id": workspace_id, "path": path,
                                "content": content, "expected_revision": revision
                            }
                        }))
                        .map_err(|_| "Invalid file path".to_string())?;
                        client
                            .command(command, command_source(), actor_identity())
                            .await
                    }
                    _ => {
                        let method = if matches!(operation, FileOperation::List) {
                            "workspace_files"
                        } else {
                            "workspace_file_read"
                        };
                        let query = serde_json::from_value(json!({
                            "method": method, "params": {"workspace_id": workspace_id, "path": path}
                        }))
                        .map_err(|_| "Invalid file path".to_string())?;
                        client
                            .query(query, command_source(), actor_identity())
                            .await
                    }
                }
                .map_err(|error| match error {
                    ClientError::Protocol(error) => error.message,
                    error => error.to_string(),
                })?;
                match response.response {
                    AppResponse::Data(data)
                        if data["workspace_id"].as_str() == Some(&workspace_id)
                            && data["path"].as_str() == Some(&path) =>
                    {
                        Ok(data)
                    }
                    AppResponse::Error(error) => Err(error.message),
                    _ => Err("Invalid file response".into()),
                }
            }
            .await;
            let _ = events
                .send(ControllerEvent::WorkspaceFileResult {
                    workspace_id,
                    path,
                    epoch,
                    operation,
                    result,
                })
                .await;
        });
    }
}
