//! P6-9 回归测试：守护 ADR-002「Agent Engine 禁止按 Provider 名称走分支」。
//!
//! Agent 核心不得认识任何具体 Provider 名字。本测试扫描本 crate 的 `src/`，
//! 一旦出现已知 provider 名串（用于比较 / 特例判断）即失败，防止未来回归。
//!
//! P15-9 扩展：Phase 15 新增 hosted_tools / ReasoningItem / ReasoningEffort /
//! capability 协商（`ResolvedCapabilities` / `CapabilityNegotiator`）属 canonical
//! 逻辑，同样禁止按 Provider 名分支。本测试额外扫描承载这些构造的 Core 纯逻辑
//! 文件（`provider-runtime/src/negotiate.rs`、`provider-runtime/src/capability.rs`、
//! `agent-domain/src/reasoning.rs`），并断言 Phase 15 canonical 符号确实出现在
//! 被守护的 Core 源码中——证明守护范围覆盖新增构造而非空扫。

use std::fs;
use std::path::{Path, PathBuf};

/// 已知 provider 名（小写）。Agent 核心源码不应出现这些字面量。
const FORBIDDEN_PROVIDER_NAMES: &[&str] = &[
    "openai",
    "anthropic",
    "claude",
    "google",
    "gemini",
    "bedrock",
    "mistral",
    "azure",
    "ollama",
    "vllm",
];

/// 递归收集目录下所有 `.rs` 文件（目录缺失返回空）。
fn collect_rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = match fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    out
}

/// 在文件文本中查找禁用 provider 名串，返回命中的名列表。
fn find_forbidden(content: &str, forbidden: &[&str]) -> Vec<String> {
    let lower = content.to_ascii_lowercase();
    forbidden
        .iter()
        .filter(|name| lower.contains(*name))
        .map(|name| (*name).to_string())
        .collect()
}

#[test]
fn agent_core_source_has_no_provider_name_branches() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = manifest_dir.join("src");
    let mut offenders: Vec<String> = Vec::new();

    for path in collect_rs_files(&src) {
        let rel = path
            .strip_prefix(&src)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let content = fs::read_to_string(&path).unwrap_or_default();
        for name in find_forbidden(&content, FORBIDDEN_PROVIDER_NAMES) {
            offenders.push(format!("{rel}: 出现 provider 名 \"{name}\""));
        }
    }

    assert!(
        offenders.is_empty(),
        "Agent 核心源码不得出现具体 provider 名（ADR-002）。违规：\n{}",
        offenders.join("\n")
    );
}

/// Phase 15 canonical 协商 / 能力 / reasoning 逻辑文件不得出现 Provider 名分支。
///
/// 只扫描承载 hosted_tools / ReasoningItem / capability 协商的纯逻辑文件
/// （`provider-runtime/src/negotiate.rs`、`provider-runtime/src/capability.rs`、
/// `agent-domain/src/reasoning.rs`）。这些文件按 ADR-002 必须是 Provider-neutral
/// 的；doc 注释与测试夹具里的 Provider 名（如 `provider-api` / `server_tool` 中
/// 的示例）不属于本守护范围，故不纳入此扫描，避免误报。
#[test]
fn phase15_canonical_logic_has_no_provider_name_branches() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let targets = [
        "../provider-runtime/src/negotiate.rs",
        "../provider-runtime/src/capability.rs",
        "../agent-domain/src/reasoning.rs",
    ];
    let mut offenders: Vec<String> = Vec::new();
    for rel in targets {
        let path = manifest_dir.join(rel);
        let Some(content) = fs::read_to_string(&path).ok() else {
            // 同级 crate 源文件缺失（例如单 crate 检出）时跳过，不构成违规。
            continue;
        };
        for name in find_forbidden(&content, FORBIDDEN_PROVIDER_NAMES) {
            offenders.push(format!("{rel}: 出现 provider 名 \"{name}\""));
        }
    }
    assert!(
        offenders.is_empty(),
        "Phase 15 canonical 协商/能力/reasoning 逻辑不得出现 provider 名（ADR-002）。违规：\n{}",
        offenders.join("\n")
    );
}

/// 守护范围回归：Phase 15 canonical 符号确实出现在被守护的 Core 源码中。
///
/// 防止上一条守护因路径漂移或文件改名而「空扫通过」。断言 hosted_tools /
/// ReasoningItem / ReasoningEffort / ResolvedCapabilities / CapabilityNegotiator
/// 至少各出现一次于 Core src（agent-domain / provider-api / provider-runtime）。
#[test]
fn phase15_guard_covers_canonical_symbols() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut core_src: Vec<PathBuf> = Vec::new();
    for crate_rel in [
        "../agent-domain/src",
        "../provider-api/src",
        "../provider-runtime/src",
    ] {
        let dir = manifest_dir.join(crate_rel);
        core_src.extend(collect_rs_files(&dir));
    }
    // 守护范围必须非空（至少能定位到 Core src），否则后续断言无意义。
    assert!(
        !core_src.is_empty(),
        "未能定位 Core src（agent-domain/provider-api/provider-runtime），守护范围异常"
    );

    let symbols = [
        "hosted_tools",
        "ReasoningItem",
        "ReasoningEffort",
        "ResolvedCapabilities",
        "CapabilityNegotiator",
    ];
    let mut missing: Vec<&str> = Vec::new();
    for symbol in symbols {
        let found = core_src.iter().any(|path| {
            fs::read_to_string(path)
                .unwrap_or_default()
                .contains(symbol)
        });
        if !found {
            missing.push(symbol);
        }
    }
    assert!(
        missing.is_empty(),
        "Phase 15 守护应覆盖以下 canonical 符号，但 Core src 中未找到：{:?}",
        missing
    );
}
