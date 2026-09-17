//! Manual isolated acceptance helper. Every JSON line is an explicitly requested action.
//! `cargo run -p pawork-computer-use --example isolated_probe -- /tmp/computer-proof.jpg`
//! Send {"action":"status"}, then {"action":"screenshot"}; inputs use the returned ID.
use pawork_computer_use::{Action, Computer};
use std::io::{BufRead, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let image_path = std::env::args_os()
        .nth(1)
        .ok_or("provide a screenshot output path")?;
    let computer = Computer::isolated();
    for line in std::io::stdin().lock().lines() {
        let result = serde_json::from_str::<Action>(&line?)
            .map_err(|e| e.to_string())
            .and_then(|action| {
                computer
                    .execute("manual-isolated-probe", action, &|| false)
                    .map_err(|e| e.to_string())
            });
        match result {
            Ok(output) => {
                if let Some(bytes) = output.jpeg {
                    std::fs::write(&image_path, bytes)?;
                }
                println!(
                    "{}",
                    serde_json::json!({"ok":true,"observation":output.observation,"permissions":output.permissions})
                );
            }
            Err(error) => println!("{}", serde_json::json!({"ok":false,"error":error})),
        }
        std::io::stdout().flush()?;
    }
    Ok(())
}
