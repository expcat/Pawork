//! 回归：守护「Agent Engine 禁止按 Provider 名称走分支」。
//!
//! Agent 核心不得认识任何具体 Provider 名字。本测试扫描本 crate 的 `src/`，
//! 一旦出现已知 provider 名串（用于比较 / 特例判断）即失败，防止未来回归。

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
        "Agent 核心源码不得出现具体 provider 名。违规：\n{}",
        offenders.join("\n")
    );
}
