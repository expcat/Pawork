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
}
