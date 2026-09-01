use pawork_app::{parse_task_kind, AppCore};

use crate::CliError;

use super::TasksCommand;

pub async fn run_tasks(core: &AppCore, command: TasksCommand, json: bool) -> Result<(), CliError> {
    match command {
        TasksCommand::List => {
            let tasks = core.tasks_list();
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&tasks)
                        .map_err(|error| CliError::Usage(error.to_string()))?
                );
                return Ok(());
            }
            if tasks.is_empty() {
                println!("no tasks");
                return Ok(());
            }
            for task in tasks {
                println!(
                    "{}  {:?}  {:?}",
                    task.task_id.as_str(),
                    task.task_kind,
                    task.status
                );
            }
            Ok(())
        }
        TasksCommand::Status { task } => {
            let snapshot = core.tasks_status(&task)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&snapshot)
                        .map_err(|error| CliError::Usage(error.to_string()))?
                );
            } else {
                println!(
                    "{}  {:?}  {:?}  {}",
                    snapshot.task_id.as_str(),
                    snapshot.task_kind,
                    snapshot.status,
                    snapshot.detail.as_deref().unwrap_or("-")
                );
            }
            Ok(())
        }
        TasksCommand::Cancel { task } => {
            let cancelled = core.tasks_cancel(&task)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "cancelled": cancelled.iter().map(|id| id.as_str()).collect::<Vec<_>>() })
                );
            } else {
                println!(
                    "cancelled {}",
                    cancelled
                        .iter()
                        .map(|id| id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            Ok(())
        }
        TasksCommand::Register { kind } => {
            let kind = parse_task_kind(kind.as_deref().unwrap_or("automation"))?;
            let id = core.tasks_register(kind)?;
            if json {
                println!("{}", serde_json::json!({ "task_id": id.as_str() }));
            } else {
                println!("{}", id.as_str());
            }
            Ok(())
        }
    }
}
