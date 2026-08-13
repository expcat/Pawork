//! P17-3 Marketplace 定向集成测试：全部 I/O 经 InMemorySourceIo + RecordingHost +
//! MemoryStore / AtomicFileStore，完全离线；覆盖事务语义、签名、trust、policy、
//! pin、依赖、多 source 与状态重放。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use agent_domain::MonitorId;
use ed25519_dalek::SigningKey;
use marketplace::{
    content_digest_hex, sign_archive, AtomicFileStore, InMemorySourceIo, IndexEntry, Keyring,
    Marketplace, MarketplaceError, MemoryStore, Pin, RecordingHost, SourceIndex, SourceSpec,
    TeamPolicy, TrustLevel,
};
use plugin_package::{
    read_archive, write_archive, McpServerDeclaration, McpTransportSpec, MonitorDeclaration,
    MonitorDriverEntry, MonitorLifecycle, PackageDependency, PackageId, PackageManifest,
    PackageRelativePath, PackageScope, ResourceRef, PACKAGE_MANIFEST_VERSION,
};
use semver::{Version, VersionReq};
use serde_json::json;

fn pkg_id(id: &str) -> PackageId {
    PackageId::new(id).unwrap()
}

fn ops(list: &[&str]) -> Vec<String> {
    list.iter().map(|op| op.to_string()).collect()
}

fn trusted_source(name: &str) -> SourceSpec {
    SourceSpec::registry(name, format!("https://{name}.example/index.json"))
        .with_trust(TrustLevel::Trusted)
}

fn plain_source(name: &str) -> SourceSpec {
    SourceSpec::registry(name, format!("https://{name}.example/index.json"))
}

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn full_manifest_with(
    id: &str,
    version: &str,
    skill_path: &str,
    dependencies: Vec<PackageDependency>,
) -> PackageManifest {
    let driver =
        MonitorDriverEntry::default().with_config(json!({"kind": "file_change", "paths": ["src"]}));
    PackageManifest {
        manifest_version: PACKAGE_MANIFEST_VERSION,
        id: pkg_id(id),
        name: format!("ACME {id}"),
        version: Version::parse(version).unwrap(),
        license: None,
        description: None,
        entrypoint: None,
        scope: PackageScope::Global,
        dependencies,
        skills: vec![ResourceRef::Path {
            path: PackageRelativePath::new(skill_path).unwrap(),
        }],
        agents: vec![ResourceRef::Inline {
            manifest: json!({"name": "acme-default"}),
        }],
        hooks: vec![ResourceRef::Inline {
            manifest: json!({"trigger": "run_started"}),
        }],
        mcp: vec![McpServerDeclaration {
            name: "fs".into(),
            transport: McpTransportSpec::Stdio {
                command: "fs-server".into(),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
            auto_start: false,
        }],
        lsp: vec![ResourceRef::Path {
            path: PackageRelativePath::new("lsp/rust.toml").unwrap(),
        }],
        monitors: vec![MonitorDeclaration::new(
            MonitorId::new("watch-build"),
            driver,
            MonitorLifecycle::TaskManager,
        )],
    }
}

fn full_manifest(id: &str, version: &str) -> PackageManifest {
    full_manifest_with(id, version, "skills/search", Vec::new())
}

fn minimal_manifest(id: &str, version: &str) -> PackageManifest {
    PackageManifest {
        manifest_version: PACKAGE_MANIFEST_VERSION,
        id: pkg_id(id),
        name: format!("ACME {id}"),
        version: Version::parse(version).unwrap(),
        license: None,
        description: None,
        entrypoint: None,
        scope: PackageScope::Global,
        dependencies: Vec::new(),
        skills: Vec::new(),
        agents: Vec::new(),
        hooks: Vec::new(),
        mcp: Vec::new(),
        lsp: Vec::new(),
        monitors: Vec::new(),
    }
}

fn write_full_package(dir: &Path, manifest: &PackageManifest) {
    fs::create_dir_all(dir).unwrap();
    for resource in &manifest.skills {
        if let Some(path) = resource.path() {
            let skill_dir = dir.join(path.as_path());
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(skill_dir.join("manifest.toml"), r#"name = "search""#).unwrap();
            fs::write(skill_dir.join("SKILL.md"), "# Search").unwrap();
        }
    }
    for resource in &manifest.lsp {
        if let Some(path) = resource.path() {
            let lsp_path = dir.join(path.as_path());
            if let Some(parent) = lsp_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(lsp_path, r#"id = "rust""#).unwrap();
        }
    }
    write_archive(dir, manifest).unwrap();
}

fn publish(
    io: &mut InMemorySourceIo,
    source: &str,
    dir: &Path,
    key: Option<(&str, &SigningKey)>,
    with_digest: bool,
) -> IndexEntry {
    let archive = read_archive(dir).unwrap();
    let digest_hex = content_digest_hex(&archive);
    let signature = key.map(|(key_id, signing_key)| sign_archive(key_id, signing_key, &archive));
    let entry = IndexEntry {
        id: archive.manifest.id.clone(),
        version: archive.manifest.version.clone(),
        location: "mem".into(),
        digest_hex: with_digest.then_some(digest_hex),
        signature,
    };
    io.publish_dir(source, &entry, dir.to_path_buf());
    entry
}

fn setup(temp: &Path, sources: Vec<SourceSpec>, io: InMemorySourceIo) -> Marketplace {
    Marketplace::new(
        sources,
        Box::new(io),
        Box::new(MemoryStore::new()),
        temp.join("staging"),
    )
}

#[test]
fn install_registers_all_resources_in_order_and_persists_state() {
    let temp = tempfile::tempdir().unwrap();
    let pkg_dir = temp.path().join("pkg");
    let manifest = full_manifest("acme.pkg", "1.0.0");
    write_full_package(&pkg_dir, &manifest);
    let twin_dir = temp.path().join("twin");
    write_full_package(&twin_dir, &full_manifest("acme.twin", "1.0.0"));

    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, None, true);
    publish(&mut io, "acme", &twin_dir, None, true);
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();
    let id = pkg_id("acme.pkg");

    let outcome = marketplace.install(&mut host, &id, None, true).unwrap();

    assert_eq!(outcome.version.to_string(), "1.0.0");
    assert_eq!(outcome.source, "acme");
    assert_eq!(outcome.summary.skills, 1);
    assert_eq!(outcome.summary.monitors, 1);
    assert_eq!(
        host.ops,
        ops(&[
            "register skill skills/search",
            "register agent inline:acme-default",
            "register hook run_started",
            "register mcp fs",
            "register lsp lsp/rust.toml",
            "register monitor watch-build",
        ])
    );

    let installed = marketplace.installed().unwrap();
    let record = installed.get("acme.pkg").unwrap();
    assert_eq!(record.version.to_string(), "1.0.0");
    assert_eq!(record.source, "acme");
    assert_eq!(record.plan.total(), 6);
    let expected_digest = content_digest_hex(&read_archive(&pkg_dir).unwrap());
    assert_eq!(record.digest_hex, expected_digest);
    assert_eq!(outcome.digest_hex, expected_digest);

    // 资源键冲突的包（不同 id、相同子资源）被拒绝。
    let twin = pkg_id("acme.twin");
    let error = marketplace
        .install(&mut host, &twin, None, true)
        .unwrap_err();
    assert!(matches!(error, MarketplaceError::ResourceConflict { .. }));
}

fn skill_only_manifest(id: &str, version: &str, skill_path: &str) -> PackageManifest {
    PackageManifest {
        manifest_version: PACKAGE_MANIFEST_VERSION,
        id: pkg_id(id),
        name: format!("ACME {id}"),
        version: Version::parse(version).unwrap(),
        license: None,
        description: None,
        entrypoint: None,
        scope: PackageScope::Global,
        dependencies: Vec::new(),
        skills: vec![ResourceRef::Path {
            path: PackageRelativePath::new(skill_path).unwrap(),
        }],
        agents: Vec::new(),
        hooks: Vec::new(),
        mcp: Vec::new(),
        lsp: Vec::new(),
        monitors: Vec::new(),
    }
}

fn write_skill_identity_package(dir: &Path, manifest: &PackageManifest, skill_id: &str) {
    fs::create_dir_all(dir).unwrap();
    for resource in &manifest.skills {
        if let Some(path) = resource.path() {
            let skill_dir = dir.join(path.as_path());
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("manifest.toml"),
                format!("id = \"{skill_id}\"\n"),
            )
            .unwrap();
            fs::write(skill_dir.join("SKILL.md"), "# Search").unwrap();
        }
    }
    write_archive(dir, manifest).unwrap();
}

#[test]
fn install_rejects_same_skill_identity_at_different_paths() {
    let temp = tempfile::tempdir().unwrap();
    let alpha_dir = temp.path().join("alpha");
    let beta_dir = temp.path().join("beta");
    write_skill_identity_package(
        &alpha_dir,
        &skill_only_manifest("acme.alpha", "1.0.0", "skills/alpha"),
        "search",
    );
    write_skill_identity_package(
        &beta_dir,
        &skill_only_manifest("acme.beta", "1.0.0", "skills/beta"),
        "search",
    );

    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &alpha_dir, None, true);
    publish(&mut io, "acme", &beta_dir, None, true);
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();

    marketplace
        .install(&mut host, &pkg_id("acme.alpha"), None, true)
        .unwrap();
    let error = marketplace
        .install(&mut host, &pkg_id("acme.beta"), None, true)
        .unwrap_err();
    match error {
        MarketplaceError::ResourceConflict { kind, key, package } => {
            assert_eq!(kind, "skill");
            assert_eq!(key, "search");
            assert_eq!(package, "acme.alpha");
        }
        other => panic!("expected skill identity conflict, got {other:?}"),
    }
}

#[test]
fn install_rolls_back_when_host_registration_fails() {
    let temp = tempfile::tempdir().unwrap();
    let pkg_dir = temp.path().join("pkg");
    let manifest = full_manifest("acme.pkg", "1.0.0");
    write_full_package(&pkg_dir, &manifest);
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, None, true);
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();
    host.fail_next("register mcp fs");
    let id = pkg_id("acme.pkg");

    let error = marketplace.install(&mut host, &id, None, true).unwrap_err();

    assert!(matches!(error, MarketplaceError::Package(_)));
    assert_eq!(
        host.ops,
        ops(&[
            "register skill skills/search",
            "register agent inline:acme-default",
            "register hook run_started",
            "unregister hook run_started",
            "unregister agent inline:acme-default",
            "unregister skill skills/search",
        ])
    );
    assert_eq!(host.registered_count(), 0);
    assert!(marketplace.installed().unwrap().is_empty());
}

#[test]
fn tampered_archive_fails_signature_verification() {
    let temp = tempfile::tempdir().unwrap();
    let pkg_dir = temp.path().join("pkg");
    let manifest = full_manifest("acme.pkg", "1.0.0");
    write_full_package(&pkg_dir, &manifest);
    let key = signing_key();
    let mut keyring = Keyring::default();
    keyring.insert("key-1", key.verifying_key());

    let mut io = InMemorySourceIo::new();
    // 不带 digest：确保命中签名闸门而非 bundle hash 闸门。
    publish(&mut io, "acme", &pkg_dir, Some(("key-1", &key)), false);
    // 篡改内容并重建归档：归档自洽但签名失配。
    fs::write(pkg_dir.join("skills/search/SKILL.md"), "# tampered").unwrap();
    write_archive(&pkg_dir, &manifest).unwrap();

    let mut marketplace =
        setup(temp.path(), vec![trusted_source("acme")], io).with_keyring(keyring);
    let mut host = RecordingHost::new();
    let id = pkg_id("acme.pkg");

    let error = marketplace.install(&mut host, &id, None, true).unwrap_err();

    assert!(matches!(error, MarketplaceError::Signature { .. }));
    assert!(host.ops.is_empty());
    assert!(marketplace.installed().unwrap().is_empty());
}

#[test]
fn stale_index_digest_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let pkg_dir = temp.path().join("pkg");
    write_full_package(&pkg_dir, &full_manifest("acme.pkg", "1.0.0"));
    let mut io = InMemorySourceIo::new();
    let mut entry = publish(&mut io, "acme", &pkg_dir, None, true);
    entry.digest_hex = Some("deadbeef".into());
    io.set_index(
        "acme",
        SourceIndex {
            packages: vec![entry],
        },
    );

    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();
    let id = pkg_id("acme.pkg");

    let error = marketplace.install(&mut host, &id, None, true).unwrap_err();

    assert!(matches!(error, MarketplaceError::BundleHashMismatch { .. }));
    assert!(host.ops.is_empty());
    assert!(marketplace.installed().unwrap().is_empty());
}

#[test]
fn hash_pin_enforced_at_resolve_and_after_fetch() {
    let temp = tempfile::tempdir().unwrap();
    let pkg_dir = temp.path().join("pkg");
    write_full_package(&pkg_dir, &full_manifest("acme.pkg", "1.0.0"));
    let lazy_dir = temp.path().join("lazy");
    write_full_package(&lazy_dir, &full_manifest("acme.lazy", "1.0.0"));
    let real_digest = content_digest_hex(&read_archive(&pkg_dir).unwrap());

    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, None, true);
    publish(&mut io, "acme", &lazy_dir, None, false);
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();
    let id = pkg_id("acme.pkg");
    let lazy_id = pkg_id("acme.lazy");

    // 错误 pin + 索引带摘要 → 解析阶段拒绝。
    marketplace.set_pin(&id, Pin::hash("deadbeef")).unwrap();
    let error = marketplace.install(&mut host, &id, None, true).unwrap_err();
    assert!(matches!(error, MarketplaceError::HashPinMismatch { .. }));

    // 正确 pin → 安装成功。
    marketplace.set_pin(&id, Pin::hash(real_digest)).unwrap();
    marketplace.install(&mut host, &id, None, true).unwrap();

    // 索引摘要缺失 → 延迟到拉取后重算，仍 fail-closed。
    marketplace
        .set_pin(&lazy_id, Pin::hash("deadbeef"))
        .unwrap();
    let error = marketplace
        .install(&mut host, &lazy_id, None, true)
        .unwrap_err();
    assert!(matches!(error, MarketplaceError::HashPinMismatch { .. }));
    assert_eq!(marketplace.installed().unwrap().len(), 1);
}

#[test]
fn exact_pin_selects_version_and_conflicts_with_range() {
    let temp = tempfile::tempdir().unwrap();
    let dir_12 = temp.path().join("v12");
    write_full_package(&dir_12, &full_manifest("acme.pkg", "1.2.0"));
    let dir_20 = temp.path().join("v20");
    write_full_package(&dir_20, &full_manifest("acme.pkg", "2.0.0"));
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &dir_12, None, true);
    publish(&mut io, "acme", &dir_20, None, true);
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();
    let id = pkg_id("acme.pkg");

    marketplace
        .set_pin(&id, Pin::exact(Version::new(1, 2, 0)))
        .unwrap();

    // 范围不覆盖 pin 版本 → VersionPinViolation。
    let range = VersionReq::parse("^2").unwrap();
    let error = marketplace
        .install(&mut host, &id, Some(&range), true)
        .unwrap_err();
    assert!(matches!(
        error,
        MarketplaceError::VersionPinViolation { .. }
    ));

    // 无范围时安装 pin 版本。
    let outcome = marketplace.install(&mut host, &id, None, true).unwrap();
    assert_eq!(outcome.version.to_string(), "1.2.0");
}

#[test]
fn requirement_selects_highest_matching_version() {
    let temp = tempfile::tempdir().unwrap();
    let mut io = InMemorySourceIo::new();
    for version in ["1.0.0", "1.4.2", "2.0.0"] {
        let dir = temp.path().join(format!("v{version}"));
        write_full_package(&dir, &full_manifest("acme.pkg", version));
        publish(&mut io, "acme", &dir, None, true);
    }
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();
    let id = pkg_id("acme.pkg");
    let range = VersionReq::parse("^1").unwrap();
    let outcome = marketplace
        .install(&mut host, &id, Some(&range), true)
        .unwrap();
    assert_eq!(outcome.version.to_string(), "1.4.2");
}

#[test]
fn update_switches_versions_with_exact_operation_sequence() {
    let temp = tempfile::tempdir().unwrap();
    let dir_10 = temp.path().join("v10");
    write_full_package(&dir_10, &full_manifest("acme.pkg", "1.0.0"));
    let dir_11 = temp.path().join("v11");
    write_full_package(&dir_11, &full_manifest("acme.pkg", "1.1.0"));
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &dir_10, None, true);
    publish(&mut io, "acme", &dir_11, None, true);
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();
    let id = pkg_id("acme.pkg");
    let range_10 = VersionReq::parse("=1.0.0").unwrap();
    marketplace
        .install(&mut host, &id, Some(&range_10), true)
        .unwrap();
    host.ops.clear();

    let outcome = marketplace.update(&mut host, &id, None, true).unwrap();

    assert!(outcome.switched);
    assert_eq!(outcome.from.to_string(), "1.0.0");
    assert_eq!(outcome.to.to_string(), "1.1.0");
    assert_eq!(
        host.ops,
        ops(&[
            "stop monitor watch-build",
            "unregister monitor watch-build",
            "unregister lsp lsp/rust.toml",
            "unregister mcp fs",
            "unregister hook run_started",
            "unregister agent inline:acme-default",
            "unregister skill skills/search",
            "register skill skills/search",
            "register agent inline:acme-default",
            "register hook run_started",
            "register mcp fs",
            "register lsp lsp/rust.toml",
            "register monitor watch-build",
        ])
    );
    assert_eq!(
        marketplace.installed().unwrap()["acme.pkg"]
            .version
            .to_string(),
        "1.1.0"
    );

    // 解析到相同版本的更新 → 验证过的 no-op。
    host.ops.clear();
    let range_11 = VersionReq::parse("^1.1").unwrap();
    let noop = marketplace
        .update(&mut host, &id, Some(&range_11), true)
        .unwrap();
    assert!(!noop.switched);
    assert!(host.ops.is_empty());
}

#[test]
fn update_failure_restores_previous_package() {
    let temp = tempfile::tempdir().unwrap();
    let dir_10 = temp.path().join("v10");
    write_full_package(&dir_10, &full_manifest("acme.pkg", "1.0.0"));
    let dir_12 = temp.path().join("v12");
    write_full_package(
        &dir_12,
        &full_manifest_with("acme.pkg", "1.2.0", "skills/search2", Vec::new()),
    );
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &dir_10, None, true);
    publish(&mut io, "acme", &dir_12, None, true);
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();
    let id = pkg_id("acme.pkg");
    let range_10 = VersionReq::parse("=1.0.0").unwrap();
    let range_12 = VersionReq::parse("=1.2.0").unwrap();
    marketplace
        .install(&mut host, &id, Some(&range_10), true)
        .unwrap();
    host.ops.clear();

    // A：移除旧资源阶段（stop monitor）失败 → 无残留操作、状态不变。
    host.fail_next("stop monitor watch-build");
    let error = marketplace
        .update(&mut host, &id, Some(&range_12), true)
        .unwrap_err();
    assert!(matches!(error, MarketplaceError::Host { .. }));
    assert!(host.ops.is_empty());
    assert_eq!(
        marketplace.installed().unwrap()["acme.pkg"]
            .version
            .to_string(),
        "1.0.0"
    );

    // B：新资源注册阶段失败 → 旧资源全部恢复。
    host.fail_next("skills/search2");
    let error = marketplace
        .update(&mut host, &id, Some(&range_12), true)
        .unwrap_err();
    assert!(matches!(error, MarketplaceError::Package(_)));
    assert_eq!(
        host.ops,
        ops(&[
            "stop monitor watch-build",
            "unregister monitor watch-build",
            "unregister lsp lsp/rust.toml",
            "unregister mcp fs",
            "unregister hook run_started",
            "unregister agent inline:acme-default",
            "unregister skill skills/search",
            "register skill skills/search",
            "register agent inline:acme-default",
            "register hook run_started",
            "register mcp fs",
            "register lsp lsp/rust.toml",
            "register monitor watch-build",
        ])
    );
    assert_eq!(host.registered_count(), 6);
    assert!(host.is_registered("skill", "skills/search"));
    assert_eq!(
        marketplace.installed().unwrap()["acme.pkg"]
            .version
            .to_string(),
        "1.0.0"
    );
}

#[test]
fn uninstall_stops_monitor_first_and_removes_everything() {
    let temp = tempfile::tempdir().unwrap();
    let pkg_dir = temp.path().join("pkg");
    write_full_package(&pkg_dir, &full_manifest("acme.pkg", "1.0.0"));
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, None, true);
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();
    let id = pkg_id("acme.pkg");
    marketplace.install(&mut host, &id, None, true).unwrap();
    host.ops.clear();

    let outcome = marketplace.uninstall(&mut host, &id).unwrap();

    assert_eq!(outcome.removed, 6);
    assert_eq!(outcome.version.to_string(), "1.0.0");
    assert_eq!(
        host.ops,
        ops(&[
            "stop monitor watch-build",
            "unregister monitor watch-build",
            "unregister lsp lsp/rust.toml",
            "unregister mcp fs",
            "unregister hook run_started",
            "unregister agent inline:acme-default",
            "unregister skill skills/search",
        ])
    );
    assert_eq!(host.registered_count(), 0);
    assert!(marketplace.installed().unwrap().is_empty());

    let error = marketplace.uninstall(&mut host, &id).unwrap_err();
    assert!(matches!(error, MarketplaceError::NotInstalled(_)));
}

#[test]
fn team_policy_min_trust_overrides_user_approval() {
    let temp = tempfile::tempdir().unwrap();
    let pkg_dir = temp.path().join("pkg");
    write_full_package(&pkg_dir, &full_manifest("acme.pkg", "1.0.0"));
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, None, true);
    let policy = TeamPolicy {
        min_trust: TrustLevel::Verified,
        ..TeamPolicy::default()
    };
    // source 缺省 untrusted；用户批准不能覆盖组织策略。
    let mut marketplace = setup(temp.path(), vec![plain_source("acme")], io).with_policy(policy);
    let mut host = RecordingHost::new();
    let id = pkg_id("acme.pkg");

    let error = marketplace.install(&mut host, &id, None, true).unwrap_err();

    assert!(matches!(error, MarketplaceError::PolicyDenied(_)));
    assert!(host.ops.is_empty());
    assert!(marketplace.installed().unwrap().is_empty());
}

#[test]
fn team_policy_denied_sources_signature_and_versions() {
    let temp = tempfile::tempdir().unwrap();
    let pkg_dir = temp.path().join("pkg");
    write_full_package(&pkg_dir, &full_manifest("acme.pkg", "1.0.0"));
    let id = pkg_id("acme.pkg");
    let mut host = RecordingHost::new();

    let mut policy = TeamPolicy::default();
    policy.denied_sources.insert("acme".into());
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, None, true);
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io).with_policy(policy);
    let error = marketplace.install(&mut host, &id, None, true).unwrap_err();
    assert!(matches!(error, MarketplaceError::PolicyDenied(_)));

    let policy = TeamPolicy {
        require_signature: true,
        ..TeamPolicy::default()
    };
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, None, true);
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io).with_policy(policy);
    let error = marketplace.install(&mut host, &id, None, true).unwrap_err();
    assert!(matches!(error, MarketplaceError::PolicyDenied(_)));

    let policy = TeamPolicy {
        allowed_versions: BTreeMap::from([(
            "acme.pkg".to_string(),
            VersionReq::parse("^2").unwrap(),
        )]),
        ..TeamPolicy::default()
    };
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, None, true);
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io).with_policy(policy);
    let error = marketplace.install(&mut host, &id, None, true).unwrap_err();
    assert!(matches!(error, MarketplaceError::PolicyDenied(_)));
    assert!(marketplace.installed().unwrap().is_empty());
}

#[test]
fn trust_gates_untrusted_and_verified_sources() {
    let temp = tempfile::tempdir().unwrap();
    let pkg_dir = temp.path().join("pkg");
    write_full_package(&pkg_dir, &full_manifest("acme.pkg", "1.0.0"));
    let id = pkg_id("acme.pkg");

    // untrusted + 未显式批准 → 拒绝。
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, None, true);
    let mut marketplace = setup(temp.path(), vec![plain_source("acme")], io);
    let mut host = RecordingHost::new();
    let error = marketplace
        .install(&mut host, &id, None, false)
        .unwrap_err();
    assert!(matches!(error, MarketplaceError::TrustDenied { .. }));

    // untrusted + 显式批准 → 可安装。
    marketplace.install(&mut host, &id, None, true).unwrap();

    // verified + 无签名 → 批准也拒绝。
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, None, true);
    let mut verified = setup(
        temp.path(),
        vec![plain_source("acme").with_trust(TrustLevel::Verified)],
        io,
    );
    let error = verified.install(&mut host, &id, None, true).unwrap_err();
    assert!(matches!(error, MarketplaceError::TrustDenied { .. }));

    // verified + 有效签名 → 可安装。
    let key = signing_key();
    let mut keyring = Keyring::default();
    keyring.insert("key-1", key.verifying_key());
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, Some(("key-1", &key)), true);
    let mut signed = setup(
        temp.path(),
        vec![plain_source("acme").with_trust(TrustLevel::Verified)],
        io,
    )
    .with_keyring(keyring);
    let mut fresh_host = RecordingHost::new();
    signed.install(&mut fresh_host, &id, None, true).unwrap();
}

#[test]
fn multi_source_tie_break_and_discovery() {
    let temp = tempfile::tempdir().unwrap();
    let dir_a = temp.path().join("a");
    write_full_package(&dir_a, &full_manifest("acme.pkg", "1.0.0"));
    let dir_b = temp.path().join("b");
    write_full_package(&dir_b, &full_manifest("acme.pkg", "1.0.0"));
    let dir_only_b = temp.path().join("only-b");
    write_full_package(&dir_only_b, &minimal_manifest("acme.other", "0.1.0"));

    let mut io = InMemorySourceIo::new();
    publish(&mut io, "a", &dir_a, None, true);
    publish(&mut io, "b", &dir_b, None, true);
    publish(&mut io, "b", &dir_only_b, None, true);
    let mut marketplace = setup(
        temp.path(),
        vec![trusted_source("a"), trusted_source("b")],
        io,
    );

    let discovery = marketplace.discover().unwrap();
    let candidates = &discovery.packages["acme.pkg"];
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].source_name, "a");

    let mut host = RecordingHost::new();
    // 同 id@version 双 source → 取配置靠前者。
    let outcome = marketplace
        .install(&mut host, &pkg_id("acme.pkg"), None, true)
        .unwrap();
    assert_eq!(outcome.source, "a");
    // 仅存在于 b 的包 → 从 b 发现并安装。
    let outcome = marketplace
        .install(&mut host, &pkg_id("acme.other"), None, true)
        .unwrap();
    assert_eq!(outcome.source, "b");
}

#[test]
fn atomic_file_store_replays_state_across_instances() {
    let temp = tempfile::tempdir().unwrap();
    let state_path = temp.path().join("state.json");
    let staging = temp.path().join("staging");
    let pkg_dir = temp.path().join("pkg");
    write_full_package(&pkg_dir, &full_manifest("acme.pkg", "1.0.0"));
    let id = pkg_id("acme.pkg");

    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, None, true);
    let mut first = Marketplace::new(
        vec![trusted_source("acme")],
        Box::new(io),
        Box::new(AtomicFileStore::new(state_path.clone())),
        staging.clone(),
    );
    let mut host = RecordingHost::new();
    first.install(&mut host, &id, None, true).unwrap();
    drop(first);

    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, None, true);
    let mut second = Marketplace::new(
        vec![trusted_source("acme")],
        Box::new(io),
        Box::new(AtomicFileStore::new(state_path.clone())),
        staging.clone(),
    );
    // 状态自磁盘重放。
    let installed = second.installed().unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed["acme.pkg"].version.to_string(), "1.0.0");
    assert_eq!(installed["acme.pkg"].plan.total(), 6);
    // 重放后重复安装被拒。
    let error = second.install(&mut host, &id, None, true).unwrap_err();
    assert!(matches!(error, MarketplaceError::AlreadyInstalled(_)));
    // 卸载使用持久化的 plan。
    let outcome = second.uninstall(&mut host, &id).unwrap();
    assert_eq!(outcome.removed, 6);

    // 损坏快照 → fail-closed。
    fs::write(&state_path, "not json").unwrap();
    let third = Marketplace::new(
        vec![trusted_source("acme")],
        Box::new(InMemorySourceIo::new()),
        Box::new(AtomicFileStore::new(state_path)),
        staging,
    );
    assert!(matches!(third.installed(), Err(MarketplaceError::State(_))));
}

#[test]
fn already_installed_not_found_and_no_matching_version() {
    let temp = tempfile::tempdir().unwrap();
    let pkg_dir = temp.path().join("pkg");
    write_full_package(&pkg_dir, &full_manifest("acme.pkg", "1.0.0"));
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &pkg_dir, None, true);
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();
    let id = pkg_id("acme.pkg");

    marketplace.install(&mut host, &id, None, true).unwrap();
    let error = marketplace.install(&mut host, &id, None, true).unwrap_err();
    assert!(matches!(error, MarketplaceError::AlreadyInstalled(_)));

    marketplace.uninstall(&mut host, &id).unwrap();
    let range = VersionReq::parse("^9").unwrap();
    let error = marketplace
        .install(&mut host, &id, Some(&range), true)
        .unwrap_err();
    assert!(matches!(error, MarketplaceError::NoMatchingVersion { .. }));

    let unknown = pkg_id("acme.unknown");
    let error = marketplace
        .install(&mut host, &unknown, None, true)
        .unwrap_err();
    assert!(matches!(error, MarketplaceError::PackageNotFound { .. }));
    let error = marketplace.uninstall(&mut host, &unknown).unwrap_err();
    assert!(matches!(error, MarketplaceError::NotInstalled(_)));
}

#[test]
fn staging_is_cleaned_after_success_and_failure() {
    let temp = tempfile::tempdir().unwrap();
    let ok_dir = temp.path().join("ok");
    write_full_package(&ok_dir, &full_manifest("acme.ok", "1.0.0"));
    let broken_dir = temp.path().join("broken");
    write_full_package(&broken_dir, &full_manifest("acme.broken", "1.0.0"));
    let mut io = InMemorySourceIo::new();
    publish(&mut io, "acme", &ok_dir, None, true);
    let broken_entry = publish(&mut io, "acme", &broken_dir, None, true);
    io.fail_archive("acme", &broken_entry, "network down");
    let staging = temp.path().join("staging");
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();

    marketplace
        .install(&mut host, &pkg_id("acme.ok"), None, true)
        .unwrap();
    let error = marketplace
        .install(&mut host, &pkg_id("acme.broken"), None, true)
        .unwrap_err();
    assert!(matches!(error, MarketplaceError::SourceIo { .. }));

    let leftovers: Vec<_> = fs::read_dir(&staging).unwrap().collect();
    assert!(leftovers.is_empty());
}

#[test]
fn package_dependencies_must_be_installed_and_match() {
    let temp = tempfile::tempdir().unwrap();
    let mut io = InMemorySourceIo::new();
    for version in ["1.2.0", "2.0.0"] {
        let dir = temp.path().join(format!("base-{version}"));
        write_full_package(&dir, &minimal_manifest("acme.base", version));
        publish(&mut io, "acme", &dir, None, true);
    }
    let dep_dir = temp.path().join("dep");
    let dep_manifest = full_manifest_with(
        "acme.pkg",
        "1.0.0",
        "skills/search",
        vec![
            PackageDependency::Package {
                id: pkg_id("acme.base"),
                version: "^1".into(),
            },
            // Provider 依赖由宿主运行时层校验，marketplace 跳过。
            PackageDependency::Provider {
                name: "acme-llm".into(),
                version: None,
            },
        ],
    );
    write_full_package(&dep_dir, &dep_manifest);
    publish(&mut io, "acme", &dep_dir, None, true);
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();
    let dep_id = pkg_id("acme.pkg");
    let base_id = pkg_id("acme.base");

    // 依赖缺失 → Resolution。
    let error = marketplace
        .install(&mut host, &dep_id, None, true)
        .unwrap_err();
    assert!(matches!(error, MarketplaceError::Resolution { .. }));

    // base 2.0.0 不满足 ^1 → Resolution。
    marketplace
        .install(&mut host, &base_id, None, true)
        .unwrap();
    let error = marketplace
        .install(&mut host, &dep_id, None, true)
        .unwrap_err();
    assert!(matches!(error, MarketplaceError::Resolution { .. }));

    // 换装 base 1.2.0 → 依赖满足，安装成功。
    marketplace.uninstall(&mut host, &base_id).unwrap();
    let range = VersionReq::parse("^1").unwrap();
    let outcome = marketplace
        .install(&mut host, &base_id, Some(&range), true)
        .unwrap();
    assert_eq!(outcome.version.to_string(), "1.2.0");
    let outcome = marketplace.install(&mut host, &dep_id, None, true).unwrap();
    assert_eq!(outcome.version.to_string(), "1.0.0");
}

#[test]
fn update_and_uninstall_preserve_the_dependency_closure_before_host_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let base_1 = temp.path().join("base-1");
    write_full_package(&base_1, &minimal_manifest("acme.base", "1.2.0"));
    let base_2 = temp.path().join("base-2");
    write_full_package(&base_2, &minimal_manifest("acme.base", "2.0.0"));

    let pkg_1 = temp.path().join("pkg-1");
    write_full_package(
        &pkg_1,
        &full_manifest_with(
            "acme.pkg",
            "1.0.0",
            "skills/search",
            vec![PackageDependency::Package {
                id: pkg_id("acme.base"),
                version: "^1".into(),
            }],
        ),
    );
    let pkg_2 = temp.path().join("pkg-2");
    write_full_package(
        &pkg_2,
        &full_manifest_with(
            "acme.pkg",
            "2.0.0",
            "skills/search",
            vec![PackageDependency::Package {
                id: pkg_id("acme.base"),
                version: "^3".into(),
            }],
        ),
    );

    let mut io = InMemorySourceIo::new();
    for dir in [&base_1, &base_2, &pkg_1, &pkg_2] {
        publish(&mut io, "acme", dir, None, true);
    }
    let mut marketplace = setup(temp.path(), vec![trusted_source("acme")], io);
    let mut host = RecordingHost::new();
    let base_id = pkg_id("acme.base");
    let pkg_id = pkg_id("acme.pkg");
    marketplace
        .install(
            &mut host,
            &base_id,
            Some(&VersionReq::parse("=1.2.0").unwrap()),
            true,
        )
        .unwrap();
    marketplace
        .install(
            &mut host,
            &pkg_id,
            Some(&VersionReq::parse("=1.0.0").unwrap()),
            true,
        )
        .unwrap();

    host.ops.clear();
    let error = marketplace
        .update(
            &mut host,
            &pkg_id,
            Some(&VersionReq::parse("=2.0.0").unwrap()),
            true,
        )
        .unwrap_err();
    assert!(matches!(error, MarketplaceError::Resolution { .. }));
    assert!(
        host.ops.is_empty(),
        "dependency failure must precede removal"
    );
    assert_eq!(
        marketplace.installed().unwrap()["acme.pkg"].version,
        Version::new(1, 0, 0)
    );

    let error = marketplace.uninstall(&mut host, &base_id).unwrap_err();
    assert!(matches!(error, MarketplaceError::Resolution { .. }));
    assert!(
        host.ops.is_empty(),
        "dependent failure must precede removal"
    );

    let error = marketplace
        .update(
            &mut host,
            &base_id,
            Some(&VersionReq::parse("=2.0.0").unwrap()),
            true,
        )
        .unwrap_err();
    assert!(matches!(error, MarketplaceError::Resolution { .. }));
    assert!(
        host.ops.is_empty(),
        "reverse dependency failure must precede removal"
    );
    assert_eq!(
        marketplace.installed().unwrap()["acme.base"].version,
        Version::new(1, 2, 0)
    );
}
