//! protocol-probe --self-test：进程内 MemoryTransport + GuiHostAdapter 的 10 场景。

#[path = "probe/harness.rs"]
mod harness;
#[path = "probe/scenarios.rs"]
mod scenarios;

#[tokio::test]
async fn self_test_all_scenarios() {
    assert_eq!(
        scenarios::run_all().await,
        0,
        "protocol-probe --self-test 应全绿"
    );
}
