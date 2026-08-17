use pawork_app::{AppCore, MultiAgentDemoOptions};

use crate::CliError;

pub async fn run_agents_demo(
    core: &AppCore,
    cancel: bool,
    budget_tokens: Option<u64>,
    json: bool,
) -> Result<(), CliError> {
    let report = core
        .run_multi_agent_demo(MultiAgentDemoOptions {
            cancel,
            budget_input_tokens: budget_tokens,
        })
        .await?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|error| CliError::Usage(error.to_string()))?
        );
        return Ok(());
    }
    println!("parent {}", report.parent_id);
    println!("workers {}", report.workers.join(", "));
    if !report.cancelled.is_empty() {
        println!("cancelled {}", report.cancelled.join(", "));
    }
    if report.budget_exceeded {
        println!("budget-gate: exceeded");
    }
    println!("events {}", report.event_kinds.join(", "));
    Ok(())
}
