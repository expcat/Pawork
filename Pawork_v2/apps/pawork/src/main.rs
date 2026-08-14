//! composition root：组装后交给 `pawork-cli`。

#[tokio::main]
async fn main() -> std::process::ExitCode {
    pawork_cli::run().await
}
