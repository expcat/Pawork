use pawork_app::AppCore;

use crate::CliError;

pub async fn run_usage(
    core: &AppCore,
    session: Option<String>,
    json: bool,
) -> Result<(), CliError> {
    let session_id = if let Some(spec) = session {
        Some(core.resolve_session(&spec).await?)
    } else {
        None
    };
    let overview = core.usage_overview(None, session_id.as_ref()).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&overview).map_err(|error| CliError::Usage(error.to_string()))?
        );
        return Ok(());
    }
    println!("provider: {}", overview.provider_id);
    if let Some(session) = &overview.session {
        println!(
            "session {}: in {} out {} (cache read {} / write {})",
            session.session_id,
            session.input_tokens,
            session.output_tokens,
            session.cache_read_tokens,
            session.cache_write_tokens
        );
    }
    println!(
        "ledger: in {} out {} (cache read {} / write {})",
        overview.ledger.input_tokens,
        overview.ledger.output_tokens,
        overview.ledger.cache_read_tokens,
        overview.ledger.cache_write_tokens
    );
    for window in &overview.windows {
        println!(
            "quota tokens {}: used {} limit {} remaining {} ({})",
            window.window, window.used, window.limit, window.remaining, window.confidence
        );
    }
    Ok(())
}
