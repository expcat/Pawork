//! `pawork mcp list/test`。

use pawork_app::AppCore;

use crate::{CliError, McpCommand};

pub async fn run_mcp(
    core: &mut AppCore,
    command: McpCommand,
    json: bool,
) -> Result<(), CliError> {
    match command {
        McpCommand::List => print_status(core.mcp_list(), json),
        McpCommand::Test { name } => {
            let rows = core.mcp_test(name.as_deref()).await?;
            print_status(rows, json)
        }
    }
}

fn print_status(rows: Vec<pawork_app::McpServerStatus>, json: bool) -> Result<(), CliError> {
    if json {
        println!(
            "{}",
            serde_json::to_string(&rows).map_err(|error| CliError::Usage(error.to_string()))?
        );
        return Ok(());
    }
    if rows.is_empty() {
        eprintln!("(no MCP servers configured)");
        return Ok(());
    }
    for row in rows {
        let tools = if row.tools.is_empty() {
            "-".into()
        } else {
            row.tools.join(", ")
        };
        match row.last_error {
            Some(error) => println!(
                "{}\t{}\t{}\t{}\t{error}",
                row.name, row.transport, row.state, tools
            ),
            None => println!("{}\t{}\t{}\t{}", row.name, row.transport, row.state, tools),
        }
    }
    Ok(())
}
