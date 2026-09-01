use pawork_app::{review_status_label, AppCore, PlanSnapshot};
use pawork_domain::SessionId;

use crate::CliError;

use super::PlanCommand;

pub async fn run_plan(core: &AppCore, command: PlanCommand, json: bool) -> Result<(), CliError> {
    match command {
        PlanCommand::Show { session } => {
            let session_id = resolve_or_latest(core, session).await?;
            match core.plan_snapshot(&session_id).await? {
                Some(snapshot) => print_plan(&snapshot, json)?,
                None => {
                    if json {
                        println!("{}", serde_json::json!({ "plan": null }));
                    } else {
                        println!("no plan on session {}", session_id.as_str());
                    }
                }
            }
            Ok(())
        }
        PlanCommand::Create {
            session,
            title,
            step,
        } => {
            let session_id = resolve_or_create(core, session, &title).await?;
            let snapshot = core.plan_create(&session_id, &title, step).await?;
            print_plan(&snapshot, json)
        }
        PlanCommand::Replace {
            session,
            title,
            step,
        } => {
            let session_id = resolve_or_latest(core, session).await?;
            let snapshot = core.plan_replace(&session_id, &title, step).await?;
            print_plan(&snapshot, json)
        }
        PlanCommand::Submit { session } => {
            let session_id = resolve_or_latest(core, session).await?;
            let snapshot = core.plan_submit(&session_id).await?;
            print_plan(&snapshot, json)
        }
        PlanCommand::Approve { session } => {
            let session_id = resolve_or_latest(core, session).await?;
            let snapshot = core.plan_approve(&session_id).await?;
            print_plan(&snapshot, json)
        }
        PlanCommand::Reject { session, reason } => {
            let session_id = resolve_or_latest(core, session).await?;
            let snapshot = core.plan_reject(&session_id, &reason).await?;
            print_plan(&snapshot, json)
        }
    }
}

async fn resolve_or_latest(core: &AppCore, session: Option<String>) -> Result<SessionId, CliError> {
    core.resolve_session(session.as_deref().unwrap_or("latest"))
        .await
        .map_err(CliError::from)
}

async fn resolve_or_create(
    core: &AppCore,
    session: Option<String>,
    title: &str,
) -> Result<SessionId, CliError> {
    match session {
        Some(spec) => Ok(core.resolve_session(&spec).await?),
        None => match core.resolve_session("latest").await {
            Ok(id) => Ok(id),
            Err(_) => Ok(core.create_session(title).await?),
        },
    }
}

fn print_plan(snapshot: &PlanSnapshot, json: bool) -> Result<(), CliError> {
    if json {
        println!(
            "{}",
            serde_json::to_string(snapshot).map_err(|error| CliError::Usage(error.to_string()))?
        );
        return Ok(());
    }
    println!(
        "{}@{}  {}  {}",
        snapshot.plan_id.as_str(),
        snapshot.version.as_str(),
        snapshot.title,
        review_status_label(snapshot.review_status)
    );
    for step in &snapshot.steps {
        println!(
            "  - [{}] {}",
            format!("{:?}", step.status).to_ascii_lowercase(),
            step.text
        );
    }
    Ok(())
}
