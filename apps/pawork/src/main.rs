//! `pawork` —— Pawork 的唯一正式可执行宿主（CLI 与 Core 同进程同二进制）。
//!
//! 装配流程：解析 CLI → 初始化 tracing → 装配 [`core_runtime::CoreRuntime`]
//! （AppService + EventHub + EventPump）→ 交给 [`cli_host::CliHost`] 按运行模式
//! 执行 → 以退出码结束进程。

use clap::Parser;
use cli_command::Cli;
use cli_host::CliHost;
use core_runtime::CoreRuntime;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    init_tracing();
    let cli = Cli::parse();

    // 装配完整 Core：AppService + EventHub + EventPump（10ms 轮询事件队列）。
    let runtime = CoreRuntime::new(cli.instance.clone());
    let host = CliHost::with_hub(runtime.service().clone(), runtime.hub().clone());
    // GUI Server 装配位（P13-4 注入）；未装配时 serve 仅等待信号。
    // host.attach_gui_server(gui_server);

    let outcome = host.execute(cli).await;
    println!("{}", outcome.output);
    if outcome.exit_code != 0 {
        std::process::exit(outcome.exit_code);
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
