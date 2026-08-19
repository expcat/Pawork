//! 极简 YAML frontmatter 解析（只取顶层标量键值，绝不解释嵌套结构）。
//!
//! 外部 Skill / Agent / Cursor rule 的 frontmatter 只用于提取 name、
//! description、tools、model、alwaysApply、globs 等已知标量；其余键仅
//! 记录键名（不复制值），嵌套结构视为无法映射。

use std::collections::BTreeMap;

/// frontmatter 解析结果：标量键值 + 剩余正文 + 无法映射的键名。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Frontmatter {
    pub scalars: BTreeMap<String, String>,
    /// 非标量（嵌套/列表）键名，值不回显。
    pub complex_keys: Vec<String>,
    /// frontmatter 之后的正文字段。
    pub body: String,
}

/// 解析以 --- 开头的 YAML frontmatter；无 frontmatter 时原样返回正文。
/// 只处理首行 fence 与首个闭合 fence 之间的顶层标量行。
pub(crate) fn split_frontmatter(text: &str) -> Frontmatter {
    let mut result = Frontmatter::default();
    let Some(after_open) = text.strip_prefix("---") else {
        result.body = text.to_string();
        return result;
    };
    let newline = char::from(10);
    let carriage = char::from(13);
    let inner = if after_open.as_bytes().first() == Some(&10) {
        &after_open[1..]
    } else if let Some(rest) = after_open.strip_prefix("\r\n") {
        rest
    } else {
        result.body = text.to_string();
        return result;
    };
    let mut offset = 0usize;
    let mut closed = false;
    for part in inner.split(newline) {
        if part.trim_end_matches(carriage) == "---" {
            closed = true;
            break;
        }
        offset += part.len() + 1;
    }
    if !closed {
        result.body = text.to_string();
        return result;
    }
    let raw = &inner[..offset];
    let remainder = &inner[offset..];
    let body = remainder
        .split_once(newline)
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_default();
    parse_scalars(raw, &mut result);
    result.body = body;
    result
}

fn parse_scalars(raw: &str, result: &mut Frontmatter) {
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((key, value)) = split_key_value(line) else {
            // 嵌套或列表行：记录父键名（不复制值）。
            let key = line
                .trim_start()
                .trim_start_matches("-")
                .trim()
                .split(":")
                .next()
                .unwrap_or("")
                .trim();
            if !key.is_empty() && !result.complex_keys.iter().any(|item| item == key) {
                result.complex_keys.push(key.to_string());
            }
            continue;
        };
        if value.trim().is_empty() {
            if !result.complex_keys.iter().any(|item| item == &key) {
                result.complex_keys.push(key);
            }
        } else {
            result.scalars.insert(key, unquote(value.trim()));
        }
    }
}

fn split_key_value(line: &str) -> Option<(String, String)> {
    let index = line.find(":")?;
    let key = line[..index].trim();
    if key.is_empty() {
        return None;
    }
    let value = line[index + 1..].trim();
    Some((key.to_string(), value.to_string()))
}

/// 去掉成对的单/双引号（34 = 双引号，39 = 单引号字节）。
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == 34 && last == 34) || (first == 39 && last == 39) {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalars_and_body() {
        let text =
            "---\nname: doc\ndescription: \"write docs\"\nallowed-tools: Bash, Read\n---\n# body\n";
        let parsed = split_frontmatter(text);
        assert_eq!(parsed.scalars.get("name").map(String::as_str), Some("doc"));
        assert_eq!(
            parsed.scalars.get("description").map(String::as_str),
            Some("write docs")
        );
        assert_eq!(parsed.body, "# body\n");
    }

    #[test]
    fn missing_fence_returns_whole_text() {
        let text = "# no frontmatter\n";
        let parsed = split_frontmatter(text);
        assert_eq!(parsed.body, text);
        assert!(parsed.scalars.is_empty());
    }
}
