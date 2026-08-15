//! composition root：全局脱敏日志 + 组装后交给 `pawork-cli`。

use std::io;
use std::sync::{Arc, Mutex};

use pawork_diagnostics::{RedactingFmtLayer, Redactor};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

fn install_logging() {
    // 级别由 RUST_LOG 控制（默认 warn）；输出走 stderr，stdout 留给协议/JSON。
    // 所有字段先经 Redactor 再格式化，Secret 不进终端与日志。
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let layer = RedactingFmtLayer::new(
        Redactor::default(),
        Arc::new(Mutex::new(io::stderr())) as Arc<Mutex<dyn io::Write + Send>>,
    );
    tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .init();
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    install_logging();
    pawork_cli::run().await
}
