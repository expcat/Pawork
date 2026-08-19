//! 依赖红线断言(ADR-039 D2/D6):engine 是纯执行核,生产依赖中唯一的
//! pawork-* 是 pawork-domain。dev-dependencies 允许 pawork-testkit。

#[test]
fn engine_prod_deps_stay_domain_only() {
    let manifest = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let Some((_, rest)) = manifest.split_once("[dependencies]") else {
        panic!("engine Cargo.toml is missing [dependencies]");
    };
    let deps = rest
        .split("\n[")
        .next()
        .expect("dependency table")
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            line.split([' ', '=', '{'])
                .next()
                .map(str::trim)
                .filter(|name| !name.is_empty())
        })
        .collect::<Vec<_>>();
    assert!(
        deps.iter().any(|name| *name == "pawork-domain"),
        "engine must depend on pawork-domain: {deps:?}"
    );
    for name in deps.iter().filter(|name| name.starts_with("pawork-")) {
        assert!(
            *name == "pawork-domain",
            "engine must not depend on {name} (domain-only): {deps:?}"
        );
    }
}
