//! Generic model gateway CLI; token management does not assemble or contact providers.
use crate::{CliError, GatewayCommand, GatewayTokenCommand};
use pawork_app::AppCore;
use pawork_domain::CancellationToken;
use pawork_gateway::GatewayTokenStore;
use std::{path::PathBuf, sync::Arc};

pub fn token_store(directory: &std::path::Path) -> GatewayTokenStore {
    GatewayTokenStore::new(directory.join("gateway-tokens"))
}
pub fn manage(
    command: &GatewayCommand,
    directory: &std::path::Path,
    json: bool,
) -> Result<bool, CliError> {
    let store = token_store(directory);
    match command {
        GatewayCommand::Token { command } => {
            match command {
                GatewayTokenCommand::Issue { client } => {
                    let (info, token) = store.issue(client)?;
                    if json {
                        println!(
                            "{}",
                            serde_json::json!({"id":info.id,"client":info.client,"token":token})
                        );
                    } else {
                        eprintln!(
                            "token id: {} · client: {} (secret shown once)",
                            info.id, info.client
                        );
                        println!("{token}");
                    }
                }
                GatewayTokenCommand::List => println!(
                    "{}",
                    serde_json::to_string_pretty(&store.list()?)
                        .map_err(|_| CliError::Usage("cannot list gateway tokens".into()))?
                ),
                GatewayTokenCommand::Revoke { id } => {
                    store.revoke(id)?;
                    println!("{}", serde_json::json!({"revoked":id}));
                }
            }
            Ok(true)
        }
        GatewayCommand::Status | GatewayCommand::Shutdown => {
            let path = directory.join("gateway-serve.pid");
            let pid = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());
            let running = pid.is_some_and(|pid| {
                pawork_app::active_instance_host_pids(directory)
                    .is_ok_and(|pids| pids.contains(&pid))
            }) && pawork_gateway::gateway_is_running(directory)?;
            if matches!(command, GatewayCommand::Shutdown) {
                if !running {
                    return Err(CliError::Usage("Pawork gateway is not running".into()));
                }
                crate::ops::send_term(pid.expect("running gateway PID"))?;
            }
            println!(
                "{}",
                serde_json::json!({"running":running,"pid":if running {pid}else{None},"action":if matches!(command,GatewayCommand::Shutdown){"shutdown"}else{"status"}})
            );
            Ok(true)
        }
        GatewayCommand::Serve { .. } => Ok(false),
    }
}

pub async fn serve(core: AppCore, directory: PathBuf, port: u16) -> Result<(), CliError> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let address = listener.local_addr()?;
    let pid_path = directory.join("gateway-serve.pid");
    crate::ops::write_pid_file(&pid_path)?;
    let cancel = CancellationToken::new();
    let core = Arc::new(core);
    eprintln!("Pawork model gateway: http://{address}/v1 (Ctrl-C to stop)");
    let result = {
        let service = pawork_gateway::serve_gateway(
            core.clone(),
            listener,
            token_store(&directory),
            cancel.clone(),
        );
        tokio::pin!(service);
        #[cfg(unix)]
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let stop = async {
            #[cfg(unix)]
            tokio::select! { _=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{} }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
        };
        tokio::select! {
            result=&mut service=>result,
            _=stop=>{cancel.cancel();service.await}
        }
    };
    crate::ops::remove_pid_file(&pid_path);
    Arc::try_unwrap(core)
        .map_err(|_| CliError::Usage("gateway tasks still active".into()))?
        .shutdown()
        .await?;
    result?;
    Ok(())
}
