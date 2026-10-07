//! Browser action claims and replies remain on the authenticated GUI connection.
use super::*;
impl DesktopController {
    pub fn poll_browser(&self, session_id: String, run_id: String) {
        let Some(client) = self.current_client() else {
            return;
        };
        if !client
            .capabilities()
            .contains(&GuiCapability::BrowserControl)
            || client.api_version().minor < 18
            || self.state.browser_polling.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let state = self.state.clone();
        let events = self.event_sender();
        self.runtime.spawn(async move {
            if let Ok(response) = client
                .query(
                    AppQuery::BrowserNext {
                        session_id: session_id.clone().into(),
                        run_id: run_id.clone().into(),
                    },
                    command_source(),
                    actor_identity(),
                )
                .await
            {
                if let AppResponse::Data(data) = response.response {
                    if let Some(id) = data["request_id"].as_str() {
                        let _ = events
                            .send(ControllerEvent::BrowserRequest {
                                session_id,
                                run_id,
                                request_id: id.into(),
                                action: data["action"].clone(),
                            })
                            .await;
                    }
                }
            }
            state.browser_polling.store(false, Ordering::Release);
        });
    }
    pub fn browser_respond(&self, request_id: String, result: serde_json::Value) {
        let Some(client) = self.current_client() else {
            return;
        };
        self.runtime.spawn(async move {
            let _ = client
                .command(
                    AppCommand::BrowserRespond { request_id, result },
                    command_source(),
                    actor_identity(),
                )
                .await;
        });
    }

    /// CU-15：外部浏览器（用户真实 Chrome/Edge）动作。不经内嵌
    /// WebView、不依赖任务可见性：发现目录里由用户启用开关选中的桥
    /// 进程执行动作，回执仍走 BrowserRespond。桥客户端是同步 unix
    /// socket 往返，放 spawn_blocking 避免 tokio reactor 内做阻塞 IO。
    pub fn external_browser_request(&self, request_id: String, action: serde_json::Value) {
        let Some(client) = self.current_client() else {
            return;
        };
        self.runtime.spawn(async move {
            let data_dir = pawork_browser::ext::default_data_dir();
            let result = tokio::task::spawn_blocking(move || {
                pawork_browser::ext::client::execute(&data_dir, &action)
            })
            .await
            .unwrap_or_else(|_| Err("external browser action crashed".to_string()));
            let reply = match result {
                Ok(data) => serde_json::json!({"ok": true, "data": data}),
                Err(error) => serde_json::json!({"ok": false, "error": error}),
            };
            let _ = client
                .command(
                    AppCommand::BrowserRespond {
                        request_id,
                        result: reply,
                    },
                    command_source(),
                    actor_identity(),
                )
                .await;
        });
    }
}
