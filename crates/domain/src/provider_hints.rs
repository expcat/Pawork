//! provider_hints 命名空间契约（R5 / T6）。
//!
//! Provider 特定的扩展元数据（reasoning hint 等）统一走
//! `provider_hints.<provider>.<key>` 命名空间：
//!
//!   - `provider`：非空，小写 `[a-z0-9-]+`；
//!   - `key`：非空，字符集为 ASCII 字母数字、`.`、`_`；
//!   - 整键 ≤ [`MAX_HINT_KEY_BYTES`] 字节，单值序列化后 ≤ [`MAX_HINT_VALUE_BYTES`] 字节。
//!
//! 事件信封形状不变：hints 本就位于 `ReasoningItem` 的 `opaque_metadata` /
//! `continuation_metadata` 开放地图内。存储层按本规则透传并叠加 Secret 键扫描、
//! 大小上限与已知形状校验，不再维护 provider 键名清单。
//!
//! 历史落盘键提供冻结的读兼容映射（[`LEGACY_HINT_KEY_MAP`]），写路径永不
//! 产出旧拼写：
//!
//!   - `responses.summary_entries`（R5 前生产者的无前缀拼写）
//!   - `openai.responses.summary_entries`（R5 前 storage allowlist 拼写）
//!   - `anthropic_block_kind`（R5 前 storage allowlist 拼写，预留脚手架）

/// 命名空间前缀。
pub const PROVIDER_HINTS_PREFIX: &str = "provider_hints.";

/// 整键长度上限（字节）。
pub const MAX_HINT_KEY_BYTES: usize = 128;

/// 单值序列化长度上限（字节，64 KiB）。
pub const MAX_HINT_VALUE_BYTES: usize = 64 * 1024;

/// OpenAI Responses reasoning summary 条目 hint 的规范键。
pub const OPENAI_RESPONSES_SUMMARY_ENTRIES_HINT: &str =
    "provider_hints.openai.responses.summary_entries";

/// Anthropic block kind hint 的规范键。
pub const ANTHROPIC_BLOCK_KIND_HINT: &str = "provider_hints.anthropic.block_kind";

/// 旧拼写 → 规范键（冻结数据；后续迁移只追加行，不改动既有行）。
pub const LEGACY_HINT_KEY_MAP: &[(&str, &str)] = &[
    (
        "responses.summary_entries",
        OPENAI_RESPONSES_SUMMARY_ENTRIES_HINT,
    ),
    (
        "openai.responses.summary_entries",
        OPENAI_RESPONSES_SUMMARY_ENTRIES_HINT,
    ),
    ("anthropic_block_kind", ANTHROPIC_BLOCK_KIND_HINT),
];

/// 判断是否为语法与大小上限均合法的 provider hint 键。
pub fn is_provider_hint_key(key: &str) -> bool {
    let Some(rest) = key.strip_prefix(PROVIDER_HINTS_PREFIX) else {
        return false;
    };
    if key.len() > MAX_HINT_KEY_BYTES {
        return false;
    }
    let Some((provider, hint)) = rest.split_once('.') else {
        return false;
    };
    valid_provider_segment(provider) && valid_hint_key_segment(hint)
}

/// 已知旧拼写 → 规范键查询；规范键与未知键返回 `None`。
pub fn canonical_hint_key(key: &str) -> Option<&'static str> {
    LEGACY_HINT_KEY_MAP
        .iter()
        .find(|(legacy, _)| *legacy == key)
        .map(|(_, canonical)| *canonical)
}

fn valid_provider_segment(provider: &str) -> bool {
    !provider.is_empty()
        && provider
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_hint_key_segment(hint: &str) -> bool {
    !hint.is_empty()
        && hint
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_namespaced_hint_keys() {
        assert!(is_provider_hint_key(OPENAI_RESPONSES_SUMMARY_ENTRIES_HINT));
        assert!(is_provider_hint_key(ANTHROPIC_BLOCK_KIND_HINT));
        assert!(is_provider_hint_key(
            "provider_hints.glm-coding.usage.note_1"
        ));
        assert!(is_provider_hint_key("provider_hints.openai.a.b.c"));
    }

    #[test]
    fn rejects_malformed_hint_keys() {
        for key in [
            "",
            "provider_hints",
            "provider_hints.",
            "provider_hints.openai",
            "provider_hints.openai.",
            "provider_hints..key",
            "provider_hints.OpenAI.key",
            "provider_hints.openai_key.hint",
            "provider_hints.openai/key",
            "responses.summary_entries",
            "openai.responses.summary_entries",
            "anthropic_block_kind",
        ] {
            assert!(!is_provider_hint_key(key), "must reject: {key}");
        }
    }

    #[test]
    fn key_length_limit_is_pinned_at_128_bytes() {
        let prefix = "provider_hints.openai.";
        let legal = format!("{prefix}{}", "a".repeat(MAX_HINT_KEY_BYTES - prefix.len()));
        assert_eq!(legal.len(), MAX_HINT_KEY_BYTES);
        assert!(is_provider_hint_key(&legal));

        let mut oversized = legal;
        oversized.push('a');
        assert!(!is_provider_hint_key(&oversized));
    }

    #[test]
    fn value_limit_is_pinned_at_64_kib() {
        assert_eq!(MAX_HINT_VALUE_BYTES, 64 * 1024);
    }

    #[test]
    fn legacy_map_is_frozen() {
        assert_eq!(
            LEGACY_HINT_KEY_MAP,
            &[
                (
                    "responses.summary_entries",
                    "provider_hints.openai.responses.summary_entries"
                ),
                (
                    "openai.responses.summary_entries",
                    "provider_hints.openai.responses.summary_entries"
                ),
                (
                    "anthropic_block_kind",
                    "provider_hints.anthropic.block_kind"
                ),
            ]
        );
        assert_eq!(
            canonical_hint_key("responses.summary_entries"),
            Some(OPENAI_RESPONSES_SUMMARY_ENTRIES_HINT)
        );
        assert_eq!(
            canonical_hint_key("openai.responses.summary_entries"),
            Some(OPENAI_RESPONSES_SUMMARY_ENTRIES_HINT)
        );
        assert_eq!(
            canonical_hint_key("anthropic_block_kind"),
            Some(ANTHROPIC_BLOCK_KIND_HINT)
        );
        assert_eq!(
            canonical_hint_key(OPENAI_RESPONSES_SUMMARY_ENTRIES_HINT),
            None
        );
        assert_eq!(canonical_hint_key("unknown_legacy_key"), None);
    }
}
