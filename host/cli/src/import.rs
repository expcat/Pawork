//! `pawork import <tool>`：compat 配置导入向导。

use std::io::{self, IsTerminal, Write};

use pawork_app::{AppCore, CompatTool};

use crate::CliError;

pub async fn run_import(
    core: &AppCore,
    tool: String,
    yes: bool,
    dry_run: bool,
    json: bool,
) -> Result<(), CliError> {
    let source = CompatTool::parse(&tool)?;
    let preview = core.preview_compat_import(source, None)?;
    if json && dry_run {
        println!(
            "{}",
            serde_json::to_string(&preview).map_err(|error| CliError::Usage(error.to_string()))?
        );
        return Ok(());
    }
    if !json {
        println!("{}", preview.preview);
        if preview.items.is_empty() {
            eprintln!("(no importable items)");
        }
    }
    if dry_run {
        return Ok(());
    }
    if !yes && !confirm(&preview)? {
        eprintln!("cancelled");
        return Ok(());
    }
    let report = core.apply_compat_import(source, None)?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|error| CliError::Usage(error.to_string()))?
        );
        return Ok(());
    }
    println!(
        "imported {} item(s), skipped {}, plan {}",
        report.applied.len(),
        report.skipped.len(),
        report.plan_path.display()
    );
    for id in &report.applied {
        println!("  applied {id}");
    }
    for id in &report.skipped {
        println!("  skipped {id}");
    }
    if report.sources_unchanged {
        println!("source files unchanged (mtime/content)");
    }
    Ok(())
}

fn confirm(preview: &pawork_app::CompatImportPreview) -> Result<bool, CliError> {
    if !io::stdin().is_terminal() {
        return Err(CliError::Usage(
            "non-interactive import requires --yes (or --dry-run)".into(),
        ));
    }
    eprint!(
        "Apply {} item(s) from {}? [y/N] ",
        preview.items.len(),
        preview.tool
    );
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let answer = line.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}
