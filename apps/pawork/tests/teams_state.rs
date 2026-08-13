//! P17-6 状态收敛：普通 CLI 启动不再无条件打开 teams.sqlite。
//!
//! 当前尚无 canonical ingress，正式宿主必须保持 `CoreRuntime` 的
//! `team_db_path: None`（Team store 走内存实现）。本测试用临时数据目录与
//! 命名实例真实启动 `pawork serve --once`，验证实例目录中不产生
//! `teams.sqlite`。

use assert_cmd::cargo::cargo_bin_cmd;
use tempfile::TempDir;

#[test]
fn normal_cli_startup_does_not_create_teams_sqlite() {
    let data_dir = TempDir::new().expect("create temp data dir");
    let instance = format!("teams-probe-{}", std::process::id());

    cargo_bin_cmd!("pawork")
        .env("PAWORK_DATA_DIR", data_dir.path())
        .args(["--instance", &instance, "serve", "--once"])
        .assert()
        .success();

    let teams_db = data_dir.path().join(&instance).join("teams.sqlite");
    assert!(
        !teams_db.exists(),
        "normal CLI startup must not create durable Team state at {}",
        teams_db.display()
    );
}
