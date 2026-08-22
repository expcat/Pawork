//! pawork auth：list / set-key / login / logout。
//!
//! stdout 纪律：--json 时 stdout 只承载 JSON；URL、提示与结果说明走 stderr。
//! 明文 key 只经 stdin 进入 auth 文件（0600），不回显、不落日志。

use std::time::Duration;

use pawork_app::AppCore;
use pawork_app::OAuthLogin;

use crate::{AuthCommand, CliError};

const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub async fn run_auth(core: &AppCore, command: AuthCommand, json: bool) -> Result<(), CliError> {
    match command {
        AuthCommand::List => list(core, json),
        AuthCommand::SetKey { provider } => set_key(core, &provider, json),
        AuthCommand::Login { provider } => login(core, &provider, json).await,
        AuthCommand::Logout { provider } => logout(core, &provider, json),
    }
}

fn list(core: &AppCore, json: bool) -> Result<(), CliError> {
    let rows = core.auth_status()?;
    if json {
        let payload: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                serde_json::json!({
                    "provider": row.provider,
                    "kind": row.kind,
                    "source": row.source.as_str(),
                    "masked": row.masked,
                    "expires_at_ms": row.expires_at_ms,
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&payload).map_err(json_err)?);
        return Ok(());
    }
    println!("{:<16} {:<8} {:<9} {:<20} {}", "PROVIDER", "KIND", "SOURCE", "MASKED", "EXPIRES");
    for row in rows {
        let masked = row.masked.as_deref().unwrap_or("-");
        let expires = row
            .expires_at_ms
            .map(|ms| crate::sessions::format_millis(ms as i64))
            .unwrap_or_else(|| "-".into());
        println!(
            "{:<16} {:<8} {:<9} {:<20} {}",
            row.provider,
            row.kind,
            row.source.as_str(),
            masked,
            expires
        );
    }
    Ok(())
}

fn set_key(core: &AppCore, provider: &str, json: bool) -> Result<(), CliError> {
    eprintln!("输入 {provider} 的 API key（stdin 单行，输入不回显于日志）：");
    let mut secret = String::new();
    std::io::stdin().read_line(&mut secret)?;
    let masked = core.auth_set_key(provider, &secret)?;
    if json {
        println!(
            "{}",
            serde_json::json!({"provider": provider, "masked": masked.as_str()})
        );
    } else {
        eprintln!("已写入 auth 文件：{provider} {}", masked.as_str());
    }
    Ok(())
}

async fn login(core: &AppCore, provider: &str, json: bool) -> Result<(), CliError> {
    let login = core.oauth_begin(provider).await?;
    match &login {
        OAuthLogin::Pkce { auth_url, .. } => {
            eprintln!("在浏览器完成登录（最长等待 5 分钟）：");
            eprintln!("{auth_url}");
        }
        OAuthLogin::Device { prompt, .. } => {
            eprintln!("打开验证页并输入代码完成登录（最长等待 5 分钟）：");
            eprintln!("{}", prompt.verification_uri);
            if let Some(uri) = &prompt.verification_uri_complete {
                eprintln!("（或直接访问 {uri}）");
            }
            eprintln!("代码：{}", prompt.user_code);
        }
    }
    let stored = core.oauth_complete(login, LOGIN_TIMEOUT).await?;
    if json {
        println!(
            "{}",
            serde_json::json!({"provider": provider, "masked": stored.masked.as_str()})
        );
    } else {
        eprintln!("已写入 auth 文件：{provider} {}", stored.masked.as_str());
    }
    Ok(())
}

fn logout(core: &AppCore, provider: &str, json: bool) -> Result<(), CliError> {
    core.auth_logout(provider)?;
    if json {
        println!("{}", serde_json::json!({"provider": provider, "status": "logged_out"}));
    } else {
        eprintln!("已删除 auth 文件 default 条目：{provider}（env fallback 不受影响）");
    }
    Ok(())
}

fn json_err(error: serde_json::Error) -> CliError {
    CliError::Turn(error.to_string())
}
