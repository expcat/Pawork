//! `pawork` —— Pawork 的唯一正式可执行宿主（CLI 与 Core 同进程同二进制）。
//!
//! 装配流程：解析 CLI → 初始化 tracing → 装配 [`core_runtime::CoreRuntime`]
//! （AppService + EventHub + EventPump）→ 装配 GUI Server（P13-4，serve 模式
//! 打开本地端点）→ 交给 [`cli_host::CliHost`] 按运行模式执行 → 以退出码结束进程。
//!
//! `headless --json-stdio`（P17-8）走独立路径：不装配 GUI Server、不经过
//! execute 的文本输出，stdout 只写 NDJSON 帧（协议入口，无 TUI/CLI 文本）。

mod gui_host;

use std::sync::Arc;

use clap::Parser;
use cli_command::{AcpCommand, Cli, Command};
use cli_host::CliHost;
use client_auth::{TokenAuthenticator, TokenStore};
use core_api::SUPPORTED_API_VERSIONS;
use core_runtime::{CoreRuntime, CoreRuntimeConfig};
use gui_host::ServeGuiHost;
use gui_protocol::{GuiCapability, HandshakeService};
use gui_server::{GuiServer, GuiServerConfig};
use session_store::SessionStore;
use tracing_subscriber::EnvFilter;
use transport_remote::{
    RealRemoteTransport, RealRemoteTransportConfig, RealRemoteTransportProvider,
};

#[tokio::main]
async fn main() {
    init_tracing();
    let cli = Cli::parse();

    // 装配完整 Core：P17 durable Team + P18 durable usage/control plane +
    // EventHub/EventPump。任一持久事实源无法打开都 fail loud，不降级为内存状态。
    let instance_dir = gui_host::instance_dir(&cli.instance);
    let ledger_path = instance_dir.join("usage-ledger.sqlite3");
    let control_plane_path = instance_dir.join("control-plane.sqlite3");
    let runtime = match CoreRuntime::with_persistent_control_plane_config(
        CoreRuntimeConfig {
            instance: cli.instance.clone(),
            team_db_path: Some(instance_dir.join("teams.sqlite")),
            ..CoreRuntimeConfig::default()
        },
        &ledger_path,
        &control_plane_path,
    )
    .await
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(
                ledger_path = %ledger_path.display(),
                control_plane_path = %control_plane_path.display(),
                error = %error,
                "persistent Team/usage/control-plane state unavailable; refusing to start",
            );
            std::process::exit(1);
        }
    };

    // P17-1 生产装配：global + workspace 两级 user hook 配置 → UserHookHost
    // （pre-prompt / pre-tool 权威位点回灌 + 事件桥 + 审计落库）。装配失败
    // 不阻断宿主启动，仅降级为不加载 hooks（与未注入时行为一致）。
    match pawork::user_hooks::assemble_user_hooks(
        runtime.service().clone(),
        &pawork::user_hooks::cli_workspace_roots(&cli),
        config_service::config_dir_for_app(),
        instance_dir.join("hooks.sqlite"),
        Arc::new(auth_service::KeychainBackend::new()),
    ) {
        Ok(assembly) => {
            tracing::info!(
                hooks = assembly.hook_ids.len(),
                workspaces = assembly.workspace_ids.len(),
                "user hooks assembled"
            );
        }
        Err(message) => tracing::warn!("user hooks disabled: {message}"),
    }

    // P17-5 生产装配：主 run profile 解析器（复用 P17-1 ResourceLoader 加载
    // profiles_v2）+ 后台任务管理器（background run 经 TaskManager 注册 /
    // 启动 / 完成 / 取消 TaskKind::Agent）。装配失败不阻断宿主启动，仅降级为
    // 不注入（RunStart 携带 profile 名 / background 时 fail-closed）。
    match pawork::user_hooks::assemble_run_profiles(
        &runtime.service().clone(),
        &pawork::user_hooks::cli_workspace_roots(&cli),
        config_service::config_dir_for_app(),
    ) {
        Ok(resolver) if !resolver.is_empty() => {
            runtime
                .service()
                .set_profile_resolver(std::sync::Arc::new(resolver));
        }
        Ok(_) => {}
        Err(message) => tracing::warn!("run profile resolver disabled: {message}"),
    }
    // P17-5 生产模型覆盖授权策略：显式模型与 profile canonical 落点不同时，
    // 仅本机交互来源（LocalCli / LocalGui）+ LocalUser 身份放行；Remote /
    // Automation / Plugin / MCP 一律拒绝（System 默认拒绝：内部服务动作的
    // 模型覆盖应走显式 profile/配置，宿主可注入自定义策略显式放行）。
    runtime
        .service()
        .set_model_override_policy(std::sync::Arc::new(
            app_service::ProductionModelOverridePolicy,
        ));
    let (sandbox_backend, _selection) = sandbox_runtime::SandboxSelector::new().pick();
    runtime
        .service()
        .set_task_manager(std::sync::Arc::new(task_manager::TaskManager::new(
            sandbox_backend,
        )));

    let mut host = CliHost::with_hub(runtime.service().clone(), runtime.hub().clone());

    // `acp serve`（P17-7）：ACP（Agent Client Protocol v1）stdio JSON-RPC
    // 入口。与 headless 同理：不装配 GUI Server（协议隔离），stdout 只写
    // 协议帧；Session Registry 复用同一实例 SQLite SessionStore
    // （SqliteClientSessionRegistryStore），不私建 ownership/credential
    // 状态；Core 事件经共享 Event Hub 订阅回译。
    if let Command::Acp(acp) = &cli.command {
        if matches!(acp.command, AcpCommand::Serve) {
            let path = instance_dir.join("session.db");
            match SessionStore::open(&path).await {
                Ok((store, _)) => {
                    let exit_code = host.run_acp_stdio(store).await;
                    std::process::exit(exit_code);
                }
                Err(error) => {
                    eprintln!(
                        "acp serve: cannot open session registry at {}: {error}",
                        path.display()
                    );
                    std::process::exit(1);
                }
            }
        }
    }

    // P17-11 真实远程 Transport 装配：Provider 与 GUI Server 共享同一
    // RealRemoteTransport 实例——`remote publish` 返回的端点由同一 Core 经
    // ServeGuiHost::bind_remote 实际 bind / accept；unpublish / revoke 关闭
    // 监听器并使凭证真正失效。装配不触盘（端点凭证在 publish 时生成）。
    let remote_transport = Arc::new(RealRemoteTransport::new(RealRemoteTransportConfig::new(
        TokenStore::new(instance_dir.join("remote.token")),
        None,
    )));
    host.attach_remote_provider(Arc::new(RealRemoteTransportProvider::new(Arc::clone(
        &remote_transport,
    ))));

    // headless --json-stdio（P17-8）：真实协议入口。stdout 只写 JSONL 帧，
    // 不装配 GUI Server（协议隔离），compat 导入/历史接到本实例 SessionStore
    // 持久化；直接运行 NDJSON 循环并以退出码结束，不经过 execute 文本输出。
    if let Command::Headless(args) = &cli.command {
        if args.json_stdio {
            let path = instance_dir.join("session.db");
            match SessionStore::open(&path).await {
                Ok((store, _)) => host.attach_session_store(Arc::new(store)),
                Err(error) => tracing::warn!(
                    "headless session store disabled at {}: {error}",
                    path.display()
                ),
            }
            let exit_code = host.run_headless_stdio().await;
            std::process::exit(exit_code);
        }
    }

    // GUI Server 装配（P13-4）：serve 模式打开本地 GUI Endpoint；装配失败
    // （token 目录不可写等）时降级为仅等待信号并告警。
    match build_gui_server(&runtime, &cli.instance, remote_transport) {
        Ok(server) => host.attach_gui_server(Arc::new(server)),
        Err(message) => tracing::warn!("gui server disabled: {message}"),
    }

    let outcome = host.execute(cli).await;
    println!("{}", outcome.output);
    if outcome.exit_code != 0 {
        std::process::exit(outcome.exit_code);
    }
}

/// 装配 serve 模式 GUI Server：本地端点走 LocalTransport、远程端点走与
/// Provider 共享的 RealRemoteTransport（复合路由），每实例 token 认证。
fn build_gui_server(
    runtime: &CoreRuntime,
    instance: &str,
    remote_transport: Arc<RealRemoteTransport>,
) -> Result<ServeGuiHost, String> {
    let token_store = TokenStore::new(gui_host::instance_dir(instance).join("gui.token"));
    if !token_store.path().exists() {
        // 首次运行生成 token；已存在则复用（重启不覆盖）。
        let _ = token_store
            .generate()
            .map_err(|error| format!("cannot create gui token: {error}"))?;
    } else {
        let _ = token_store
            .load()
            .map_err(|error| format!("cannot read gui token: {error}"))?;
    }

    let handshake = HandshakeService::new(
        agent_domain::CoreInstanceId::from(instance),
        SUPPORTED_API_VERSIONS.to_vec(),
        vec![
            GuiCapability::Events,
            GuiCapability::Snapshots,
            GuiCapability::ArtifactStreaming,
        ],
    )
    .with_authenticator(Box::new(TokenAuthenticator::new(token_store)));

    let local: Arc<dyn transport_api::GuiTransportServer> =
        Arc::new(transport_local::LocalTransport::default());
    let transport: Arc<dyn transport_api::GuiTransportServer> = Arc::new(
        gui_host::CompositeGuiTransport::new(local, remote_transport),
    );
    let server = Arc::new(GuiServer::new(GuiServerConfig {
        app_service: runtime.service().clone(),
        handshake,
        transport,
        hub: runtime.hub().clone(),
        connections: None,
    }));
    Ok(ServeGuiHost::new(server))
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        // CLI stdout is the command protocol surface (including `--json`).
        // Keep diagnostics on stderr so startup/reconciliation logs cannot
        // corrupt machine-readable responses.
        .with_writer(std::io::stderr)
        .try_init();
}
