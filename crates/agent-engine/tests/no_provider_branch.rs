//! P6-9 回归测试：守护 ADR-002「Agent Engine 禁止按 Provider 名称走分支」。
//!
//! Agent 核心不得认识任何具体 Provider 名字。本测试扫描本 crate 的 `src/`，
//! 一旦出现已知 provider 名串（用于比较 / 特例判断）即失败，防止未来回归。

use std::fs;
use std::path::PathBuf;

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

#[test]
fn agent_core_source_has_no_provider_name_branches() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = manifest_dir.join("src");
    let mut offenders: Vec<String> = Vec::new();

    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let rel = path
                .strip_prefix(&src)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let content = fs::read_to_string(&path).unwrap_or_default();
            let lower = content.to_ascii_lowercase();
            for name in FORBIDDEN_PROVIDER_NAMES {
                if lower.contains(name) {
                    offenders.push(format!("{rel}: 出现 provider 名 \"{name}\""));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "Agent 核心源码不得出现具体 provider 名（ADR-002）。违规：\n{}",
        offenders.join("\n")
    );
}
