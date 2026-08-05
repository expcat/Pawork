use assert_cmd::cargo::cargo_bin_cmd;

#[test]
fn doctor_returns_stable_json() {
    let output = cargo_bin_cmd!("pawork")
        .args(["--json", "doctor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).expect("parse doctor JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["kind"], "doctor");
    assert_eq!(value["data"]["ok"], true);
}

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
