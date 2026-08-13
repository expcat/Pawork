//! P17 §3.1 最小可用 Remote：`remote publish` 成功后长驻到 Ctrl-C。
//!
//! 覆盖：
//! - publish 输出成功后进程仍存活，listener / registry 可用；
//! - 真实跨进程连接：测试进程从 publish JSON 取 endpoint、从端点 token 文件
//!   （`TokenStore::load`）读凭证，用独立的 `RealRemoteTransport` +
//!   `RealRemoteConnector` 经真实 TCP + TLS 连入子进程 listener，关闭后再次
//!   连接成功（connect / reconnect）；
//! - SIGINT 后 close/unpublish 清理本进程创建的 endpoint token；
//! - 同名再次 publish 不因残留 token 失败；
//! - 独立进程的 unpublish/revoke 对未知 handle fail-closed；
//! - stdout 契约（P17-14）：publish 只输出一个响应，SIGINT 后 stdout 不再
//!   出现非协议文本 / 第二个响应（退出后剩余 stdout 为空）。

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use client_auth::TokenStore;
use serde_json::Value;
use transport_api::{ConnectOptions, ConnectionLocality, GuiConnection, TransportEndpoint};
use transport_remote::{
    RealRemoteConnector, RealRemoteTransport, RealRemoteTransportConfig, DEFAULT_MAX_FRAME_BYTES,
};

fn unique_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "pawork-remote-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ))
}

/// 启动 `remote publish` 并接管 stdout：返回子进程与 BufReader，保证测试能
/// 在进程退出后读取「首行 publish 响应之后的剩余 stdout」并断言为空。
fn spawn_publish(data: &Path, instance: &str, name: &str) -> (Child, BufReader<ChildStdout>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pawork"))
        .args([
            "--json",
            "--instance",
            instance,
            "remote",
            "publish",
            "--name",
            name,
        ])
        .env("PAWORK_DATA_DIR", data)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pawork remote publish");
    let stdout = child.stdout.take().expect("piped stdout");
    (child, BufReader::new(stdout))
}

fn wait_for_json_line(reader: &mut BufReader<ChildStdout>, timeout: Duration) -> Value {
    let deadline = Instant::now() + timeout;
    let mut line = String::new();
    loop {
        assert!(
            Instant::now() < deadline,
            "timeout waiting for publish JSON"
        );
        line.clear();
        let read = reader.read_line(&mut line).expect("read publish stdout");
        assert!(read > 0, "EOF before publish JSON");
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        return serde_json::from_str(trimmed)
            .unwrap_or_else(|error| panic!("publish stdout is not JSON: {trimmed:?}: {error}"));
    }
}

/// 进程退出后 stdout 必须已耗尽：除首行 publish 响应外不得再有任何输出
/// （SIGINT 收尾不得向 stdout 追加非协议文本或第二个响应）。
fn assert_remaining_stdout_empty(reader: &mut BufReader<ChildStdout>) {
    let mut rest = String::new();
    reader
        .read_to_string(&mut rest)
        .expect("drain remaining stdout");
    assert!(
        rest.trim().is_empty(),
        "stdout must be empty after the publish response, got: {rest:?}"
    );
}

fn run_remote(data: &Path, instance: &str, args: &[&str]) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_pawork"))
        .args(["--json", "--instance", instance, "remote"])
        .args(args)
        .env("PAWORK_DATA_DIR", data)
        .output()
        .expect("run remote control command");
    (
        output.status.code().expect("exit code"),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

fn token_dir(data: &Path, instance: &str) -> PathBuf {
    data.join(instance).join("remote.token.d")
}

fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !pred() {
        assert!(Instant::now() < deadline, "condition not met in time");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn interrupt(child: &Child) {
    #[cfg(unix)]
    {
        let status = Command::new("/bin/kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .expect("send SIGINT via /bin/kill");
        assert!(status.success(), "SIGINT failed: {status}");
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

#[tokio::test]
async fn remote_publish_serves_real_cross_process_client_and_reconnects_then_sigint_cleans() {
    let data = unique_dir("stay");
    std::fs::create_dir_all(&data).expect("create data dir");
    let instance = "stay";
    let (mut child, mut stdout) = spawn_publish(&data, instance, "edge");
    let value = wait_for_json_line(&mut stdout, Duration::from_secs(20));
    assert_eq!(value["ok"], true, "publish JSON: {value}");
    assert_eq!(value["data"]["action"], "publish");
    assert_eq!(value["data"]["status"], "published");

    // 从 publish JSON 取 endpoint / address（而非仅凭 handle 推导）。
    let endpoint: TransportEndpoint = serde_json::from_value(value["data"]["endpoint"].clone())
        .expect("publish data.endpoint must deserialize to TransportEndpoint");
    let address = match &endpoint {
        TransportEndpoint::Remote { address, adapter } => {
            assert_eq!(adapter, "remote");
            address.clone()
        }
        other => panic!("expected remote endpoint, got {other:?}"),
    };
    // 端点 id 取自地址并与 handle 一致；token 路径由它推导，证明凭证 ↔ 地址。
    let endpoint_id = address
        .strip_prefix("real://")
        .and_then(|rest| rest.split('?').next())
        .expect("address must carry the endpoint id");
    let handle = value["data"]["handle_id"]
        .as_str()
        .expect("handle")
        .to_string();
    assert_eq!(endpoint_id, handle, "address and handle must agree");

    // 端点独立凭证：从磁盘 token 文件加载（跨进程事实源），不做内存注入。
    let token_path = token_dir(&data, instance).join(endpoint_id).join("token");
    wait_until(Duration::from_secs(5), || token_path.exists());
    let token = TokenStore::new(&token_path)
        .load()
        .expect("load endpoint token from disk");

    // 测试进程侧独立的 client transport + connector：与子进程不共享任何状态。
    let client_transport = Arc::new(RealRemoteTransport::new(RealRemoteTransportConfig::new(
        TokenStore::new(data.join("client").join("remote.token")),
        None,
    )));
    let connector = RealRemoteConnector::new(Arc::clone(&client_transport), Some(token));
    let options = ConnectOptions {
        timeout_ms: 5_000,
        client_label: Some("pawork-e2e-remote".into()),
        max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
    };

    // 首次连接：真实 TCP + TLS 1.3（按地址指纹 pin 证书）+ 端点 token 认证。
    let first = connector
        .connect_typed(&endpoint, options.clone())
        .await
        .expect("first connect into subprocess TLS listener");
    let first_info = first.info();
    assert_eq!(first_info.locality, ConnectionLocality::Remote);
    assert!(
        first_info.encrypted,
        "first connection must be TLS-encrypted"
    );

    // 关闭 / 丢弃后重连：listener 与凭证在子进程内持续可用。
    drop(first);
    let second = connector
        .connect_typed(&endpoint, options.clone())
        .await
        .expect("reconnect into subprocess TLS listener");
    let second_info = second.info();
    assert_eq!(second_info.locality, ConnectionLocality::Remote);
    assert!(second_info.encrypted, "reconnect must be TLS-encrypted");
    assert_ne!(first_info.connection_id, second_info.connection_id);
    drop(second);

    // 连接验证完成后 publish 进程仍必须长驻（不因连接关闭而退出）。
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "publish must stay resident after success output"
    );

    interrupt(&child);

    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        match child.try_wait().expect("wait after signal") {
            Some(status) => break status,
            None => {
                assert!(
                    Instant::now() < deadline,
                    "process did not exit after SIGINT"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    assert!(
        status.success(),
        "expected clean exit after SIGINT, got {status}"
    );
    assert_remaining_stdout_empty(&mut stdout);
    wait_until(Duration::from_secs(5), || !token_path.exists());
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn remote_publish_same_name_after_cleanup_does_not_conflict() {
    let data = unique_dir("restart");
    std::fs::create_dir_all(&data).expect("create data dir");
    let instance = "restart";

    let (mut first, mut first_stdout) = spawn_publish(&data, instance, "edge");
    let first_value = wait_for_json_line(&mut first_stdout, Duration::from_secs(20));
    assert_eq!(first_value["ok"], true, "first publish: {first_value}");
    let first_handle = first_value["data"]["handle_id"]
        .as_str()
        .expect("handle")
        .to_string();
    let first_token = token_dir(&data, instance).join(&first_handle).join("token");
    wait_until(Duration::from_secs(5), || first_token.exists());

    interrupt(&first);
    let _ = first.wait();
    assert_remaining_stdout_empty(&mut first_stdout);
    wait_until(Duration::from_secs(5), || !first_token.exists());

    let (mut second, mut second_stdout) = spawn_publish(&data, instance, "edge");
    let second_value = wait_for_json_line(&mut second_stdout, Duration::from_secs(20));
    assert_eq!(second_value["ok"], true, "second publish: {second_value}");
    assert_eq!(second_value["data"]["status"], "published");
    let second_handle = second_value["data"]["handle_id"]
        .as_str()
        .expect("handle")
        .to_string();
    let second_token = token_dir(&data, instance)
        .join(&second_handle)
        .join("token");
    wait_until(Duration::from_secs(5), || second_token.exists());
    assert!(
        second.try_wait().expect("try_wait").is_none(),
        "second publish must stay resident"
    );

    interrupt(&second);
    let _ = second.wait();
    assert_remaining_stdout_empty(&mut second_stdout);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn remote_unpublish_and_revoke_without_live_host_fail_closed() {
    let data = unique_dir("fail-closed");
    std::fs::create_dir_all(&data).expect("create data dir");
    let instance = "fail-closed";

    let (unpublish_code, unpublish_out) =
        run_remote(&data, instance, &["unpublish", "--handle", "edge-0"]);
    assert_ne!(unpublish_code, 0, "unpublish stdout: {unpublish_out}");
    let unpublish: Value = serde_json::from_str(unpublish_out.trim()).expect("unpublish json");
    assert_eq!(unpublish["ok"], false);
    assert_eq!(unpublish["data"]["action"], "unpublish");
    assert!(
        unpublish["message"]
            .as_str()
            .unwrap_or_default()
            .contains("unknown remote publish handle"),
        "unpublish: {unpublish}"
    );

    let (revoke_code, revoke_out) = run_remote(&data, instance, &["revoke", "--handle", "edge-0"]);
    assert_ne!(revoke_code, 0, "revoke stdout: {revoke_out}");
    let revoke: Value = serde_json::from_str(revoke_out.trim()).expect("revoke json");
    assert_eq!(revoke["ok"], false);
    assert_eq!(revoke["data"]["action"], "revoke");
    assert!(
        revoke["message"]
            .as_str()
            .unwrap_or_default()
            .contains("unknown remote publish handle"),
        "revoke: {revoke}"
    );
    let _ = std::fs::remove_dir_all(&data);
}
