//! `pawork service install|start|stop`：默认 dry-run，`--apply` 才改系统。

use std::path::PathBuf;
use std::process::Command;

use pawork_app::DEFAULT_INSTANCE;

use crate::ops::service_name;
use crate::{CliError, ServiceCommand};

pub fn run_service(command: ServiceCommand, instance: &str, json: bool) -> Result<(), CliError> {
    let instance = if instance.trim().is_empty() {
        DEFAULT_INSTANCE
    } else {
        instance
    };
    let (action, apply) = match command {
        ServiceCommand::Install { apply } => ("install", apply),
        ServiceCommand::Start { apply } => ("start", apply),
        ServiceCommand::Stop { apply } => ("stop", apply),
    };
    let exe = std::env::current_exe()
        .map_err(CliError::Io)?
        .display()
        .to_string();
    let name = service_name(instance);
    let plan = install_definition(&exe, &name, instance);
    let activation = activation_hint(&name);
    if json {
        println!(
            "{}",
            serde_json::json!({
                "service": name,
                "action": action,
                "dry_run": !apply,
                "plan": plan,
                "applied": apply,
                "platform": std::env::consts::OS,
            })
        );
    } else if apply {
        execute_service_action(action, &exe, &name, instance)?;
        eprintln!("applied {action} for service '{name}'");
    } else {
        eprintln!("install plan for service '{name}':\n{plan}\nthen activate:\n  {activation}");
        eprintln!("(dry-run; pass --apply to modify the system)");
    }
    Ok(())
}

fn install_definition(exe: &str, name: &str, instance: &str) -> String {
    if cfg!(windows) {
        format!(
            "sc create {name} binPath= \"\\\"{exe}\\\" --instance {instance} gui serve\" start= auto displayname= \"Pawork Core\""
        )
    } else if cfg!(target_os = "macos") {
        let plist = launchd_plist(exe, name, instance);
        format!("{plist}")
    } else if cfg!(target_os = "linux") {
        format!(
            "[Unit]\nDescription=Pawork Core\n\n[Service]\nExecStart={exe} --instance {instance} gui serve\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n"
        )
    } else {
        "unsupported platform for service install".to_string()
    }
}

fn launchd_plist(exe: &str, name: &str, instance: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{name}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>--instance</string>
    <string>{instance}</string>
    <string>gui</string>
    <string>serve</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
</dict>
</plist>
"#
    )
}

fn activation_hint(name: &str) -> String {
    if cfg!(windows) {
        format!("sc start {name}")
    } else if cfg!(target_os = "macos") {
        format!("launchctl load ~/Library/LaunchAgents/{name}.plist")
    } else if cfg!(target_os = "linux") {
        format!("systemctl --user enable --now {name}.service")
    } else {
        "unsupported platform".into()
    }
}

fn execute_service_action(
    action: &str,
    exe: &str,
    name: &str,
    instance: &str,
) -> Result<(), CliError> {
    if cfg!(windows) {
        match action {
            "install" => run_cmd(
                "sc",
                &[
                    "create",
                    name,
                    "binPath=",
                    &format!("\"{exe}\" --instance {instance} gui serve"),
                    "start=",
                    "auto",
                    "displayname=",
                    "Pawork Core",
                ],
            ),
            "start" => run_cmd("sc", &["start", name]),
            "stop" => run_cmd("sc", &["stop", name]),
            _ => Err(CliError::Usage(format!("unknown service action {action}"))),
        }
    } else if cfg!(target_os = "macos") {
        let plist_path = launch_agent_path(name)?;
        match action {
            "install" => {
                if let Some(parent) = plist_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&plist_path, launchd_plist(exe, name, instance))?;
                Ok(())
            }
            "start" => run_cmd("launchctl", &["load", &plist_path.display().to_string()]),
            "stop" => run_cmd("launchctl", &["unload", &plist_path.display().to_string()]),
            _ => Err(CliError::Usage(format!("unknown service action {action}"))),
        }
    } else if cfg!(target_os = "linux") {
        let unit = user_unit_path(name)?;
        match action {
            "install" => {
                if let Some(parent) = unit.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&unit, install_definition(exe, name, instance))?;
                Ok(())
            }
            "start" => run_cmd("systemctl", &["--user", "start", &format!("{name}.service")]),
            "stop" => run_cmd("systemctl", &["--user", "stop", &format!("{name}.service")]),
            _ => Err(CliError::Usage(format!("unknown service action {action}"))),
        }
    } else {
        Err(CliError::Usage(
            "unsupported platform for service install".into(),
        ))
    }
}

fn launch_agent_path(name: &str) -> Result<PathBuf, CliError> {
    let home = std::env::var_os("HOME").ok_or_else(|| CliError::Usage("HOME is not set".into()))?;
    Ok(PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{name}.plist")))
}

fn user_unit_path(name: &str) -> Result<PathBuf, CliError> {
    let home = std::env::var_os("HOME").ok_or_else(|| CliError::Usage("HOME is not set".into()))?;
    Ok(PathBuf::from(home)
        .join(".config/systemd/user")
        .join(format!("{name}.service")))
}

fn run_cmd(program: &str, args: &[&str]) -> Result<(), CliError> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(CliError::Io)?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Usage(format!(
            "{program} {} failed with {status}",
            args.join(" ")
        )))
    }
}
