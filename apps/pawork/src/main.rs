//! `pawork` —— Pawork 的唯一正式可执行宿主（CLI 与 Core 同进程同二进制）。

use std::sync::Arc;

use app_service::AppService;
use clap::Parser;
use cli_command::Cli;
use cli_host::CliHost;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let service = Arc::new(AppService::new(cli.instance.clone()));
    let outcome = CliHost::new(service).execute(cli).await;
    println!("{}", outcome.output);
    if outcome.exit_code != 0 {
        std::process::exit(outcome.exit_code);
    }
}
