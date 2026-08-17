//! 五类来源 fixture smoke（P17-13 步骤 7）。
//!
//! 只用 crate 内 fixture 与 tempdir，绝不接触宿主环境；不运行 workspace 门禁。

use std::collections::BTreeSet;
use std::path::PathBuf;

use pawork_compat::{
    CompatLimits, CompatLoader, ExportOutcome, ExternalSource, GlobalSource, ImportCategory,
    ImportStatus,
};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn scan_fixtures() -> pawork_compat::CompatPlan {
    CompatLoader::default()
        .scan(Some(&fixture_root()), &[], None)
        .expect("scan fixtures")
}

#[test]
fn imports_all_five_sources_and_all_six_categories() {
    let plan = scan_fixtures();
    let sources: BTreeSet<ExternalSource> = plan.sources.iter().copied().collect();
    for source in ExternalSource::ALL {
        assert!(sources.contains(&source), "missing source {source:?}");
    }
    let categories: BTreeSet<ImportCategory> = plan
        .items
        .iter()
        .filter(|item| item.status == ImportStatus::Imported)
        .map(|item| item.category)
        .collect();
    for category in [
        ImportCategory::Instructions,
        ImportCategory::Skill,
        ImportCategory::McpServer,
        ImportCategory::AgentProfile,
        ImportCategory::UserHook,
        ImportCategory::PermissionRule,
    ] {
        assert!(
            categories.contains(&category),
            "missing category {category:?}"
        );
    }
    // 导入的 hook 默认禁用、需人工审查，绝不在导入期执行。
    let hook = plan
        .items
        .iter()
        .find(|item| {
            item.category == ImportCategory::UserHook && item.status == ImportStatus::Imported
        })
        .expect("imported user hook");
    assert!(hook.requires_review);
    let json = serde_json::to_value(hook.payload.as_ref().expect("payload")).expect("hook json");
    assert_eq!(json["hook"]["enabled"], false);
    assert_eq!(json["hook"]["handler"]["kind"], "command");
    assert_eq!(json["hook"]["handler"]["program"], "python");
}

#[test]
fn plaintext_secrets_are_replaced_by_references_only() {
    let plan = scan_fixtures();
    let serialized = serde_json::to_string(&plan).expect("serialize plan");
    for needle in [
        "sk-live-secret-abc123",
        "sk-ant-fixture-do-not-use",
        "hunter2",
        "Bearer",
    ] {
        assert!(!serialized.contains(needle), "secret leaked: {needle}");
    }
    // cursor http headers：字面量 Authorization 被丢弃、只留占位；${HEADER_TOKEN} 变成引用。
    let cursor = plan
        .items
        .iter()
        .find(|item| item.id == "mcp:cursor-web")
        .expect("cursor-web item");
    let json =
        serde_json::to_value(cursor.payload.as_ref().expect("payload")).expect("cursor json");
    let headers = &json["server"]["transport"]["headers"];
    assert_eq!(headers["X-Ref"]["account"], "cursor-web:X-Ref");
    assert!(headers.get("Authorization").is_none());
    let pending = json["pending_credentials"]
        .as_array()
        .expect("pending array");
    assert!(pending.iter().any(|entry| entry["name"] == "Authorization"));
    // codex stdio env：${GITHUB_TOKEN} → SecretRef。
    let github = plan
        .items
        .iter()
        .find(|item| item.id == "mcp:github")
        .expect("github item");
    let json =
        serde_json::to_value(github.payload.as_ref().expect("payload")).expect("github json");
    assert_eq!(
        json["server"]["transport"]["env"]["GITHUB_TOKEN"]["account"],
        "github:GITHUB_TOKEN"
    );
    assert!(plan
        .credential_references
        .iter()
        .any(|reference| reference.account == "github:GITHUB_TOKEN"));
}

#[test]
fn same_tier_conflict_prefers_higher_source_rank() {
    let plan = scan_fixtures();
    let group: Vec<&pawork_compat::CompatItem> = plan
        .items
        .iter()
        .filter(|item| {
            item.category == ImportCategory::PermissionRule && item.id == "permission:global"
        })
        .collect();
    assert_eq!(
        group.len(),
        2,
        "expected codex + claude global permission entries"
    );
    let winner = group
        .iter()
        .find(|item| item.status == ImportStatus::Imported)
        .expect("imported winner");
    assert_eq!(winner.source.external, ExternalSource::Codex);
    let loser = group
        .iter()
        .find(|item| item.status == ImportStatus::Conflict)
        .expect("conflict loser");
    assert_eq!(loser.source.external, ExternalSource::Claude);
    assert!(loser.payload.is_none());
    assert!(loser
        .issues
        .iter()
        .any(|issue| issue.code == "conflict_loser"));
}

#[test]
fn conflict_prefers_higher_tier_and_diagnoses_loser() {
    let temp = tempfile::tempdir().expect("tempdir");
    let global_root = temp.path().join("global");
    std::fs::create_dir_all(&global_root).expect("mkdir");
    std::fs::write(
        global_root.join(".mcp.json"),
        r#"{"mcpServers":{"echo":{"command":"echo","args":["global-echo"]}}}"#,
    )
    .expect("write global mcp json");
    let plan = CompatLoader::default()
        .scan(
            Some(&fixture_root()),
            &[GlobalSource::new(ExternalSource::Cursor, &global_root)],
            None,
        )
        .expect("scan with global source");
    let group: Vec<&pawork_compat::CompatItem> = plan
        .items
        .iter()
        .filter(|item| item.id == "mcp:echo")
        .collect();
    assert_eq!(group.len(), 2);
    let winner = group
        .iter()
        .find(|item| item.status == ImportStatus::Imported)
        .expect("workspace winner");
    assert_eq!(winner.source.tier.priority(), 3);
    let loser = group
        .iter()
        .find(|item| item.status == ImportStatus::Conflict)
        .expect("global loser");
    assert_eq!(loser.source.tier.priority(), 1);
    assert!(loser
        .issues
        .iter()
        .any(|issue| issue.code == "conflict_loser"));
}

#[test]
fn export_plan_is_explicit_idempotent_and_never_rewrites_sources() {
    let plan = scan_fixtures();
    let marker = fixture_root().join("CLAUDE.md");
    let before = std::fs::read_to_string(&marker).expect("read marker before");
    let temp = tempfile::tempdir().expect("tempdir");
    let output_dir = temp.path().join("out");
    let loader = CompatLoader::default();
    let first = loader.export_plan(&plan, &output_dir).expect("export_plan");
    assert_eq!(first.outcome, ExportOutcome::Exported);
    assert!(first.bytes_written > 0);
    assert!(output_dir.join("compat-import.json").is_file());
    assert!(output_dir.join(".compat-import-fingerprint").is_file());
    let second = loader
        .export_plan(&plan, &output_dir)
        .expect("export_plan again");
    assert_eq!(second.outcome, ExportOutcome::Noop);
    assert_eq!(second.bytes_written, 0);
    let after = std::fs::read_to_string(&marker).expect("read marker after");
    assert_eq!(before, after);
    // 写入的计划不含任何明文 secret。
    let persisted =
        std::fs::read_to_string(output_dir.join("compat-import.json")).expect("read plan");
    assert!(!persisted.contains("sk-live-secret-abc123"));
}

#[test]
fn dry_run_preview_lists_items_without_writing() {
    let plan = scan_fixtures();
    let preview = CompatLoader::default().dry_run(&plan);
    assert!(preview.contains("dry-run preview"));
    assert!(preview.contains("mcp:github"));
    assert!(preview.contains(&format!("fingerprint {}", plan.fingerprint)));
    let temp = tempfile::tempdir().expect("tempdir");
    let output_dir = temp.path().join("out");
    assert!(!output_dir.exists());
}

#[test]
fn select_keeps_only_requested_items() {
    let plan = scan_fixtures();
    let wanted: BTreeSet<String> = ["mcp:github".to_string()].into_iter().collect();
    let selected = plan.select(&wanted);
    assert!(!selected.items.is_empty());
    assert!(selected.items.iter().all(|item| item.id == "mcp:github"));
    assert_ne!(selected.fingerprint, plan.fingerprint);
    assert!(selected
        .credential_references
        .iter()
        .any(|reference| reference.account == "github:GITHUB_TOKEN"));

    let other: BTreeSet<String> = ["mcp:cursor-web".to_string()].into_iter().collect();
    let other_selected = plan.select(&other);
    assert_ne!(selected.fingerprint, other_selected.fingerprint);
    assert!(other_selected
        .credential_references
        .iter()
        .all(|reference| reference.account.starts_with("cursor-web:")));
}

#[test]
fn dangerous_and_unknown_content_is_isolated_not_imported() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    std::fs::create_dir_all(root.join(".claude")).expect("mkdir claude");
    std::fs::write(
        root.join(".claude/settings.json"),
        r#"{
  "permissions": { "defaultMode": "bypassPermissions" },
  "zebraz": 1
}"#,
    )
    .expect("write claude settings");
    std::fs::create_dir_all(root.join(".pi")).expect("mkdir pi");
    std::fs::write(
        root.join(".pi/settings.json"),
        r#"{"instructions": "ok", "mysteryKey": "hunter2"}"#,
    )
    .expect("write pi settings");
    let plan = CompatLoader::default()
        .scan(Some(root), &[], None)
        .expect("scan temp workspace");
    let serialized = serde_json::to_string(&plan).expect("serialize plan");
    assert!(!serialized.contains("hunter2"), "secret leaked");
    let bypass = plan
        .items
        .iter()
        .find(|item| item.id == "permission:global")
        .expect("bypass entry");
    assert_eq!(bypass.status, ImportStatus::Unsupported);
    assert!(bypass
        .issues
        .iter()
        .any(|issue| issue.code == "permission_bypass_not_imported"));
    assert!(plan.issues.iter().any(|issue| {
        issue.code == "unknown_key" && issue.source_path.as_deref() == Some(".claude/settings.json")
    }));
}

// —— P17-13 安全审查补强回归 ——

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed_during_scan() {
    // no-follow 读取：任何 symlink 组件（含指向根内 / 根外的）一律拒绝，不读取其目标。
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    std::fs::create_dir_all(root.join(".codex")).expect("mkdir codex");
    std::fs::write(
        temp.path().join("outside-secret"),
        "approval_policy = \"on-failure\"\n",
    )
    .expect("write outside target");
    // .codex/config.toml 作为 symlink 指向根外文件。
    symlink("../outside-secret", root.join(".codex/config.toml")).expect("symlink");
    let plan = CompatLoader::default()
        .scan(Some(root), &[], None)
        .expect("scan symlink workspace");
    let serialized = serde_json::to_string(&plan).expect("serialize plan");
    assert!(
        !serialized.contains("outside-secret"),
        "symlink target content must never be read"
    );
    assert!(plan.items.iter().all(|item| {
        !(item.id == "permission:global" && item.source.external == ExternalSource::Codex)
    }));

    // 根内 symlink 同样不跟随：写入真实文件再用 symlink 别名声明，别名不可读取。
    std::fs::write(root.join("CLAUDE.real.md"), "# real\n").expect("write real");
    symlink("CLAUDE.real.md", root.join("CLAUDE.md")).expect("symlink in root");
    let plan = CompatLoader::default()
        .scan(Some(root), &[], None)
        .expect("scan in-root symlink");
    assert!(plan
        .items
        .iter()
        .all(|item| item.source.relative_path != "CLAUDE.md"));
}

#[test]
fn per_kind_budget_hard_truncates_glob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    std::fs::create_dir_all(root.join(".claude/rules")).expect("mkdir rules");
    for index in 0..8u8 {
        std::fs::write(
            root.join(format!(".claude/rules/rule-{index}.md")),
            format!("# rule {index}\n"),
        )
        .expect("write rule");
    }
    let limits = CompatLimits {
        max_files_per_kind: 3,
        ..CompatLimits::default()
    };
    let plan = CompatLoader::new(limits)
        .scan(Some(root), &[], None)
        .expect("scan capped glob");
    assert!(
        plan.issues
            .iter()
            .any(|issue| issue.code == "scan_kind_limit"),
        "per-kind hard cap must emit scan_kind_limit"
    );
    let rule_items = plan
        .items
        .iter()
        .filter(|item| {
            item.category == ImportCategory::Instructions
                && item.source.relative_path.starts_with(".claude/rules/")
        })
        .count();
    assert!(
        rule_items <= 3,
        "per-kind budget must hard-truncate, got {rule_items}"
    );
}

#[test]
fn total_budget_hard_truncates_detection() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    std::fs::write(root.join("CLAUDE.md"), "# claude\n").expect("write claude");
    std::fs::write(root.join("CLAUDE.local.md"), "# local\n").expect("write local");
    std::fs::write(root.join("AGENTS.md"), "# agents\n").expect("write agents");
    let limits = CompatLimits {
        max_total_files: 2,
        ..CompatLimits::default()
    };
    let plan = CompatLoader::new(limits)
        .scan(Some(root), &[], None)
        .expect("scan total-capped");
    assert!(
        plan.issues
            .iter()
            .any(|issue| issue.code == "scan_total_limit"),
        "total hard cap must emit scan_total_limit"
    );
    let distinct_paths: BTreeSet<&str> = plan
        .items
        .iter()
        .map(|item| item.source.relative_path.as_str())
        .collect();
    assert!(
        distinct_paths.len() <= 2,
        "total budget must hard-truncate unique files, got {}",
        distinct_paths.len()
    );
}

#[test]
fn different_selects_export_separately_and_noop_checks_identity() {
    let plan = scan_fixtures();
    let temp = tempfile::tempdir().expect("tempdir");
    let output_dir = temp.path().join("out");
    let loader = CompatLoader::default();

    let select_a: BTreeSet<String> = ["mcp:github".to_string()].into_iter().collect();
    let select_b: BTreeSet<String> = ["mcp:cursor-web".to_string()].into_iter().collect();
    let plan_a = plan.select(&select_a);
    let plan_b = plan.select(&select_b);
    assert_ne!(plan_a.fingerprint, plan_b.fingerprint);

    // 不同选择互不为 noop：先写 a，再写 b 必须是 Exported（身份不同）。
    let exported_a = loader
        .export_plan(&plan_a, &output_dir)
        .expect("export_plan a");
    assert_eq!(exported_a.outcome, ExportOutcome::Exported);
    let exported_b = loader
        .export_plan(&plan_b, &output_dir)
        .expect("export_plan b");
    assert_eq!(
        exported_b.outcome,
        ExportOutcome::Exported,
        "different select must not be treated as noop"
    );
    // 相同计划 + 内容身份一致才 noop。
    let noop_b = loader
        .export_plan(&plan_b, &output_dir)
        .expect("export_plan b again");
    assert_eq!(noop_b.outcome, ExportOutcome::Noop);
    // 指纹仍为 a，但磁盘上是 b：再 export_plan a 必须重写（noop 仅在内容身份一致时成立）。
    let reexported_a = loader
        .export_plan(&plan_a, &output_dir)
        .expect("export_plan a again");
    assert_eq!(reexported_a.outcome, ExportOutcome::Exported);

    // 篡改计划文件但保留指纹：noop 必须因内容身份不符而重写。
    let plan_path = output_dir.join("compat-import.json");
    let before = std::fs::read(&plan_path).expect("read plan");
    let last = before.last().copied().unwrap_or(b'}');
    std::fs::write(&plan_path, b"tampered-by-test").expect("tamper plan");
    let repaired = loader
        .export_plan(&plan_a, &output_dir)
        .expect("repair export_plan");
    assert_eq!(
        repaired.outcome,
        ExportOutcome::Exported,
        "content identity mismatch must trigger rewrite, not noop"
    );
    let after = std::fs::read(&plan_path).expect("read repaired plan");
    assert_ne!(after, b"tampered-by-test");
    assert_eq!(after.last().copied().unwrap_or(b'}'), last);
}

#[test]
fn on_failure_approval_maps_to_ask_not_allow() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    std::fs::create_dir_all(root.join(".codex")).expect("mkdir codex");
    std::fs::write(
        root.join(".codex/config.toml"),
        "approval_policy = \"on-failure\"\n",
    )
    .expect("write codex config");
    let plan = CompatLoader::default()
        .scan(Some(root), &[], None)
        .expect("scan codex on-failure");
    let global = plan
        .items
        .iter()
        .find(|item| {
            item.id == "permission:global" && item.source.external == ExternalSource::Codex
        })
        .expect("codex global permission");
    assert_eq!(global.status, ImportStatus::Imported);
    assert!(global.requires_review);
    let json = serde_json::to_value(global.payload.as_ref().expect("payload")).expect("json");
    assert_eq!(
        json["decision"], "ask",
        "OnFailure must not widen to allow; it maps to ask"
    );
}

#[test]
fn conflict_winner_keeps_deterministic_rank_but_requires_review() {
    let plan = scan_fixtures();
    let winner = plan
        .items
        .iter()
        .find(|item| {
            item.category == ImportCategory::PermissionRule
                && item.id == "permission:global"
                && item.status == ImportStatus::Imported
        })
        .expect("imported conflict winner");
    assert_eq!(winner.source.external, ExternalSource::Codex);
    assert!(
        winner.requires_review,
        "deterministic winner of a conflict still needs human review"
    );
}
