//! 崩溃 restart 状态机：崩溃 → 自动重启 → 重初始化 → resync；预算耗尽；显式 restart。

mod common;
use common::{full_capabilities, test_descriptor, MockAction, MockServerSpec, MockSpawner};

use lsp_runtime::{ClientCapabilities, LanguageClient, LspClient, Phase, ServerSpawnConfig};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

async fn wait_until<F: Fn() -> bool>(cond: F) {
    for _ in 0..100 {
        if cond() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("condition not met within timeout");
}

fn stable_handler() -> common::MockHandler {
    Arc::new(|_m, _p, _id| MockAction::Respond(json!({})))
}

fn crash_on_initialize_handler() -> common::MockHandler {
    Arc::new(|_m, _p, _id| MockAction::Crash)
}

fn crash_on_hover_handler() -> common::MockHandler {
    Arc::new(|method, _p, _id| match method {
        "textDocument/hover" => MockAction::Crash,
        _ => MockAction::Respond(json!({})),
    })
}

#[tokio::test]
async fn crash_triggers_restart_and_recovers() {
    let crash_on_hover = crash_on_hover_handler();
    let stable = stable_handler();
    let spawner = MockSpawner::new(vec![
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: crash_on_hover,
            init_delay: None,
        },
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: stable,
            init_delay: None,
        },
    ])
    .into_shared();

    let client = LspClient::start(
        test_descriptor("rust"),
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    assert_eq!(client.phase().await, Phase::Initialized);

    let lc =
        LanguageClient::new(client).with_request_timeout(std::time::Duration::from_millis(800));
    let _ = lc
        .hover("file:///a.rs", lsp_runtime::Position::new(0, 0), None)
        .await;
    assert!(
        lc.lsp()
            .wait_restarted(std::time::Duration::from_secs(3))
            .await
    );
    assert_eq!(lc.lsp().phase().await, Phase::Initialized);
    assert_eq!(lc.lsp().restart_count().await, 1);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn restart_budget_exhaustion_enters_failed() {
    let mut desc = test_descriptor("rust");
    desc.max_restarts = 1;
    let crash_on_hover = crash_on_hover_handler();
    let spawner = MockSpawner::new(vec![
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: crash_on_hover.clone(),
            init_delay: None,
        },
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: crash_on_hover,
            init_delay: None,
        },
    ])
    .into_shared();

    let client = LspClient::start(
        desc,
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let lc =
        LanguageClient::new(client).with_request_timeout(std::time::Duration::from_millis(800));
    let _ = lc
        .hover("file:///a.rs", lsp_runtime::Position::new(0, 0), None)
        .await;
    assert!(
        lc.lsp()
            .wait_restarted(std::time::Duration::from_secs(3))
            .await
    );
    let _ = lc
        .hover("file:///a.rs", lsp_runtime::Position::new(0, 0), None)
        .await;
    assert!(
        lc.lsp()
            .wait_restarted(std::time::Duration::from_secs(3))
            .await
    );
    assert_eq!(lc.lsp().phase().await, Phase::Failed);
}

#[tokio::test]
async fn max_restarts_zero_disables_restart_entirely() {
    let mut desc = test_descriptor("rust");
    desc.max_restarts = 0;
    let crash_on_hover = crash_on_hover_handler();
    let spawner = MockSpawner::new(vec![MockServerSpec {
        capabilities: full_capabilities(),
        handler: crash_on_hover,
        init_delay: None,
    }])
    .into_shared();

    let client = LspClient::start(
        desc,
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let lc =
        LanguageClient::new(client).with_request_timeout(std::time::Duration::from_millis(800));
    let _ = lc
        .hover("file:///a.rs", lsp_runtime::Position::new(0, 0), None)
        .await;
    assert!(
        lc.lsp()
            .wait_restarted(std::time::Duration::from_secs(3))
            .await
    );
    assert_eq!(lc.lsp().phase().await, Phase::Failed);
    // 预算 0 → 一次都没重启。
    assert_eq!(lc.lsp().restart_count().await, 0);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn explicit_restart_reinitializes() {
    let spawner = MockSpawner::single(stable_handler()).into_shared();
    let client = LspClient::start(
        test_descriptor("rust"),
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let lc = LanguageClient::new(client).with_request_timeout(std::time::Duration::from_secs(2));
    lc.did_open("file:///a.rs", "rust", "fn main() {}\n")
        .await
        .unwrap();
    lc.lsp()
        .restart(std::time::Duration::from_secs(3))
        .await
        .expect("restart");
    assert_eq!(lc.lsp().phase().await, Phase::Initialized);
    assert_eq!(lc.lsp().restart_count().await, 1);
    lc.did_change_full("file:///a.rs", "updated").await.unwrap();
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn crash_resync_reopens_documents() {
    let seen = Arc::new(AtomicUsize::new(0));
    let seen_clone = seen.clone();
    let stable: common::MockHandler = Arc::new(move |method, _p, _id| {
        if method == "textDocument/didOpen" {
            seen_clone.fetch_add(1, Ordering::SeqCst);
        }
        MockAction::Respond(json!({}))
    });
    let crash_on_hover: common::MockHandler = Arc::new(|method, _p, _id| match method {
        "textDocument/hover" => MockAction::Crash,
        _ => MockAction::Respond(json!({})),
    });
    let spawner = MockSpawner::new(vec![
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: crash_on_hover,
            init_delay: None,
        },
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: stable,
            init_delay: None,
        },
    ])
    .into_shared();
    let client = LspClient::start(
        test_descriptor("rust"),
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let lc =
        LanguageClient::new(client).with_request_timeout(std::time::Duration::from_millis(800));
    lc.did_open("file:///a.rs", "rust", "x").await.unwrap();
    lc.did_open("file:///b.rs", "rust", "y").await.unwrap();
    let _ = lc
        .hover("file:///a.rs", lsp_runtime::Position::new(0, 0), None)
        .await;
    assert!(
        lc.lsp()
            .wait_restarted(std::time::Duration::from_secs(3))
            .await
    );
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(seen.load(Ordering::SeqCst), 2);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn settings_are_resent_after_crash_restart() {
    let count = Arc::new(AtomicUsize::new(0));
    let count_a = count.clone();
    let count_b = count.clone();
    let crash_on_hover: common::MockHandler = Arc::new(move |method, _p, _id| {
        if method == "workspace/didChangeConfiguration" {
            count_a.fetch_add(1, Ordering::SeqCst);
        }
        match method {
            "textDocument/hover" => MockAction::Crash,
            _ => MockAction::Respond(json!({})),
        }
    });
    let stable: common::MockHandler = Arc::new(move |method, _p, _id| {
        if method == "workspace/didChangeConfiguration" {
            count_b.fetch_add(1, Ordering::SeqCst);
        }
        MockAction::Respond(json!({}))
    });
    let spawner = MockSpawner::new(vec![
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: crash_on_hover,
            init_delay: None,
        },
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: stable,
            init_delay: None,
        },
    ])
    .into_shared();
    let mut desc = test_descriptor("rust");
    desc.settings = Some(Value::String("lsp-settings".into()));
    let client = LspClient::start(
        desc,
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let lc =
        LanguageClient::new(client).with_request_timeout(std::time::Duration::from_millis(800));
    // 初次握手发送一次。
    wait_until(|| count.load(Ordering::SeqCst) >= 1).await;
    let _ = lc
        .hover("file:///a.rs", lsp_runtime::Position::new(0, 0), None)
        .await;
    assert!(
        lc.lsp()
            .wait_restarted(std::time::Duration::from_secs(3))
            .await
    );
    // restart 握手后重发。
    wait_until(|| count.load(Ordering::SeqCst) >= 2).await;
    assert_eq!(count.load(Ordering::SeqCst), 2);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn initial_start_handshake_crash_closes_every_spawned_lifecycle() {
    // restart 握手 / 写失败必须关闭新 lifecycle：initialize 握手即崩溃时，
    // start 必须失败，且每次 spawn（初始 1 次 + 预算内重启尝试）产出的
    // lifecycle 都必须被 close，绝不泄漏 mock 进程。
    let mut desc = test_descriptor("rust");
    desc.max_restarts = 2;
    let spawner = MockSpawner::new(vec![MockServerSpec {
        capabilities: full_capabilities(),
        handler: crash_on_initialize_handler(),
        init_delay: None,
    }]);
    let stats = spawner.stats.clone();
    let result = LspClient::start(
        desc,
        spawner.into_shared(),
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await;
    assert!(result.is_err(), "initialize 握手崩溃必须让 start 失败");
    assert_eq!(
        stats.lifecycle_closes.load(Ordering::SeqCst),
        3,
        "初始 spawn + 预算内 2 次 restart 尝试的 lifecycle 都必须关闭"
    );
}

#[tokio::test]
async fn restart_retries_handshake_within_budget_until_stable() {
    // 连续重启预算语义：restart 尝试在握手期失败不是终点——预算内按同一
    // restart_count 继续尝试（每次尝试都计入预算），稳定后才恢复 Initialized。
    let mut desc = test_descriptor("rust");
    desc.max_restarts = 3;
    let spawner = MockSpawner::new(vec![
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: crash_on_hover_handler(),
            init_delay: None,
        },
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: crash_on_initialize_handler(),
            init_delay: None,
        },
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: stable_handler(),
            init_delay: None,
        },
    ])
    .into_shared();
    let client = LspClient::start(
        desc,
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let lc =
        LanguageClient::new(client).with_request_timeout(std::time::Duration::from_millis(800));
    let _ = lc
        .hover("file:///a.rs", lsp_runtime::Position::new(0, 0), None)
        .await;
    assert!(
        lc.lsp()
            .wait_restarted(std::time::Duration::from_secs(3))
            .await
    );
    assert_eq!(lc.lsp().phase().await, Phase::Initialized);
    // 第一次尝试（握手崩溃）失败后仍在预算内继续：两次尝试都计入预算。
    assert_eq!(lc.lsp().restart_count().await, 2);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn restart_budget_exhaustion_on_handshake_failure_enters_failed() {
    // 预算语义收口：所有 restart 尝试都在握手期失败 → 预算耗尽才进入 Failed；
    // 期间每次 spawn 的 lifecycle 都被关闭。
    let mut desc = test_descriptor("rust");
    desc.max_restarts = 2;
    let spawner = MockSpawner::new(vec![
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: crash_on_hover_handler(),
            init_delay: None,
        },
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: crash_on_initialize_handler(),
            init_delay: None,
        },
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: crash_on_initialize_handler(),
            init_delay: None,
        },
    ]);
    let stats = spawner.stats.clone();
    let client = LspClient::start(
        desc,
        spawner.into_shared(),
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let lc =
        LanguageClient::new(client).with_request_timeout(std::time::Duration::from_millis(800));
    let _ = lc
        .hover("file:///a.rs", lsp_runtime::Position::new(0, 0), None)
        .await;
    assert!(
        lc.lsp()
            .wait_restarted(std::time::Duration::from_secs(3))
            .await
    );
    assert_eq!(lc.lsp().phase().await, Phase::Failed);
    assert_eq!(lc.lsp().restart_count().await, 2);
    // 初始 lifecycle + 2 次失败尝试的 lifecycle 全部关闭，无一泄漏。
    assert_eq!(stats.lifecycle_closes.load(Ordering::SeqCst), 3);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn requests_during_restart_fail_fast_and_new_generation_recovers() {
    // 崩溃代际隔离：restart 窗口内（writer 已摘除）新请求快速失败、不滞留
    // pending；restart settle 后新代际请求正常服务，不被旧代际清理误伤。
    let hover_ok = json!({
        "contents": { "kind": "markdown", "value": "ok" }
    });
    let delayed_stable: common::MockHandler = Arc::new(move |method, _p, _id| match method {
        "textDocument/hover" => MockAction::Respond(hover_ok.clone()),
        _ => MockAction::Respond(json!({})),
    });
    let spawner = MockSpawner::new(vec![
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: crash_on_hover_handler(),
            init_delay: None,
        },
        MockServerSpec {
            capabilities: full_capabilities(),
            handler: delayed_stable,
            init_delay: Some(Duration::from_millis(400)),
        },
    ])
    .into_shared();
    let client = LspClient::start(
        test_descriptor("rust"),
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let lc =
        LanguageClient::new(client).with_request_timeout(std::time::Duration::from_millis(800));
    // 触发崩溃：notify 不等待响应，mock 收到后崩溃 → reader 进入 restart。
    // （in-flight 请求的旧代际清理由 crash_triggers_restart_and_recovers 覆盖。）
    lc.lsp()
        .notify("textDocument/hover", None)
        .await
        .expect("notify 写入成功");
    // 等 restart 进入握手窗口（phase=Restarting 之后 writer 已被摘除）。
    for _ in 0..100 {
        if lc.lsp().phase().await == Phase::Restarting {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(lc.lsp().phase().await, Phase::Restarting);
    tokio::time::sleep(Duration::from_millis(20)).await;
    // 窗口内新请求快速失败（writer 不可用），不滞留 pending。
    let started = Instant::now();
    let err = lc
        .hover("file:///a.rs", lsp_runtime::Position::new(0, 0), None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, lsp_runtime::LspError::Transport(_)),
        "restart 窗口内请求必须快速失败，got {err:?}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "restart 窗口内请求不得滞留到超时"
    );
    // settle 后新代际请求正常服务。
    assert!(
        lc.lsp()
            .wait_restarted(std::time::Duration::from_secs(3))
            .await
    );
    assert_eq!(lc.lsp().phase().await, Phase::Initialized);
    let res = common::inline(
        lc.hover("file:///a.rs", lsp_runtime::Position::new(0, 0), None)
            .await
            .expect("restart 后请求正常"),
    );
    assert!(res.is_some());
    lc.shutdown().await.unwrap();
}
