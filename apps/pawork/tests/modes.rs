//! P13-2 四种运行模式的端到端测试：run / serve / shell / service。

use assert_cmd::cargo::cargo_bin_cmd;

/// 一次性模式：无 Provider 时 run 走完 workspace→session→run start 全流程后
/// 返回结构化错误（不 panic），退出码非零。
#[test]
fn run_mode_reports_structured_error_without_provider() {
    let dir = std::env::temp_dir().join(format!("pawork-modes-run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create run workspace");
    let output = cargo_bin_cmd!("pawork")
        .args([
            "--json",
            "run",
            "--workspace",
            dir.to_str().expect("utf-8 temp path"),
            "--prompt",
            "hello",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let _ = std::fs::remove_dir_all(&dir);

    let value: serde_json::Value = serde_json::from_slice(&output).expect("parse run JSON");
    assert_eq!(value["ok"], false);
    assert_eq!(value["kind"], "run");
    assert!(value["message"]
        .as_str()
        .expect("message")
        .contains("provider"));
    // 信封响应携带结构化错误上下文。
    assert_eq!(value["data"]["response"]["type"], "error");
}

/// 服务模式：`serve --once` 初始化后立即退出。
#[test]
fn serve_once_starts_the_same_process_core_host() {
    let output = cargo_bin_cmd!("pawork")
        .args(["serve", "--once"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).expect("UTF-8 output");
    assert!(output.contains("Pawork Core instance 'default' is ready"));
}

/// 交互模式：管道输入 REPL 命令后正常退出。
#[test]
fn shell_mode_handles_repl_commands() {
    let output = cargo_bin_cmd!("pawork")
        .args(["shell"])
        .write_stdin("/status\n/workspaces\n/quit\n")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).expect("UTF-8 output");
    assert!(output.contains("Core status"));
    assert!(output.contains("workspaces"));
}

/// 系统服务模式：install 默认 dry-run，不修改系统，输出注册计划。
#[test]
fn service_install_is_dry_run_by_default() {
    let output = cargo_bin_cmd!("pawork")
        .args(["service", "install"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).expect("UTF-8 output");
    assert!(output.contains("pawork"));
    assert!(output.contains("dry-run"));
    let platform_plan = if cfg!(target_os = "windows") {
        "sc create"
    } else if cfg!(target_os = "macos") {
        "plist"
    } else {
        "systemd"
    };
    assert!(
        output.contains(platform_plan),
        "expected platform plan '{platform_plan}' in: {output}"
    );
}
