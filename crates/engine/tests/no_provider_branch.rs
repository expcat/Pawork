//! 回归：守护「Agent Engine 禁止按 Provider 名称走分支」。
//!
//! Agent 核心不得认识任何具体 Provider 名字。本测试扫描本 crate 的 `src/`，
//! 一旦出现已知 provider 名串（用于比较 / 特例判断）即失败，防止未来回归。
//! 名单自 R5 波 A 起从 providers CHANNEL_REGISTRY 派生（首发通道 id）并叠加
//! 固化基线别名；新增通道自动进入守护名单，无需手改本文件（S12-CR06-10 根治）。

use std::fs;
use std::path::{Path, PathBuf};

use pawork_providers::CHANNEL_REGISTRY;

/// 固化基线别名（非首发通道的常见 provider 名与短别名；与通道注册表无关，
/// 保持 R5 波 A 前手写名单的别名强度，审查 P2 确认不缩减）。
const BASELINE_PROVIDER_ALIASES: &[&str] = &[
    "openai",
    "anthropic",
    "claude",
    "grok",
    "glm",
    "opencode",
    "qwen",
    "google",
    "gemini",
    "bedrock",
    "mistral",
    "azure",
    "ollama",
    "vllm",
];

/// 守护名单：CHANNEL_REGISTRY 派生（首发通道 id）+ 固化基线别名（小写）。
fn forbidden_provider_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = CHANNEL_REGISTRY.iter().map(|preset| preset.id).collect();
    names.extend_from_slice(BASELINE_PROVIDER_ALIASES);
    names.sort_unstable();
    names.dedup();
    names
}

#[test]
fn guard_list_derives_from_channel_registry() {
    let names = forbidden_provider_names();
    assert!(!names.is_empty(), "registry-derived guard list must not be empty");
    for expected in [
        "chatgpt",
        "xai",
        "glm-coding",
        "opencode-go",
        "qwen-token-plan",
        "deepseek",
    ] {
        assert!(
            names.contains(&expected),
            "guard list missing first-party channel {expected}"
        );
    }
}

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
    let forbidden = forbidden_provider_names();

    for path in collect_rs_files(&src) {
        let rel = path
            .strip_prefix(&src)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let content = fs::read_to_string(&path).unwrap_or_default();
        for name in find_forbidden(&content, &forbidden) {
            offenders.push(format!("{rel}: 出现 provider 名 \"{name}\""));
        }
    }

    assert!(
        offenders.is_empty(),
        "Agent 核心源码不得出现具体 provider 名。违规：\n{}",
        offenders.join("\n")
    );
}
