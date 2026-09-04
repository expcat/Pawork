use pawork_protocol::{AppCommand, AppCommandEnvelope, AppQuery, AppResponse, GeneralSettingsData};

use crate::gui_host::GuiHostAdapter;
use crate::gui_server::GuiHostError;
use crate::AppCore;

use super::settings_data;

fn invalid_proxy_url_error(candidate: Option<&str>) -> GuiHostError {
    let reason = match candidate {
        Some(url) if url.is_empty() => "empty".to_string(),
        Some(url) => url
            .parse::<reqwest::Url>()
            .err()
            .map(|error| error.to_string())
            .unwrap_or_else(|| "client_build".to_string()),
        None => "client_build".to_string(),
    };
    GuiHostAdapter::host_error(
        "invalid_proxy_url",
        format!("proxy URL is invalid ({reason})"),
    )
}

pub(crate) async fn general_settings(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::GeneralSettings = query else {
        unreachable!("general_settings handler receives GeneralSettings")
    };
    let core = adapter.core.read().await;
    Ok(settings_data(GeneralSettingsData {
        proxy_url: core.config().proxy_url.clone(),
    }))
}

pub(crate) async fn set_proxy_url(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SetProxyUrl { proxy_url } = command else {
        unreachable!("set_proxy_url handler receives SetProxyUrl")
    };
    let mut candidate = pawork_workspace::config::PaworkConfig::default();
    candidate.proxy_url = proxy_url.clone();
    let http = AppCore::http_from_config(&candidate)
        .map_err(|_| invalid_proxy_url_error(proxy_url.as_deref()))?;
    let path = pawork_workspace::config::global_config_path().ok_or_else(|| {
        GuiHostAdapter::host_error(
            "config_unavailable",
            "global config directory is not available on this platform",
        )
    })?;
    pawork_workspace::config::write_proxy_url(&path, proxy_url.as_deref())
        .map_err(|error| GuiHostAdapter::host_error("config_write", error.to_string()))?;
    {
        let mut core = adapter.core.write().await;
        core.set_proxy_url(proxy_url.clone(), http);
    }
    Ok(settings_data(GeneralSettingsData {
        proxy_url: proxy_url.clone(),
    }))
}
