//! 依赖红线断言(ADR-039 D2/D6):engine 是纯执行核,生产依赖中唯一的
//! pawork-* 是 pawork-domain。dev-dependencies 允许 pawork-testkit 与
//! pawork-providers(仅守护测试名单派生,R5 波 A)。

#[test]
fn engine_prod_deps_stay_domain_only() {
    let manifest = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let deps = production_pawork_dependencies(manifest);
    assert_eq!(
        deps,
        ["pawork-domain".to_string()].into_iter().collect(),
        "engine production dependency boundary is domain-only"
    );
}

#[test]
fn production_dependency_scan_covers_alias_and_target_tables() {
    let manifest = r#"
[dependencies]
renamed-app = { path = "../app", package = "pawork-app" }

[target.'cfg(unix)'.dependencies]
pawork-storage = { path = "../storage" }

[dependencies.renamed-tools]
package = "pawork-tools"
path = "../tools"

[dev-dependencies]
pawork-engine = { path = "../engine" }
"#;
    assert_eq!(
        production_pawork_dependencies(manifest),
        ["pawork-app", "pawork-storage", "pawork-tools"]
            .into_iter()
            .map(str::to_string)
            .collect()
    );
}

fn production_pawork_dependencies(manifest: &str) -> std::collections::BTreeSet<String> {
    let mut in_production_dependencies = false;
    let mut dependencies = std::collections::BTreeSet::new();
    for raw in manifest.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            let section = line.trim_matches(['[', ']']);
            let dependency_suffix = section
                .strip_prefix("dependencies.")
                .or_else(|| {
                    section
                        .strip_prefix("target.")
                        .and_then(|section| section.rsplit_once(".dependencies."))
                        .map(|(_, dependency)| dependency)
                })
                .map(|dependency| dependency.trim_matches(['\'', '"']).to_string());
            in_production_dependencies = section == "dependencies"
                || (section.starts_with("target.") && section.ends_with(".dependencies"))
                || dependency_suffix.is_some();
            if let Some(dependency) = dependency_suffix
                .as_ref()
                .filter(|dependency| dependency.starts_with("pawork-"))
            {
                dependencies.insert(dependency.clone());
            }
            continue;
        }
        if !in_production_dependencies || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, _)) = line.split_once('=') {
            let key = key.trim().trim_matches(['\'', '"']);
            if key.starts_with("pawork-") {
                dependencies.insert(key.to_string());
            }
        }
        if let Some(package) = line
            .split_once("package")
            .and_then(|(_, field)| field.split_once('='))
            .and_then(|(_, value)| {
                value
                    .split('"')
                    .nth(1)
                    .filter(|package| package.starts_with("pawork-"))
            })
        {
            dependencies.insert(package.to_string());
        }
    }
    dependencies
}
