use std::sync::Arc;

use agent_domain::{CancellationToken, ProviderId, RunId, StopReason, TokenUsage};
use assert_cmd::cargo::cargo_bin_cmd;
use async_trait::async_trait;
use clap::Parser;
use cli_command::Cli;
use cli_host::CliHost;
use core_runtime::CoreRuntime;
use provider_api::{
    CanonicalModelRequest, ModelDefinition, ModelProvider, ModelResponseSummary, ProviderError,
    ProviderEventSink, ProviderStreamEvent, ResolvedCredential,
};
use usage_ledger::UsageLedger as _;

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

/// P18-8 跨进程 CLI 定向测试：另一个进程写入同一 SQLite 账本后，新启动的
/// `pawork` 进程必须在装配时 replay 该账本进本地 Quota 缓存，`usage` 读到
/// 与写入进程完全相同的聚合（run→ledger→usage/quota 单一事实源；进程内
/// 内存账本绝不是可接受的第二累计源）。
#[tokio::test]
async fn usage_in_fresh_process_reads_ledger_written_by_another_process() {
    let dir = tempfile::tempdir().expect("tempdir");
    let instance = "p18-8-xproc";
    let ledger_path = dir.path().join(instance).join("usage-ledger.sqlite3");

    // “另一个进程”：直接打开同一路径的持久账本，写入一条真实用量记录。
    {
        let ledger =
            usage_ledger::SqliteUsageLedger::open(&ledger_path).expect("open seeded ledger");
        let record = usage_ledger::UsageRecord {
            record_id: "xproc-seed-1".into(),
            // 与 run 记账路径一致：canonical 身份租户 local/default。
            tenant_id: agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT_CANONICAL),
            principal_id: agent_domain::PrincipalId::default(),
            account_id: core_api::DEFAULT_QUOTA_ACCOUNT.to_string(),
            session_id: agent_domain::SessionId::default(),
            agent_id: agent_domain::AgentId::default(),
            run_id: Some(agent_domain::RunId::from("run-xproc")),
            provider_id: agent_domain::ProviderId::from("mock"),
            model_id: agent_domain::ModelId::from("mock-model"),
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_micros: 0,
            currency: "USD".into(),
            occurred_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before epoch")
                .as_millis() as u64,
            ..usage_ledger::UsageRecord::default()
        };
        ledger.record(record).await.expect("seed record");
    }

    // 新进程装配同一实例目录：必须 fail-closed 打开并 replay 持久账本。
    let output = cargo_bin_cmd!("pawork")
        .env("PAWORK_DATA_DIR", dir.path())
        .args([
            "--instance",
            instance,
            "--json",
            "usage",
            "--provider",
            "mock",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).expect("parse usage JSON");
    assert_eq!(value["ok"], true, "output: {value}");
    assert_eq!(value["kind"], "usage");
    let data = &value["data"]["response"]["data"];
    assert_eq!(
        data["from_cache"], true,
        "新进程必须 replay 账本进本地缓存：{value}"
    );
    let windows = data["windows"].as_array().expect("windows array");
    let ok_read = windows.iter().find_map(|window| {
        let read = window.get("read")?;
        (read.get("status")?.as_str()? == "ok").then_some(read)
    });
    let read = ok_read.expect("expected an ok window reading the seeded ledger");
    let used = &read["snapshot"]["values"]["used"];
    assert_eq!(used["kind"], "exact", "跨进程聚合必须来自账本：{value}");
    assert_eq!(used["value"], 150, "跨进程聚合 100 in + 50 out：{value}");
}

/// 单轮完成的 mock provider：一次 `UsageUpdated` + `TextDelta` +
/// `ResponseCompleted`。CLI 二进制尚未接线真实 provider，回归在测试进程内
/// 用两个独立装配模拟两个独立 `pawork` 进程（各自全新 AggregateState /
/// CommandRouter / QuotaRuntime，共享同一持久账本文件）。
struct CliMockProvider {
    usage: TokenUsage,
}

#[async_trait]
impl ModelProvider for CliMockProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from("mock")
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream(
        &self,
        _request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        _cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        sink.emit(ProviderStreamEvent::UsageUpdated(self.usage.clone()))
            .await?;
        sink.emit(ProviderStreamEvent::TextDelta("done".into()))
            .await?;
        sink.emit(ProviderStreamEvent::ResponseCompleted(
            StopReason::Completed,
        ))
        .await?;
        Ok(ModelResponseSummary {
            stop_reason: StopReason::Completed,
            usage: self.usage.clone(),
            response_id: None,
            provider_metadata: serde_json::Value::Null,
        })
    }
}

/// 以持久账本装配一个独立“进程”，经 CLI run 路径执行一次 mock run，
/// 等待终态后完全释放账本连接（shutdown pump），返回该 run 的 RunId。
async fn run_mock_cli_once(instance: &str, ledger_path: &std::path::Path, prompt: &str) -> RunId {
    let runtime = CoreRuntime::with_persistent_ledger(instance, ledger_path)
        .await
        .expect("open persistent ledger");
    runtime.register_provider(Arc::new(CliMockProvider {
        usage: TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        },
    }));
    let host = CliHost::with_hub(runtime.service().clone(), runtime.hub().clone());
    let cli = Cli::try_parse_from(["pawork", "run", "--workspace", ".", "--prompt", prompt])
        .expect("parse run args");
    let outcome = host.execute(cli).await;
    assert_eq!(outcome.exit_code, 0, "run failed: {}", outcome.output);
    assert!(
        outcome.output.contains("finished: Completed"),
        "run must complete: {}",
        outcome.output
    );
    let run_id = runtime
        .service()
        .router()
        .last_started_run()
        .expect("run id recorded after run start");
    // 释放账本连接：让下一个“进程”/真实进程可以重开同一文件。
    drop(host);
    runtime.shutdown();
    for _ in 0..200 {
        if runtime.pump_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    drop(runtime);
    run_id
}

/// P18-8 跨进程 RunId 碰撞回归：两个独立装配（等价两个独立 `pawork` 进程）
/// 先后对同一持久账本各 run 一次，累计用量必须不丢：
/// 1) 两个 run 的 RunId 互不相同——旧实现 `aggregate.next_id` 每进程从 0
///    计数，两侧同为 `run-1`；request_id 由 run_id 派生，账本按
///    (tenant, account, request_id, attempt) 去重会误伤另一进程的记账
///    记录（`usage ledger record failed; run usage not persisted`）；
/// 2) 每 run 的账本记录完整（各 100+50=150）；
/// 3) 真实 `pawork` 进程重开同一账本后 `usage` 聚合 = 300（不重复不丢）。
#[tokio::test]
async fn two_independent_processes_run_same_ledger_cumulative_without_loss() {
    let dir = tempfile::tempdir().expect("tempdir");
    let instance = "p18-8-xproc-runs";
    let ledger_path = dir.path().join(instance).join("usage-ledger.sqlite3");

    // 进程 A 与进程 B：同一数据目录、同一账本文件，各 run 一次。
    let run_a = run_mock_cli_once("p18-8-proc-a", &ledger_path, "first run").await;
    let run_b = run_mock_cli_once("p18-8-proc-b", &ledger_path, "second run").await;

    // 1) 跨“进程” RunId 唯一。
    assert_ne!(run_a, run_b, "两个独立进程的 run id 必须互不相同");
    assert!(run_a.as_str().starts_with("run-"), "{run_a}");
    assert!(run_b.as_str().starts_with("run-"), "{run_b}");

    // 2) 账本累计不丢：每 run 100+50，两 run 共 300。
    let ledger = usage_ledger::SqliteUsageLedger::open(&ledger_path).expect("open shared ledger");
    let mut total = 0u64;
    for run_id in [&run_a, &run_b] {
        let records = ledger
            .query(&usage_ledger::UsageQuery::by_run(run_id.clone()))
            .await
            .expect("query by run");
        assert!(!records.is_empty(), "run {run_id} 必须留有记账记录");
        let sum: u64 = records
            .iter()
            .map(|record| record.input_tokens + record.output_tokens)
            .sum();
        assert_eq!(sum, 150, "run {run_id} 用量必须完整持久化");
        total += sum;
    }
    assert_eq!(total, 300, "跨进程累计必须不丢");

    // 3) 真实 `pawork` 进程重开同一账本：usage 聚合仍为累计 300。
    let output = cargo_bin_cmd!("pawork")
        .env("PAWORK_DATA_DIR", dir.path())
        .args([
            "--instance",
            instance,
            "--json",
            "usage",
            "--provider",
            "mock",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).expect("parse usage JSON");
    assert_eq!(value["ok"], true, "output: {value}");
    assert_eq!(value["kind"], "usage");
    let data = &value["data"]["response"]["data"];
    assert_eq!(
        data["from_cache"], true,
        "新进程必须 replay 账本进本地缓存：{value}"
    );
    let windows = data["windows"].as_array().expect("windows array");
    let ok_read = windows.iter().find_map(|window| {
        let read = window.get("read")?;
        (read.get("status")?.as_str()? == "ok").then_some(read)
    });
    let read = ok_read.expect("expected an ok window reading the ledger");
    let used = &read["snapshot"]["values"]["used"];
    assert_eq!(used["kind"], "exact", "跨进程聚合必须来自账本：{value}");
    assert_eq!(used["value"], 300, "两进程 run 后累计用量：{value}");
}
