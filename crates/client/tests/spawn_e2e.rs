//! 真实命令 e2e：`PaworkClient::spawn` 启动工作区构建的 `pawork` 二进制
//! （`headless --json-stdio`），验证握手、已映射 Command/Query 往返、
//! 未映射命令/查询 fail-closed（S12 CR-07）、compat 导入与历史（经真实
//! SessionStore 持久化）以及关闭回收。

//! 二进制定位：`PAWORK_BIN` 环境变量优先，否则回退到工作区默认构建产物
//! `target/debug/pawork`；二进制不存在或尚无 `headless` 子命令时跳过
//! （本波不实现 CLI，不把 e2e 当门禁）。

use std::path::PathBuf;
use std::time::Duration;

use pawork_client::headless::{BackpressurePolicy, PaworkClient, PaworkOptions, SdkErrorKind};
use pawork_domain::{SessionId, WorkspaceId};
use pawork_protocol::headless::{CompatSource, ProtocolErrorKind, SdkCapability};
use pawork_protocol::{AppCommand, AppQuery, AppResponse, EventStream};
use serde_json::Value;

fn pawork_binary() -> Option<PathBuf> {
    let binary = if let Ok(binary) = std::env::var("PAWORK_BIN") {
        PathBuf::from(binary)
    } else {
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/pawork");
        if !fallback.exists() {
            return None;
        }
        fallback
    };
    // 本波不实现 `pawork headless`；工作区里已有的 V2 二进制若无该子命令则 skip，
    // 不把 spawn_e2e 当 CLI 门禁。
    if !supports_headless(&binary) {
        eprintln!(
            "SKIP: {} has no `headless` subcommand yet; CLI 收口后再跑本 e2e",
            binary.display()
        );
        return None;
    }
    Some(binary)
}

fn supports_headless(binary: &std::path::Path) -> bool {
    let output = std::process::Command::new(binary).arg("--help").output();
    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            stdout.contains("headless") || stderr.contains("headless")
        }
        Err(_) => false,
    }
}

/// 握手 + Command/Query 往返 + compat 持久化 + 关闭（真实进程）。
#[tokio::test]
async fn spawns_real_pawork_and_round_trips() {
    let Some(binary) = pawork_binary() else {
        eprintln!("SKIP: no usable pawork headless binary");
        return;
    };
    let data_dir = tempfile::tempdir().expect("tempdir for data");
    let root_path = data_dir.path().display().to_string();
    let options = PaworkOptions {
        binary,
        timeout: Duration::from_secs(30),
        env: vec![(
            "PAWORK_DATA_DIR".into(),
            data_dir.path().display().to_string(),
        )],
        ..PaworkOptions::default()
    };

    let client = PaworkClient::spawn(options)
        .await
        .expect("spawn + handshake");

    // 握手元信息。
    assert_eq!(
        client.api_version().await,
        Some(pawork_protocol::API_VERSION)
    );
    let instance_id = client.instance_id().await.expect("instance id");
    assert!(!instance_id.is_empty());
    let capabilities = client.capabilities().await;
    assert!(capabilities.contains(&SdkCapability::Sessions));
    assert!(capabilities.contains(&SdkCapability::CompatImport));
    assert!(capabilities.contains(&SdkCapability::CompatHistory));

    // 未映射 query fail-closed：WorkspaceList 无专属能力域，不得放行。
    let error = client
        .query(AppQuery::WorkspaceList)
        .await
        .expect_err("unmapped query must fail closed");
    assert_eq!(
        error.kind(),
        SdkErrorKind::Protocol(ProtocolErrorKind::UnsupportedCapability),
        "{error}"
    );

    // 未映射 command fail-closed：WorkspaceAdd 在能力门被拒（先于业务分发），
    // 即使路径合法也不得静默映射到已有 capability。
    let error = client
        .command(AppCommand::WorkspaceAdd {
            root_path: root_path.clone(),
        })
        .await
        .expect_err("unmapped command must fail closed");
    assert_eq!(
        error.kind(),
        SdkErrorKind::Protocol(ProtocolErrorKind::UnsupportedCapability),
        "{error}"
    );

    // 已映射 Command 往返：创建会话（SessionCreate 绑定任意 workspace_id，
    // 无需 WorkspaceAdd）。
    let workspace_id = WorkspaceId::from("ws-sdk-e2e");
    let created = client
        .command(AppCommand::SessionCreate {
            workspace_id,
            title: Some("sdk e2e".into()),
        })
        .await
        .expect("session create");
    let session_id = match created.response {
        AppResponse::Data(value) => SessionId::from(
            value
                .get("session_id")
                .and_then(Value::as_str)
                .expect("session_id"),
        ),
        other => panic!("unexpected session create response: {other:?}"),
    };
    assert!(!session_id.as_str().is_empty());

    // 已映射 Query 往返（同一 AppService 分发）。
    let response = client
        .query(AppQuery::SessionGet {
            session_id: session_id.clone(),
            timeline_after_sequence: None,
            timeline_limit: None,
        })
        .await
        .expect("mapped query round trip");
    assert!(matches!(response.response, AppResponse::Data(_)));

    // 订阅不报错（真实进程的事件流槽位）。
    let _subscription = client
        .subscribe(EventStream::Global, BackpressurePolicy::Error, 64)
        .await
        .expect("subscribe global");

    // compat 导入 → 真实 SessionStore 持久化；历史可查。
    let outcome = client
        .import_compat(
            CompatSource::Claude,
            r#"{"conversation_id":"e2e-1","chat_messages":[{"sender":"human","text":"hello"},{"sender":"assistant","text":"hi"}]}"#
                .into(),
            false,
        )
        .await
        .expect("compat import");
    assert_eq!(outcome.report.imported_messages, 2);
    assert!(!outcome.report.session_id.is_empty());

    let page = client
        .compat_history(Some(10), None)
        .await
        .expect("compat history");
    assert_eq!(page.entries.len(), 1, "import persisted into history");
    assert_eq!(page.entries[0].source, CompatSource::Claude);

    client.close().await.expect("close");
}

/// 无 provider 时 RunStart 返回显式业务错误（错误信封直通，不崩溃）。
#[tokio::test]
async fn run_start_without_provider_returns_error_response() {
    let Some(binary) = pawork_binary() else {
        eprintln!("SKIP: no usable pawork headless binary");
        return;
    };
    let data_dir = tempfile::tempdir().expect("tempdir for data");
    let options = PaworkOptions {
        binary,
        timeout: Duration::from_secs(30),
        working_dir: Some(data_dir.path().to_path_buf()),
        isolated: true,
        env: vec![
            (
                "PAWORK_DATA_DIR".into(),
                data_dir.path().display().to_string(),
            ),
            ("PAWORK_HOME".into(), data_dir.path().display().to_string()),
        ],
        ..PaworkOptions::default()
    };

    let client = PaworkClient::spawn(options).await.expect("spawn");
    // 未映射 command fail-closed：WorkspaceAdd 在能力门被拒。
    let error = client
        .command(AppCommand::WorkspaceAdd {
            root_path: data_dir.path().display().to_string(),
        })
        .await
        .expect_err("unmapped command must fail closed");
    assert_eq!(
        error.kind(),
        SdkErrorKind::Protocol(ProtocolErrorKind::UnsupportedCapability),
        "{error}"
    );

    let workspace_id = WorkspaceId::from("ws-sdk-e2e");
    let created = client
        .command(AppCommand::SessionCreate {
            workspace_id,
            title: Some("sdk e2e".into()),
        })
        .await
        .expect("session create");
    let session_id = match created.response {
        AppResponse::Data(value) => SessionId::from(
            value
                .get("session_id")
                .and_then(Value::as_str)
                .expect("session_id"),
        ),
        other => panic!("unexpected session create response: {other:?}"),
    };
    let response = client
        .command(AppCommand::RunStart {
            session_id,
            user_message: "hello".into(),
            model: None,
            provider: None,
            profile: None,
        })
        .await
        .expect("run start responds");
    assert!(
        matches!(response.response, AppResponse::Error(_)),
        "no provider → explicit error, got: {response:?}"
    );
    client.close().await.expect("close");
}

/// 真实 Host 强制能力门：只授予 Sessions 时，未授予的 Runs / CompatImport
/// 被显式拒绝（UnsupportedCapability），通用 query 仍可用。
#[tokio::test]
async fn real_host_enforces_granted_capabilities() {
    let Some(binary) = pawork_binary() else {
        eprintln!("SKIP: no usable pawork headless binary");
        return;
    };
    let data_dir = tempfile::tempdir().expect("tempdir for data");
    let options = PaworkOptions {
        binary,
        timeout: Duration::from_secs(30),
        env: vec![(
            "PAWORK_DATA_DIR".into(),
            data_dir.path().display().to_string(),
        )],
        capabilities: vec![SdkCapability::Sessions],
        ..PaworkOptions::default()
    };

    let client = PaworkClient::spawn(options)
        .await
        .expect("spawn + handshake");
    assert_eq!(
        client.capabilities().await,
        vec![SdkCapability::Sessions],
        "host grants exactly the requested subset"
    );

    // 已映射 + 已授予：Sessions 内的 Command 正常往返。
    let created = client
        .command(AppCommand::SessionCreate {
            workspace_id: WorkspaceId::from("ws-sdk-e2e"),
            title: Some("gate e2e".into()),
        })
        .await
        .expect("mapped command allowed with Sessions grant");
    let session_id = match created.response {
        AppResponse::Data(value) => SessionId::from(
            value
                .get("session_id")
                .and_then(Value::as_str)
                .expect("session_id"),
        ),
        other => panic!("unexpected session create response: {other:?}"),
    };
    // 未映射 query：即使已有授予也 fail-closed。
    let response = client
        .query(AppQuery::WorkspaceList)
        .await
        .expect_err("unmapped query fails closed even with grant");
    assert_eq!(
        response.kind(),
        SdkErrorKind::Protocol(ProtocolErrorKind::UnsupportedCapability),
        "{response}"
    );
    // 已映射 + 已授予：Sessions 内的 Query 正常往返。
    let response = client
        .query(AppQuery::SessionGet {
            session_id,
            timeline_after_sequence: None,
            timeline_limit: None,
        })
        .await
        .expect("mapped query allowed with Sessions grant");
    assert!(matches!(response.response, AppResponse::Data(_)));

    // Runs 未授予 → RunStart 被 Host 能力门显式拒绝（先于业务分发）。
    let error = client
        .command(AppCommand::RunStart {
            session_id: SessionId::from("s-none"),
            user_message: "hello".into(),
            model: None,
            provider: None,
            profile: None,
        })
        .await
        .expect_err("run start must be rejected without Runs grant");
    assert_eq!(
        error.kind(),
        SdkErrorKind::Protocol(ProtocolErrorKind::UnsupportedCapability),
        "{error}"
    );

    // CompatImport 未授予 → 显式拒绝。
    let error = client
        .import_compat(
            CompatSource::Claude,
            r#"{"conversation_id":"c-1","chat_messages":[{"sender":"human","text":"hi"}]}"#.into(),
            false,
        )
        .await
        .expect_err("compat import must be rejected without CompatImport grant");
    assert_eq!(
        error.kind(),
        SdkErrorKind::Protocol(ProtocolErrorKind::UnsupportedCapability),
        "{error}"
    );

    client.close().await.expect("close");
}
