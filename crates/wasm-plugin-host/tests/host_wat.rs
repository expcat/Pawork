//! Phase 10 端到端集成测试：用 inline component WAT 覆盖
//! 加载/invoke、卸载、trap、fuel、内存、超时、篡改签名、状态、工具/命令注册。
//!
//! 组件 ABI 固定为顶层 `invoke(string) -> string`（JSON）。
//! 所有组件都是最小可运行的 Component Model WAT，由测试现场拼装。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use agent_domain::{CancellationToken, PluginId, WorkspaceId};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use plugin_api::{
    Plugin, PluginCapability, PluginCommandInvocation, PluginCommandRegistration, PluginContext,
    PluginErrorKind, PluginInvocationOutput, PluginLifecycleEvent, PluginLifecycleEventKind,
    PluginManifest, PluginOperation, PluginSignature, PluginSignatureAlgorithm,
    PluginStateMutation, PluginStateScope, PluginStateSnapshot, PluginToolRegistration,
    SignedPluginManifest,
};
use semver::{Version, VersionReq};
use tokio::time::{timeout, Duration};
use wasm_plugin_host::{
    external_tool_name, HostConfig, InMemoryPluginStateStore, NamespacedToolRegistry,
    PluginRuntime, PluginStateError, PluginStateStore, WasmPluginHost,
};

use agent_domain::CoreInstanceId;

// ---- WAT 生成器 ------------------------------------------------------------

/// 把任意字节转义为 WAT 字符串字面量内容（用于 data 段）。
fn wat_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\{:02x}", b)),
        }
    }
    out
}

/// realloc：对齐感知的 bump allocator（忽略 old_ptr，不做释放/缩容）。
/// canonical ABI 调用签名为 (old_ptr, old_size, align, new_size) -> new_ptr。
const REALLOC_WAT: &str = r#"
    (func $realloc (export "realloc") (param $old i32) (param $osize i32) (param $align i32) (param $newsize i32) (result i32)
      (local $ptr i32)
      (local.set $ptr (global.get $break))
      ;; round ptr up to align（align 为 2 的幂，canonical ABI 保证）
      (local.set $ptr
        (i32.and
          (i32.add (local.get $ptr) (i32.sub (local.get $align) (i32.const 1)))
          (i32.xor (i32.sub (local.get $align) (i32.const 1)) (i32.const -1))))
      (global.set $break (i32.add (local.get $ptr) (local.get $newsize)))
      (local.get $ptr))"#;

/// canon lift 选项（非 legacy 语法，需要 `core` 前缀）。
/// invoke 组件统一为 `invoke(string) -> string`。
const CANON_OPTS: &str = r#"
  (type $t (func (param "x" string) (result string)))
  (core instance $i (instantiate $m))
  (func (export "invoke") (type $t)
    (canon lift (core func $i "invoke")
      (memory (core memory $i "memory"))
      (realloc (core func $i "realloc"))))"#;

/// 回显组件：`invoke(ptr,len) -> tuple_ptr`，把输入字符串复制到 bump 区，
/// 再分配一个 8 字节元组 {str_ptr,str_len} 并返回其指针（间接返回约定）。
fn echo_component() -> String {
    format!(
        r#"(component
  (core module $m
    (memory (export "memory") 1)
    (global $break (mut i32) (i32.const 0))
    {REALLOC_WAT}
    (func (export "invoke") (param $ptr i32) (param $len i32) (result i32)
      (local $sptr i32) (local $tptr i32)
      (local.set $sptr (call $realloc (i32.const 0) (i32.const 0) (i32.const 1) (local.get $len)))
      (memory.copy (local.get $sptr) (local.get $ptr) (local.get $len))
      (local.set $tptr (call $realloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 8)))
      (i32.store (local.get $tptr) (local.get $sptr))
      (i32.store offset=4 (local.get $tptr) (local.get $len))
      (local.get $tptr))
  )
  {CANON_OPTS}
)"#
    )
}

/// 固定输出组件：`invoke` 忽略输入，返回 data 段中预置的字符串。
/// 用于让 `invoke_operation` 拿到一个合法的 `PluginInvocationOutput` JSON。
fn fixed_response_component(response: &str) -> String {
    let len = response.len();
    let escaped = wat_escape(response);
    format!(
        r#"(component
  (core module $m
    (memory (export "memory") 1)
    (global $break (mut i32) (i32.const {len}))
    (data (i32.const 0) "{escaped}")
    {REALLOC_WAT}
    (func (export "invoke") (param i32 i32) (result i32)
      (local $tptr i32)
      (local.set $tptr (call $realloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 8)))
      (i32.store (local.get $tptr) (i32.const 0))
      (i32.store offset=4 (local.get $tptr) (i32.const {len}))
      (local.get $tptr))
  )
  {CANON_OPTS}
)"#
    )
}

/// trap 组件：`invoke` 立即 `unreachable`。
fn trap_component() -> String {
    format!(
        r#"(component
  (core module $m
    (memory (export "memory") 1)
    (global $break (mut i32) (i32.const 0))
    {REALLOC_WAT}
    (func (export "invoke") (param i32 i32) (result i32)
      (unreachable))
  )
  {CANON_OPTS}
)"#
    )
}

/// 死循环组件：`invoke` 永不返回（用于 fuel / 超时 / 取消）。
fn loop_component() -> String {
    format!(
        r#"(component
  (core module $m
    (memory (export "memory") 1)
    (global $break (mut i32) (i32.const 0))
    {REALLOC_WAT}
    (func (export "invoke") (param i32 i32) (result i32)
      (loop $l (br $l))
      (i32.const 0))
  )
  {CANON_OPTS}
)"#
    )
}

/// 内存增长组件：`invoke` 反复 `memory.grow`，撑爆 `StoreLimits`。
fn memory_grow_component() -> String {
    format!(
        r#"(component
  (core module $m
    (memory (export "memory") 1)
    (global $break (mut i32) (i32.const 0))
    {REALLOC_WAT}
    (func $grow-loop (export "grow-loop") (result i32)
      (local $i i32)
      (local.set $i (i32.const 0))
      (block $exit
        (loop $l
          (br_if $exit (i32.eqz (i32.lt_s (local.get $i) (i32.const 1024))))
          (drop (memory.grow (i32.const 1)))
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $l)))
      (local.get $i))
    (func (export "invoke") (param i32 i32) (result i32)
      (drop (call $grow-loop))
      (i32.const 0))
  )
  {CANON_OPTS}
)"#
    )
}

// ---- 测试夹具 ---------------------------------------------------------------
/// 未知 import 组件：声明一个 host import `hostfn`，但 Linker 不注入任何 import。
/// 实例化时必然失败（P10-5：默认无文件/网络/进程，host 零 import）。
fn unknown_import_component() -> String {
    r#"(component
  (import "hostfn" (func $hostfn (param "x" s32)))
  (core module $m
    (import "hostfn" "f" (func $f (param i32)))
    (memory (export "memory") 1)
    (func $realloc (export "realloc") (param i32 i32 i32 i32) (result i32) (i32.const 0))
    (func (export "invoke") (param i32 i32) (result i32)
      (call $f (i32.const 0))
      (i32.const 0))
  )
  (core func $corehost (canon lower (func $hostfn)))
  (core instance $ci (instantiate $m (with "hostfn" (instance (export "f" (func $corehost))))))
  (type $t (func (param "x" string) (result string)))
  (func (export "invoke") (type $t)
    (canon lift (core func $ci "invoke")
      (memory (core memory $ci "memory"))
      (realloc (core func $ci "realloc"))))
)"#
    .to_string()
}
fn test_signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn sign(manifest: &PluginManifest, component: &[u8]) -> SignedPluginManifest {
    let signing = test_signing_key();
    let payload = manifest
        .canonical_signing_payload(component)
        .expect("payload");
    let sig = signing.sign(&payload);
    SignedPluginManifest {
        manifest: manifest.clone(),
        signature: PluginSignature {
            algorithm: PluginSignatureAlgorithm::Ed25519,
            key_id: "test".into(),
            signature: base64::engine::general_purpose::STANDARD.encode(sig.to_bytes()),
        },
    }
}

fn make_host(config: HostConfig) -> WasmPluginHost {
    let verifying = test_signing_key().verifying_key();
    let mut trust = wasm_plugin_host::TrustStore::new();
    trust.install_verifying_key("test", verifying);
    WasmPluginHost::new(config, trust, Arc::new(InMemoryPluginStateStore::new())).expect("host")
}

#[derive(Default)]
struct CountingStateStore {
    snapshot_calls: AtomicUsize,
}

impl PluginStateStore for CountingStateStore {
    fn snapshot(
        &self,
        _plugin: &PluginId,
        _scope: &PluginStateScope,
    ) -> Result<PluginStateSnapshot, PluginStateError> {
        self.snapshot_calls.fetch_add(1, Ordering::AcqRel);
        Ok(PluginStateSnapshot {
            revision: 7,
            values: [("private".to_string(), serde_json::json!("must-not-leak"))]
                .into_iter()
                .collect(),
        })
    }

    fn apply(
        &self,
        _plugin: &PluginId,
        _scope: &PluginStateScope,
        _mutations: &[PluginStateMutation],
        _expected_revision: u64,
        _config: &HostConfig,
    ) -> Result<u64, PluginStateError> {
        unreachable!("state apply must not run without PersistentState")
    }
}

struct BlockingApplyStateStore {
    inner: InMemoryPluginStateStore,
    apply_entered: Barrier,
    release_apply: Barrier,
}

impl BlockingApplyStateStore {
    fn new() -> Self {
        Self {
            inner: InMemoryPluginStateStore::new(),
            apply_entered: Barrier::new(2),
            release_apply: Barrier::new(2),
        }
    }
}

impl PluginStateStore for BlockingApplyStateStore {
    fn snapshot(
        &self,
        plugin: &PluginId,
        scope: &PluginStateScope,
    ) -> Result<PluginStateSnapshot, PluginStateError> {
        self.inner.snapshot(plugin, scope)
    }

    fn apply(
        &self,
        plugin: &PluginId,
        scope: &PluginStateScope,
        mutations: &[PluginStateMutation],
        expected_revision: u64,
        config: &HostConfig,
    ) -> Result<u64, PluginStateError> {
        self.apply_entered.wait();
        self.release_apply.wait();
        self.inner
            .apply(plugin, scope, mutations, expected_revision, config)
    }
}

fn manifest(id: &str, capabilities: Vec<PluginCapability>) -> PluginManifest {
    PluginManifest {
        id: PluginId::from(id),
        name: format!("Test {id}"),
        version: Version::new(0, 1, 0),
        api_version: VersionReq::parse(">=1, <2").unwrap(),
        description: None,
        permissions: Default::default(),
        capabilities,
        tool_capabilities: vec![],
        tools: vec![],
        commands: vec![],
        lifecycle_hooks: vec![],
    }
}

// ---- 加载 / invoke / 卸载 -------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_invoke_echo_round_trip() {
    let host = make_host(HostConfig::permissive());
    let component = echo_component();
    let bytes = component.into_bytes();
    let signed = sign(&manifest("echo.plugin", vec![]), &bytes);

    host.load(&signed, &bytes).await.expect("load");
    assert!(host.is_loaded(&PluginId::from("echo.plugin")));

    let out = host
        .invoke_raw(
            &PluginId::from("echo.plugin"),
            "hello-pawork".to_string(),
            CancellationToken::new(),
        )
        .await
        .expect("invoke");
    assert_eq!(out, "hello-pawork");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unload_makes_plugin_unavailable() {
    let host = make_host(HostConfig::permissive());
    let bytes = echo_component().into_bytes();
    let signed = sign(&manifest("echo.plugin", vec![]), &bytes);
    host.load(&signed, &bytes).await.expect("load");

    host.unload(&PluginId::from("echo.plugin"))
        .await
        .expect("unload");
    assert!(!host.is_loaded(&PluginId::from("echo.plugin")));

    let err = host
        .invoke_raw(
            &PluginId::from("echo.plugin"),
            "x".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::NotLoaded);

    // 二次卸载报 NotLoaded。
    let err = host
        .unload(&PluginId::from("echo.plugin"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::NotLoaded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unload_invalidates_retained_plugin_handles() {
    let host = make_host(HostConfig::permissive());
    let response = r#"{"status":"success","data":{"result":{}}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let mut mf = manifest("handle.plugin", vec![PluginCapability::LifecycleHook]);
    mf.lifecycle_hooks.push(PluginLifecycleEventKind::Start);
    let signed = sign(&mf, &bytes);
    let loaded = host.load(&signed, &bytes).await.expect("load");

    host.unload(&mf.id).await.expect("unload");
    assert!(!loaded.is_active());
    let error = loaded
        .on_lifecycle_event(
            PluginLifecycleEvent::Start,
            PluginContext {
                instance_id: CoreInstanceId::from("core"),
                workspace_id: None,
                session_id: None,
                run_id: None,
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("retained handle must be inactive after unload");
    assert_eq!(error.kind, PluginErrorKind::NotLoaded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unload_waits_for_state_apply_and_blocks_same_id_reload() {
    let state = Arc::new(BlockingApplyStateStore::new());
    let mut trust = wasm_plugin_host::TrustStore::new();
    trust.install_verifying_key("test", test_signing_key().verifying_key());
    let host = Arc::new(
        WasmPluginHost::new(HostConfig::permissive(), trust, state.clone()).expect("host"),
    );

    let response = r#"{"status":"success","data":{"result":{},"state_mutations":[{"type":"set","key":"value","value":1}]}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let mut mf = manifest(
        "unload-state.plugin",
        vec![
            PluginCapability::RegisterCommand,
            PluginCapability::PersistentState,
        ],
    );
    mf.commands.push(PluginCommandRegistration {
        name: "write".into(),
        description: "write".into(),
        input_schema: serde_json::json!({"type": "object"}),
    });
    let signed = sign(&mf, &bytes);
    host.load(&signed, &bytes).await.expect("initial load");

    let operation = PluginOperation::Command(PluginCommandInvocation {
        name: "write".into(),
        input: serde_json::Value::Null,
        context: PluginContext {
            instance_id: CoreInstanceId::from("core"),
            workspace_id: Some(WorkspaceId::from("workspace")),
            session_id: None,
            run_id: None,
        },
    });
    let invoking_host = host.clone();
    let invoking_id = mf.id.clone();
    let invocation = tokio::spawn(async move {
        invoking_host
            .invoke_operation(&invoking_id, operation, CancellationToken::new())
            .await
    });

    // `apply` 已进入，但尚未提交；此时 unload 必须等待完整 operation 事务。
    state.apply_entered.wait();
    let unloading_host = host.clone();
    let unloading_id = mf.id.clone();
    let unload = tokio::spawn(async move { unloading_host.unload(&unloading_id).await });
    timeout(Duration::from_secs(1), async {
        while host.is_loaded(&mf.id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("unload must remove the old registration");
    assert!(!unload.is_finished(), "unload returned before state apply");

    // unload 持有 load 锁；同 id 重载不得越过尚未完成的旧实例事务。
    let reloading_host = host.clone();
    let reload_signed = signed.clone();
    let reload_bytes = bytes.clone();
    let reload =
        tokio::spawn(async move { reloading_host.load(&reload_signed, &reload_bytes).await });
    tokio::task::yield_now().await;
    assert!(
        !reload.is_finished(),
        "same-id reload overtook the unloading instance"
    );

    state.release_apply.wait();
    invocation.await.expect("invocation task").expect("invoke");
    unload.await.expect("unload task").expect("unload");
    reload.await.expect("reload task").expect("reload");
    assert!(host.is_loaded(&mf.id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_load_is_conflict() {
    let host = make_host(HostConfig::permissive());
    let bytes = echo_component().into_bytes();
    let signed = sign(&manifest("echo.plugin", vec![]), &bytes);
    host.load(&signed, &bytes).await.expect("first load");
    let err = host.load(&signed, &bytes).await.unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::Conflict);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn component_input_and_output_byte_limits_are_enforced() {
    let bytes = echo_component().into_bytes();

    let mut component_config = HostConfig::permissive();
    component_config.max_component_bytes = 1;
    let component_host = make_host(component_config);
    let signed = sign(&manifest("component-limit.plugin", vec![]), &bytes);
    let component_error = component_host.load(&signed, &bytes).await.unwrap_err();
    assert_eq!(component_error.kind, PluginErrorKind::InvalidManifest);

    let mut input_config = HostConfig::permissive();
    input_config.max_input_bytes = 4;
    let input_host = make_host(input_config);
    let input_signed = sign(&manifest("input-limit.plugin", vec![]), &bytes);
    input_host
        .load(&input_signed, &bytes)
        .await
        .expect("load input fixture");
    let input_error = input_host
        .invoke_raw(
            &PluginId::from("input-limit.plugin"),
            "12345".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(input_error.kind, PluginErrorKind::InvalidInvocation);

    let mut output_config = HostConfig::permissive();
    output_config.max_output_bytes = 4;
    let output_host = make_host(output_config);
    let output_signed = sign(&manifest("output-limit.plugin", vec![]), &bytes);
    output_host
        .load(&output_signed, &bytes)
        .await
        .expect("load output fixture");
    let output_error = output_host
        .invoke_raw(
            &PluginId::from("output-limit.plugin"),
            "12345".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(output_error.kind, PluginErrorKind::InvalidInvocation);
}

// ---- trap 隔离 -------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trap_is_isolated_and_does_not_crash_host() {
    let host = make_host(HostConfig::permissive());

    // 先加载一个正常插件，确认 trap 插件崩溃不影响它。
    let echo_bytes = echo_component().into_bytes();
    let echo_signed = sign(&manifest("good.plugin", vec![]), &echo_bytes);
    host.load(&echo_signed, &echo_bytes)
        .await
        .expect("load good");

    let trap_bytes = trap_component().into_bytes();
    let trap_signed = sign(&manifest("bad.plugin", vec![]), &trap_bytes);
    host.load(&trap_signed, &trap_bytes)
        .await
        .expect("load bad");

    let err = host
        .invoke_raw(
            &PluginId::from("bad.plugin"),
            "x".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.kind,
        PluginErrorKind::Trap,
        "trap should surface as Trap"
    );

    // good 插件仍可正常调用——崩溃隔离成立。
    let out = host
        .invoke_raw(
            &PluginId::from("good.plugin"),
            "still-here".into(),
            CancellationToken::new(),
        )
        .await
        .expect("good plugin still works");
    assert_eq!(out, "still-here");
}

// ---- fuel / 内存 / 超时 / 取消 --------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fuel_exhaustion_is_reported() {
    let mut config = HostConfig::permissive();
    config.fuel = 1_000; // 极低预算，死循环必然耗尽
    config.invoke_timeout = Duration::from_secs(30);
    let host = make_host(config);
    let bytes = loop_component().into_bytes();
    let signed = sign(&manifest("loop.plugin", vec![]), &bytes);
    host.load(&signed, &bytes).await.expect("load");

    let err = host
        .invoke_raw(
            &PluginId::from("loop.plugin"),
            "x".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::FuelExhausted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_growth_is_rejected() {
    let mut config = HostConfig::permissive();
    config.max_memory_bytes = 64 * 1024; // 仅允许 1 页；grow 立即越限
    let host = make_host(config);
    let bytes = memory_grow_component().into_bytes();
    let signed = sign(&manifest("mem.plugin", vec![]), &bytes);
    host.load(&signed, &bytes).await.expect("load");

    let err = host
        .invoke_raw(
            &PluginId::from("mem.plugin"),
            "x".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::MemoryLimit);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invoke_timeout_aborts_loop() {
    let mut config = HostConfig::permissive();
    config.fuel = u64::MAX / 4; // 充足 fuel，确保是 wall-clock 超时胜出
    config.invoke_timeout = Duration::from_millis(150);
    config.epoch_tick = Duration::from_millis(5);
    let host = make_host(config);
    let bytes = loop_component().into_bytes();
    let signed = sign(&manifest("loop.plugin", vec![]), &bytes);
    host.load(&signed, &bytes).await.expect("load");

    let err = host
        .invoke_raw(
            &PluginId::from("loop.plugin"),
            "x".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::Timeout);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_aborts_loop() {
    let mut config = HostConfig::permissive();
    config.fuel = u64::MAX / 4;
    config.invoke_timeout = Duration::from_secs(30);
    config.epoch_tick = Duration::from_millis(5);
    let host = make_host(config);
    let bytes = loop_component().into_bytes();
    let signed = sign(&manifest("loop.plugin", vec![]), &bytes);
    host.load(&signed, &bytes).await.expect("load");

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    // 短延迟后取消，让 wasm 先进入循环并被 epoch yield。
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(60)).await;
        cancel_clone.cancel();
    });

    let err = host
        .invoke_raw(&PluginId::from("loop.plugin"), "x".into(), cancel)
        .await
        .unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::Cancelled);
}

// ---- 签名 / API 版本 ------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tampered_component_bytes_rejected() {
    let host = make_host(HostConfig::permissive());
    let signed_a = sign(
        &manifest("echo.plugin", vec![]),
        echo_component().as_bytes(),
    );
    // 用错误的组件字节去加载（与签名时的摘要不一致）。
    let tampered = fixed_response_component("\"replaced\"").into_bytes();
    let err = host.load(&signed_a, &tampered).await.unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::SignatureRejected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tampered_manifest_is_rejected() {
    let host = make_host(HostConfig::permissive());
    let bytes = echo_component().into_bytes();
    let mut signed = sign(&manifest("echo.plugin", vec![]), &bytes);
    signed.manifest.name = "Tampered Name".into();

    let error = host.load(&signed, &bytes).await.unwrap_err();
    assert_eq!(error.kind, PluginErrorKind::SignatureRejected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_key_id_rejected() {
    let host = make_host(HostConfig::permissive());
    let mut signed = sign(
        &manifest("echo.plugin", vec![]),
        echo_component().as_bytes(),
    );
    signed.signature.key_id = "unknown".into();
    let bytes = echo_component().into_bytes();
    let err = host.load(&signed, &bytes).await.unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::SignatureRejected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_signature_is_rejected_as_signature_error() {
    let host = make_host(HostConfig::permissive());
    let bytes = echo_component().into_bytes();
    let mut signed = sign(&manifest("echo.plugin", vec![]), &bytes);
    signed.signature.signature = "not-base64%%%".into();

    let error = host.load(&signed, &bytes).await.unwrap_err();
    assert_eq!(error.kind, PluginErrorKind::SignatureRejected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incompatible_api_version_rejected() {
    let host = make_host(HostConfig::permissive());
    let mut manifest = manifest("echo.plugin", vec![]);
    manifest.api_version = VersionReq::parse(">=99, <100").unwrap();
    let bytes = echo_component().into_bytes();
    let signed = sign(&manifest, &bytes);
    let err = host.load(&signed, &bytes).await.unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::IncompatibleApi);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn component_without_invoke_export_rejected() {
    let host = make_host(HostConfig::permissive());
    // 没有 invoke 导出的组件。
    let wat = r#"(component
  (core module $m (memory (export "memory") 1))
  (core instance $i (instantiate $m))
)"#;
    let manifest = manifest("noinvoke.plugin", vec![]);
    let bytes = wat.as_bytes().to_vec();
    let signed = sign(&manifest, &bytes);
    let err = host.load(&signed, &bytes).await.unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::InvalidManifest);
}

// ---- 状态（invoke_operation + 状态变更） ---------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_mutations_persist_across_invocations() {
    let host = make_host(HostConfig::permissive());
    // 组件固定返回一个带状态变更的成功响应。
    let response = r#"{"status":"success","data":{"result":{"ok":true},"state_mutations":[{"type":"set","key":"counter","value":{"n":1}}]}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let pid = PluginId::from("state.plugin");
    let mut mf = manifest(
        "state.plugin",
        vec![
            PluginCapability::RegisterCommand,
            PluginCapability::PersistentState,
        ],
    );
    mf.commands.push(PluginCommandRegistration {
        name: "bump".into(),
        description: "bump".into(),
        input_schema: serde_json::json!({"type": "object"}),
    });
    let signed = sign(&mf, &bytes);
    host.load(&signed, &bytes).await.expect("load");

    let workspace = WorkspaceId::from("ws");
    let context = PluginContext {
        instance_id: CoreInstanceId::from("core"),
        workspace_id: Some(workspace.clone()),
        session_id: None,
        run_id: None,
    };
    let op = PluginOperation::Command(PluginCommandInvocation {
        name: "bump".into(),
        input: serde_json::json!({}),
        context,
    });

    let out = host
        .invoke_operation(&pid, op.clone(), CancellationToken::new())
        .await
        .expect("invoke");
    assert!(matches!(out, PluginInvocationOutput::Success(_)));

    // 第二次调用应因 revision 不匹配（快照基于旧 revision 写入）失败？
    // 实际上每次 invoke_operation 都重新 snapshot 最新 revision，所以第二次也会成功，
    // 并且状态键被再次写入（幂等 set）。
    let out2 = host
        .invoke_operation(&pid, op, CancellationToken::new())
        .await
        .expect("invoke 2");
    assert!(matches!(out2, PluginInvocationOutput::Success(_)));

    // 验证状态确实落地。
    let snap = host
        .state_store()
        .snapshot(&pid, &plugin_api::PluginStateScope::Workspace(workspace))
        .expect("snapshot");
    assert_eq!(snap.revision, 2);
    assert_eq!(snap.values["counter"]["n"], serde_json::json!(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_stateful_invocations_serialize_snapshot_and_apply() {
    let host = make_host(HostConfig::permissive());
    let response = r#"{"status":"success","data":{"result":{},"state_mutations":[{"type":"set","key":"value","value":1}]}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let mut mf = manifest(
        "state-race.plugin",
        vec![
            PluginCapability::RegisterCommand,
            PluginCapability::PersistentState,
        ],
    );
    mf.commands.push(PluginCommandRegistration {
        name: "write".into(),
        description: "write".into(),
        input_schema: serde_json::json!({"type": "object"}),
    });
    let signed = sign(&mf, &bytes);
    host.load(&signed, &bytes).await.expect("load");
    let operation = PluginOperation::Command(PluginCommandInvocation {
        name: "write".into(),
        input: serde_json::Value::Null,
        context: PluginContext {
            instance_id: CoreInstanceId::from("core"),
            workspace_id: Some(WorkspaceId::from("ws")),
            session_id: None,
            run_id: None,
        },
    });

    let (first, second) = tokio::join!(
        host.invoke_operation(&mf.id, operation.clone(), CancellationToken::new()),
        host.invoke_operation(&mf.id, operation, CancellationToken::new()),
    );
    first.expect("first stateful call");
    second.expect("second stateful call");

    let snapshot = host
        .state_store()
        .snapshot(
            &mf.id,
            &PluginStateScope::Workspace(WorkspaceId::from("ws")),
        )
        .expect("snapshot");
    assert_eq!(snapshot.revision, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_error_output_is_decoded() {
    let host = make_host(HostConfig::permissive());
    let response = r#"{"status":"error","data":{"error":{"kind":"state","message":"nope","retryable":false}}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let pid = PluginId::from("err.plugin");
    let mut mf = manifest("err.plugin", vec![PluginCapability::LifecycleHook]);
    mf.lifecycle_hooks.push(PluginLifecycleEventKind::Start);
    let signed = sign(&mf, &bytes);
    host.load(&signed, &bytes).await.expect("load");

    let context = PluginContext {
        instance_id: CoreInstanceId::from("core"),
        workspace_id: Some(WorkspaceId::from("ws")),
        session_id: None,
        run_id: None,
    };
    let op = PluginOperation::Lifecycle {
        event: PluginLifecycleEvent::Start,
        context,
    };
    let out = host
        .invoke_operation(&pid, op, CancellationToken::new())
        .await
        .expect("invoke");
    match out {
        PluginInvocationOutput::Error { error } => {
            assert_eq!(error.kind, PluginErrorKind::State);
            assert_eq!(error.message, "nope");
        }
        other => panic!("expected error output, got {other:?}"),
    }

    let loaded = host.get(&pid).expect("loaded");
    let lifecycle_error = loaded
        .on_lifecycle_event(
            PluginLifecycleEvent::Start,
            PluginContext {
                instance_id: CoreInstanceId::from("core"),
                workspace_id: Some(WorkspaceId::from("ws")),
                session_id: None,
                run_id: None,
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("component error output must reach hook-runtime");
    assert_eq!(lifecycle_error.kind, PluginErrorKind::State);
    assert_eq!(lifecycle_error.message, "nope");
}

// ---- 工具 / 命令注册（namespace + 不覆盖同名） ---------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_registry_namespaces_external_plugins() {
    // 与 host 真实 invoke 集成：注册一个转发到 state.plugin 的工具，调度后回放状态。
    let host = make_host(HostConfig::permissive());
    let response = r#"{"status":"success","data":{"result":{"echoed":true}}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let pid = PluginId::from("tool.plugin");
    let mut manifest = manifest("tool.plugin", vec![PluginCapability::RegisterTool]);
    manifest.tools.push(PluginToolRegistration {
        name: "ping".into(),
        description: "ping".into(),
        input_schema: serde_json::json!({"type":"object"}),
        default_timeout_ms: None,
        max_output_bytes: 4096,
    });
    let signed = sign(&manifest, &bytes);
    host.load(&signed, &bytes).await.expect("load");

    // 直接验证命名规则与 ExternalPlugin 标签（不在此处接入完整 tool-runtime 调度）。
    assert_eq!(external_tool_name(&pid, "ping"), "tool.plugin::ping");

    struct NullCaller;
    #[async_trait::async_trait]
    impl wasm_plugin_host::registry::ExternalToolCaller for NullCaller {
        async fn call(
            &self,
            _pid: &PluginId,
            _name: &str,
            _req: tool_api::ToolRequest,
            _ctx: tool_api::ToolExecutionContext,
            _cancel: CancellationToken,
        ) -> Result<tool_api::ToolResult, plugin_api::PluginError> {
            unreachable!("not invoked in this test")
        }
    }

    let mut registry = NamespacedToolRegistry::new();
    let caller: Arc<dyn wasm_plugin_host::registry::ExternalToolCaller> = Arc::new(NullCaller);
    registry
        .register_external(&pid, &manifest.tools, &|p, r| {
            Arc::new(wasm_plugin_host::registry::ExternalPluginToolAdapter::new(
                p,
                r,
                caller.clone(),
            ))
        })
        .expect("register");

    let desc = &registry.descriptors()[0];
    assert_eq!(desc.name, "tool.plugin::ping");
    assert_eq!(desc.capability, tool_api::ToolCapability::ExternalPlugin);

    let tr = registry.to_tool_registry().expect("registry snapshot");
    assert_eq!(tr.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_registry_namespaces_commands() {
    let host = Arc::new(make_host(HostConfig::permissive()));
    let response = r#"{"status":"success","data":{"result":{"ran":true}}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let pid = PluginId::from("cmd.plugin");
    let mut manifest = manifest("cmd.plugin", vec![PluginCapability::RegisterCommand]);
    manifest.commands.push(PluginCommandRegistration {
        name: "run".into(),
        description: "run".into(),
        input_schema: serde_json::json!({"type": "object"}),
    });
    let signed = sign(&manifest, &bytes);
    host.load(&signed, &bytes).await.expect("load");

    let mut commands = wasm_plugin_host::PluginCommandRegistry::new();
    commands
        .register(&pid, &manifest.commands)
        .expect("register");
    assert_eq!(commands.owner("cmd.plugin::run"), Some(&pid));
    assert_eq!(commands.names().len(), 1);

    let result = commands
        .invoke(
            "cmd.plugin::run",
            serde_json::json!({"arg": 1}),
            PluginContext {
                instance_id: CoreInstanceId::from("core"),
                workspace_id: Some(WorkspaceId::from("ws")),
                session_id: None,
                run_id: None,
            },
            host.as_ref(),
            CancellationToken::new(),
        )
        .await
        .expect("invoke registered command");
    assert_eq!(result["ran"], true);

    let missing = commands
        .invoke(
            "cmd.plugin::missing",
            serde_json::Value::Null,
            PluginContext {
                instance_id: CoreInstanceId::from("core"),
                workspace_id: None,
                session_id: None,
                run_id: None,
            },
            host.as_ref(),
            CancellationToken::new(),
        )
        .await
        .expect_err("unknown command must fail closed");
    assert_eq!(missing.kind, PluginErrorKind::InvalidInvocation);
}

// ---- 缺口补齐：未知 import / capability 闸门 / 跨调用 / 真实 tool+command+lifecycle / 并发 load ---

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_import_is_rejected_at_instantiation() {
    // 默认 Linker 不注入任何 import；组件声明 env::log 实例化必然失败。
    let host = make_host(HostConfig::permissive());
    let bytes = unknown_import_component().into_bytes();
    let signed = sign(&manifest("import.plugin", vec![]), &bytes);
    let err = host.load(&signed, &bytes).await.unwrap_err();
    // 实例化失败统一映射为 trap/internal；关键是 host 不崩溃、不加载。
    assert!(
        !host.is_loaded(&PluginId::from("import.plugin")),
        "失败插件不应被登记"
    );
    // 错误信息应体现 unknown import / no item found 之类。
    let msg = err.message.to_lowercase();
    assert!(
        msg.contains("not found in the linker")
            || msg.contains("unknown")
            || msg.contains("no item")
            || msg.contains("instantiate"),
        "unexpected error: {err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_mutation_without_persistent_state_is_denied() {
    let host = make_host(HostConfig::permissive());
    // 组件返回一个带状态变更的响应，但 manifest 未声明 PersistentState。
    let response = r#"{"status":"success","data":{"result":{},"state_mutations":[{"type":"set","key":"k","value":{"n":1}}]}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let pid = PluginId::from("nopersist.plugin");
    let mut mf = manifest("nopersist.plugin", vec![PluginCapability::RegisterCommand]);
    mf.commands.push(PluginCommandRegistration {
        name: "n".into(),
        description: "n".into(),
        input_schema: serde_json::json!({"type": "object"}),
    });
    let signed = sign(&mf, &bytes);
    host.load(&signed, &bytes).await.expect("load");

    let context = PluginContext {
        instance_id: CoreInstanceId::from("core"),
        workspace_id: Some(WorkspaceId::from("ws")),
        session_id: None,
        run_id: None,
    };
    let op = PluginOperation::Command(PluginCommandInvocation {
        name: "n".into(),
        input: serde_json::json!({}),
        context,
    });
    let err = host
        .invoke_operation(&pid, op, CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::PermissionDenied);

    // 确认状态未被写入。
    let snap = host
        .state_store()
        .snapshot(
            &pid,
            &plugin_api::PluginStateScope::Workspace(WorkspaceId::from("ws")),
        )
        .expect("snapshot");
    assert!(snap.values.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_snapshot_is_not_read_without_persistent_state() {
    let state = Arc::new(CountingStateStore::default());
    let mut trust = wasm_plugin_host::TrustStore::new();
    trust.install_verifying_key("test", test_signing_key().verifying_key());
    let host = WasmPluginHost::new(HostConfig::permissive(), trust, state.clone())
        .expect("host with counting state");
    let response = r#"{"status":"success","data":{"result":{}}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let mut mf = manifest(
        "nostateread.plugin",
        vec![PluginCapability::RegisterCommand],
    );
    mf.commands.push(PluginCommandRegistration {
        name: "run".into(),
        description: "run".into(),
        input_schema: serde_json::json!({"type": "object"}),
    });
    let signed = sign(&mf, &bytes);
    host.load(&signed, &bytes).await.expect("load");

    host.invoke_operation(
        &mf.id,
        PluginOperation::Command(PluginCommandInvocation {
            name: "run".into(),
            input: serde_json::Value::Null,
            context: PluginContext {
                instance_id: CoreInstanceId::from("core"),
                workspace_id: Some(WorkspaceId::from("ws")),
                session_id: None,
                run_id: None,
            },
        }),
        CancellationToken::new(),
    )
    .await
    .expect("invoke without persistent state");

    assert_eq!(state.snapshot_calls.load(Ordering::Acquire), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undeclared_operations_are_rejected_even_with_broad_capability() {
    let host = make_host(HostConfig::permissive());
    let response = r#"{"status":"success","data":{"result":{}}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let mut mf = manifest(
        "declared.plugin",
        vec![
            PluginCapability::RegisterTool,
            PluginCapability::RegisterCommand,
            PluginCapability::LifecycleHook,
        ],
    );
    mf.tools.push(PluginToolRegistration {
        name: "allowed-tool".into(),
        description: "allowed".into(),
        input_schema: serde_json::json!({"type": "object"}),
        default_timeout_ms: None,
        max_output_bytes: 1024,
    });
    mf.commands.push(PluginCommandRegistration {
        name: "allowed-command".into(),
        description: "allowed".into(),
        input_schema: serde_json::json!({"type": "object"}),
    });
    mf.lifecycle_hooks.push(PluginLifecycleEventKind::Start);
    let signed = sign(&mf, &bytes);
    host.load(&signed, &bytes).await.expect("load");

    let tool_error = host
        .invoke_operation(
            &mf.id,
            PluginOperation::Tool {
                name: "other-tool".into(),
                request: tool_api::ToolRequest {
                    tool_call_id: agent_domain::ToolCallId::from("call"),
                    input: serde_json::Value::Null,
                },
                context: tool_api::ToolExecutionContext {
                    workspace_id: WorkspaceId::from("ws"),
                    run_id: agent_domain::RunId::from("run"),
                    working_directory: None,
                },
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("undeclared tool must fail");
    assert_eq!(tool_error.kind, PluginErrorKind::PermissionDenied);

    let command_error = host
        .invoke_operation(
            &mf.id,
            PluginOperation::Command(PluginCommandInvocation {
                name: "other-command".into(),
                input: serde_json::Value::Null,
                context: PluginContext {
                    instance_id: CoreInstanceId::from("core"),
                    workspace_id: None,
                    session_id: None,
                    run_id: None,
                },
            }),
            CancellationToken::new(),
        )
        .await
        .expect_err("undeclared command must fail");
    assert_eq!(command_error.kind, PluginErrorKind::PermissionDenied);

    let lifecycle_error = host
        .invoke_operation(
            &mf.id,
            PluginOperation::Lifecycle {
                event: PluginLifecycleEvent::Stop,
                context: PluginContext {
                    instance_id: CoreInstanceId::from("core"),
                    workspace_id: None,
                    session_id: None,
                    run_id: None,
                },
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("undeclared lifecycle event must fail");
    assert_eq!(lifecycle_error.kind, PluginErrorKind::PermissionDenied);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_isolation_across_plugins_and_scopes() {
    // 两个不同插件各自写状态，确认互不可见、scope 隔离。
    let host = make_host(HostConfig::permissive());

    async fn write_once(
        host: &WasmPluginHost,
        plugin_id: &str,
        key: &str,
        value: serde_json::Value,
        workspace: &str,
    ) {
        let response = format!(
            r#"{{"status":"success","data":{{"result":{{}},"state_mutations":[{{"type":"set","key":"{key}","value":{value}}}]}}}}"#
        );
        let bytes = fixed_response_component(&response).into_bytes();
        let pid = PluginId::from(plugin_id);
        let mut mf = manifest(
            plugin_id,
            vec![
                PluginCapability::RegisterCommand,
                PluginCapability::PersistentState,
            ],
        );
        mf.commands.push(PluginCommandRegistration {
            name: "set".into(),
            description: "set".into(),
            input_schema: serde_json::json!({"type": "object"}),
        });
        let signed = sign(&mf, &bytes);
        host.load(&signed, &bytes).await.expect("load");
        let context = PluginContext {
            instance_id: CoreInstanceId::from("core"),
            workspace_id: Some(WorkspaceId::from(workspace)),
            session_id: None,
            run_id: None,
        };
        let op = PluginOperation::Command(PluginCommandInvocation {
            name: "set".into(),
            input: serde_json::json!({}),
            context,
        });
        host.invoke_operation(&pid, op, CancellationToken::new())
            .await
            .expect("invoke");
    }

    write_once(&host, "iso.a", "ka", serde_json::json!(1), "w1").await;
    write_once(&host, "iso.b", "kb", serde_json::json!(2), "w1").await;

    let pa = PluginId::from("iso.a");
    let pb = PluginId::from("iso.b");
    let w1 = plugin_api::PluginStateScope::Workspace(WorkspaceId::from("w1"));
    let snap_a = host.state_store().snapshot(&pa, &w1).unwrap();
    let snap_b = host.state_store().snapshot(&pb, &w1).unwrap();
    assert_eq!(snap_a.values["ka"], serde_json::json!(1));
    assert!(!snap_a.values.contains_key("kb"));
    assert_eq!(snap_b.values["kb"], serde_json::json!(2));
    assert!(!snap_b.values.contains_key("ka"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_call_routes_through_real_host_invoke() {
    // 端到端：ExternalPluginToolAdapter 通过 WasmPluginHost（实现 ExternalToolCaller）
    // 调用真实组件，确认结果被正确解码为 ToolResult。
    let host = std::sync::Arc::new(make_host(HostConfig::permissive()));
    let response = r#"{"status":"success","data":{"result":{"echoed":"pong"}}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let pid = PluginId::from("toolreal.plugin");
    let mut mf = manifest(
        "toolreal.plugin",
        vec![
            PluginCapability::RegisterTool,
            PluginCapability::PersistentState,
        ],
    );
    mf.tools.push(PluginToolRegistration {
        name: "ping".into(),
        description: "ping".into(),
        input_schema: serde_json::json!({"type": "object"}),
        default_timeout_ms: None,
        max_output_bytes: 4096,
    });
    let signed = sign(&mf, &bytes);
    host.load(&signed, &bytes).await.expect("load");

    // 用真实 host 作为 caller 注册工具。
    let caller: std::sync::Arc<dyn wasm_plugin_host::ExternalToolCaller> = host.clone();
    let mut registry = NamespacedToolRegistry::new();
    registry
        .register_external(&pid, &mf.tools, &|p, r| {
            std::sync::Arc::new(wasm_plugin_host::ExternalPluginToolAdapter::new(
                p,
                r,
                caller.clone(),
            ))
        })
        .expect("register");

    let tool = registry.get("toolreal.plugin::ping").expect("tool");
    let result = tool
        .execute(
            tool_api::ToolRequest {
                tool_call_id: agent_domain::ToolCallId::from("call"),
                input: serde_json::json!({}),
            },
            tool_api::ToolExecutionContext {
                workspace_id: WorkspaceId::from("ws"),
                run_id: agent_domain::RunId::from("run"),
                working_directory: None,
            },
            &tool_runtime::NoopToolEventSink,
            CancellationToken::new(),
        )
        .await
        .expect("execute");
    assert!(result.success);
    // tool_result_from_value 把非字符串 JSON 包装成单 Text 段（其文本为 JSON 序列化）。
    assert_eq!(result.content.len(), 1);

    let scheduler = tool_runtime::ToolScheduler::new(
        registry.to_tool_registry().expect("registry snapshot"),
        tool_runtime::ToolSchedulerConfig {
            max_concurrent: 2,
            approval_mode: tool_runtime::ApprovalMode::NeverAsk,
            workspace_trusted: true,
        },
    );
    let scheduled = scheduler
        .execute_named(
            "toolreal.plugin::ping",
            tool_api::ToolRequest {
                tool_call_id: agent_domain::ToolCallId::from("scheduled-call"),
                input: serde_json::json!({}),
            },
            tool_api::ToolExecutionContext {
                workspace_id: WorkspaceId::from("ws"),
                run_id: agent_domain::RunId::from("scheduled-run"),
                working_directory: None,
            },
            CancellationToken::new(),
            &tool_runtime::AutoApproveResolver,
            &tool_runtime::NoopToolEventSink,
        )
        .await
        .expect("scheduler executes plugin tool");
    assert!(scheduled.success);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_event_routes_through_loaded_plugin_trait() {
    // LoadedPlugin 实现 plugin_api::Plugin：派发 Start 事件经 invoke 落地。
    let host = make_host(HostConfig::permissive());
    let response = r#"{"status":"success","data":{"result":{"started":true}}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let pid = PluginId::from("lc.plugin");
    let mut mf = manifest("lc.plugin", vec![PluginCapability::LifecycleHook]);
    mf.lifecycle_hooks.push(PluginLifecycleEventKind::Start);
    let signed = sign(&mf, &bytes);
    host.load(&signed, &bytes).await.expect("load");

    let plugin = host.get(&pid).expect("loaded");
    let dyn_plugin: std::sync::Arc<dyn plugin_api::Plugin> = plugin.clone() as _;
    dyn_plugin
        .on_lifecycle_event(
            PluginLifecycleEvent::Start,
            PluginContext {
                instance_id: CoreInstanceId::from("core"),
                workspace_id: None,
                session_id: None,
                run_id: None,
            },
            CancellationToken::new(),
        )
        .await
        .expect("lifecycle dispatch ok");

    // 未声明的 hook 应 no-op 成功。
    dyn_plugin
        .on_lifecycle_event(
            PluginLifecycleEvent::Stop,
            PluginContext {
                instance_id: CoreInstanceId::from("core"),
                workspace_id: None,
                session_id: None,
                run_id: None,
            },
            CancellationToken::new(),
        )
        .await
        .expect("undeclared hook no-ops");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loaded_plugin_dispatches_through_hook_runtime() {
    let host = make_host(HostConfig::permissive());
    let response = r#"{"status":"success","data":{"result":{"started":true}}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let mut mf = manifest("hook-runtime.plugin", vec![PluginCapability::LifecycleHook]);
    mf.lifecycle_hooks.push(PluginLifecycleEventKind::Start);
    let signed = sign(&mf, &bytes);
    let loaded = host.load(&signed, &bytes).await.expect("load");

    let hooks = hook_runtime::HookRuntime::new();
    let plugin: Arc<dyn Plugin> = loaded;
    hooks
        .register(plugin)
        .await
        .expect("register loaded plugin");
    let report = hooks
        .start(PluginContext {
            instance_id: CoreInstanceId::from("core"),
            workspace_id: None,
            session_id: None,
            run_id: None,
        })
        .await
        .expect("dispatch start");

    assert_eq!(report.outcomes.len(), 1);
    assert_eq!(report.outcomes[0].plugin_id, mf.id);
    assert_eq!(
        report.outcomes[0].status,
        hook_runtime::PluginHookOutcomeStatus::Success
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_runtime_coordinates_load_register_dispatch_and_unload() {
    let runtime = PluginRuntime::new(make_host(HostConfig::permissive())).expect("plugin runtime");
    let response = r#"{"status":"success","data":{"result":{"ok":true}}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let mut mf = manifest(
        "runtime.plugin",
        vec![
            PluginCapability::RegisterTool,
            PluginCapability::RegisterCommand,
            PluginCapability::LifecycleHook,
        ],
    );
    mf.tools.push(PluginToolRegistration {
        name: "lookup".into(),
        description: "lookup".into(),
        input_schema: serde_json::json!({"type": "object"}),
        default_timeout_ms: None,
        max_output_bytes: 4096,
    });
    mf.commands.push(PluginCommandRegistration {
        name: "refresh".into(),
        description: "refresh".into(),
        input_schema: serde_json::json!({"type": "object"}),
    });
    mf.lifecycle_hooks.extend([
        PluginLifecycleEventKind::Start,
        PluginLifecycleEventKind::RunStart,
    ]);
    let signed = sign(&mf, &bytes);

    runtime.load(&signed, &bytes).await.expect("runtime load");
    assert_eq!(runtime.loaded_plugins(), [mf.id.clone()]);
    assert_eq!(
        runtime.command_names().await,
        ["runtime.plugin::refresh".to_string()]
    );
    let tool_snapshot = runtime.tool_registry().await.expect("registry snapshot");
    let retained_tool = tool_snapshot
        .get("runtime.plugin::lookup")
        .expect("registered tool");

    let context = PluginContext {
        instance_id: CoreInstanceId::from("core"),
        workspace_id: Some(WorkspaceId::from("workspace")),
        session_id: None,
        run_id: None,
    };
    let start = runtime.start(context.clone()).await.expect("start hooks");
    assert_eq!(start.outcomes.len(), 1);
    assert_eq!(start.outcomes[0].plugin_id, mf.id);
    let dispatched = runtime
        .dispatch(
            PluginLifecycleEvent::RunStart {
                run_id: agent_domain::RunId::from("run"),
            },
            context.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("dispatch hook");
    assert_eq!(dispatched.outcomes.len(), 1);

    let mutation_error = runtime
        .unload(&mf.id)
        .await
        .expect_err("registry mutation while started must fail");
    assert_eq!(mutation_error.kind, PluginErrorKind::Conflict);
    assert!(runtime.is_loaded(&mf.id));

    let command = runtime
        .invoke_command(
            "runtime.plugin::refresh",
            serde_json::json!({}),
            context.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("invoke registered command");
    assert_eq!(command, serde_json::json!({"ok": true}));

    let tool_result = retained_tool
        .execute(
            tool_api::ToolRequest {
                tool_call_id: agent_domain::ToolCallId::from("call"),
                input: serde_json::json!({}),
            },
            tool_api::ToolExecutionContext {
                workspace_id: WorkspaceId::from("workspace"),
                run_id: agent_domain::RunId::from("run"),
                working_directory: None,
            },
            &tool_runtime::NoopToolEventSink,
            CancellationToken::new(),
        )
        .await
        .expect("invoke registered tool");
    assert!(tool_result.success);

    runtime.stop(context).await.expect("stop hooks");
    runtime.unload(&mf.id).await.expect("runtime unload");
    assert!(runtime.loaded_plugins().is_empty());
    assert!(runtime.command_names().await.is_empty());
    assert!(runtime
        .tool_registry()
        .await
        .expect("registry snapshot")
        .is_empty());

    let stale_result = retained_tool
        .execute(
            tool_api::ToolRequest {
                tool_call_id: agent_domain::ToolCallId::from("stale-call"),
                input: serde_json::Value::Null,
            },
            tool_api::ToolExecutionContext {
                workspace_id: WorkspaceId::from("workspace"),
                run_id: agent_domain::RunId::from("stale-run"),
                working_directory: None,
            },
            &tool_runtime::NoopToolEventSink,
            CancellationToken::new(),
        )
        .await
        .expect("stale adapter returns canonical failure result");
    assert!(!stale_result.success);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_load_of_same_id_does_not_register_twice() {
    // 同 id 并发 load：load_lock 串行化，恰好一个成功，另一个 Conflict。
    let host = std::sync::Arc::new(make_host(HostConfig::permissive()));
    let bytes = echo_component().into_bytes();
    let signed = sign(&manifest("race.plugin", vec![]), &bytes);

    let h1 = host.clone();
    let h2 = host.clone();
    let s1 = signed.clone();
    let s2 = signed.clone();
    let b1 = bytes.clone();
    let b2 = bytes.clone();
    let (r1, r2) = tokio::join!(async move { h1.load(&s1, &b1).await }, async move {
        h2.load(&s2, &b2).await
    },);
    let r1_ok = r1.is_ok();
    let r2_ok = r2.is_ok();
    let r1_conflict = matches!(&r1, Err(e) if e.kind == PluginErrorKind::Conflict);
    let r2_conflict = matches!(&r2, Err(e) if e.kind == PluginErrorKind::Conflict);
    let ok_count = [r1_ok, r2_ok].iter().filter(|&&x| x).count();
    let conflict_count = [r1_conflict, r2_conflict].iter().filter(|&&x| x).count();
    assert_eq!(ok_count, 1, "exactly one load should succeed");
    assert_eq!(conflict_count, 1, "the other must be Conflict");
    assert_eq!(host.loaded_plugins().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_state_mutation_applies_through_trait() {
    // 通过 Plugin trait 派发 Lifecycle 事件，组件返回 state 变更且声明 PersistentState，
    // 确认变更真正落库（trait impl 与 host 共用同一 state store）。
    let host = make_host(HostConfig::permissive());
    let response = r#"{"status":"success","data":{"result":{},"state_mutations":[{"type":"set","key":"boot","value":{"count":7}}]}}"#;
    let bytes = fixed_response_component(response).into_bytes();
    let pid = PluginId::from("lcstate.plugin");
    let mut mf = manifest(
        "lcstate.plugin",
        vec![
            PluginCapability::LifecycleHook,
            PluginCapability::PersistentState,
        ],
    );
    mf.lifecycle_hooks.push(PluginLifecycleEventKind::Start);
    let signed = sign(&mf, &bytes);
    host.load(&signed, &bytes).await.expect("load");

    let plugin = host.get(&pid).expect("loaded");
    let dyn_plugin: std::sync::Arc<dyn plugin_api::Plugin> = plugin.clone() as _;
    dyn_plugin
        .on_lifecycle_event(
            PluginLifecycleEvent::Start,
            PluginContext {
                instance_id: CoreInstanceId::from("core"),
                workspace_id: Some(WorkspaceId::from("ws")),
                session_id: None,
                run_id: None,
            },
            CancellationToken::new(),
        )
        .await
        .expect("lifecycle ok");

    let snap = host
        .state_store()
        .snapshot(
            &pid,
            &plugin_api::PluginStateScope::Workspace(WorkspaceId::from("ws")),
        )
        .expect("snapshot");
    assert_eq!(snap.values["boot"]["count"], serde_json::json!(7));
    assert_eq!(snap.revision, 1);
}
