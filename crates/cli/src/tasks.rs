use pawork_app::{parse_task_kind, AppCore};
use pawork_client::{ConnectOptions, GuiClient, LocalTransport, TransportEndpoint};
use pawork_domain::TaskStatus;
use pawork_protocol::{ActorIdentity, AppCommand, AppResponse, CommandSource};

use crate::CliError;

use super::TasksCommand;

pub async fn run_tasks(
    core: &AppCore,
    command: TasksCommand,
    json: bool,
    data_dir: &std::path::Path,
    instance: &str,
) -> Result<(), CliError> {
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
            let snapshot = core.tasks_status(&task)?;
            let cancelled = if matches!(
                snapshot.status,
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Canceled
            ) {
                Vec::new()
            } else {
                cancel_on_host(snapshot.task_id.as_str(), data_dir, instance).await?
            };
            if json {
                println!("{}", serde_json::json!({ "cancelled": cancelled }));
            } else {
                println!("cancelled {}", cancelled.join(", "));
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

async fn cancel_on_host(
    task_id: &str,
    data_dir: &std::path::Path,
    instance: &str,
) -> Result<Vec<String>, CliError> {
    let unavailable = |error: String| {
        let owners =
            pawork_app::active_instance_host_pids(&pawork_app::instance_dir(data_dir, instance))
                .map(|pids| {
                    pids.iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|_| "unknown".into());
        CliError::Usage(format!(
            "cannot cancel task without its owning GUI Host: {error}; active Host PIDs: [{}]. For a CLI/headless/ACP task, cancel through its original host. An orphan task requires host recovery; its snapshot was not changed.",
            if owners.is_empty() { "none" } else { &owners }
        ))
    };
    let authentication = crate::ops::load_gui_client_authentication(data_dir, instance)
        .map_err(|error| unavailable(error.to_string()))?;
    let client = GuiClient::connect(
        std::sync::Arc::new(LocalTransport::default()),
        TransportEndpoint::Local {
            address: crate::ops::gui_socket_path(data_dir, instance)
                .to_string_lossy()
                .into_owned(),
        },
        ConnectOptions {
            timeout_ms: 5_000,
            client_label: Some("pawork-tasks-cancel".into()),
            max_frame_bytes: 1024 * 1024,
        },
        Some(authentication),
    )
    .await
    .map_err(|error| unavailable(error.to_string()))?;
    let result = async {
        if client.api_version() < pawork_protocol::V1_23 {
            return Err(CliError::Usage(
                "tasks cancel requires Host API 1.23 or newer; restart the Host after upgrading"
                    .into(),
            ));
        }
        let response = client
            .command(
                AppCommand::TasksCancel {
                    task_id: task_id.into(),
                },
                CommandSource::RemoteGui {
                    client_id: client.client_id().clone(),
                    connection_id: client.connection_id().clone(),
                },
                ActorIdentity::LocalUser {
                    actor_id: "local-user".into(),
                    display_name: None,
                },
            )
            .await
            .map_err(|error| CliError::Usage(error.to_string()))?;
        match response.response {
            AppResponse::Data(data) => serde_json::from_value(
                data.get("cancelled").cloned().unwrap_or_default(),
            )
            .map_err(|error| CliError::Usage(format!("invalid tasks cancel response: {error}"))),
            AppResponse::Error(error) => Err(CliError::Usage(error.message)),
            _ => Err(CliError::Usage("invalid tasks cancel response".into())),
        }
    }
    .await;
    let _ = client.disconnect().await;
    result
}
