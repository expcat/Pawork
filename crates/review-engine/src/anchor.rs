//! 行锚点解析与 re-anchor：邻近行内容指纹稳定化。
//!
//! 只读 workspace 文件（仅 `fs::read_to_string`），本模块不执行任何写。
//! 路径必须是 workspace 根内的相对路径，拒绝绝对路径与 `..` 逃逸。

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use agent_domain::ReviewAnchor;

use crate::error::ReviewError;

/// 指纹半径：锚点前后各取几行文本拼接后哈希。
pub const FINGERPRINT_RADIUS: usize = 3;
/// re-anchor 搜索窗口：向上下各扩多少行。
pub const REANCHOR_WINDOW: u32 = 200;

/// 锚点解析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedAnchor {
    pub anchor: ReviewAnchor,
    pub line_count: u32,
    /// 锚点上下文指纹；文件不可读时为 `None`（不静默失效，snapshot 标 stale）。
    pub fingerprint: Option<String>,
    pub unavailable: bool,
}

/// re-anchor 结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReanchorOutcome {
    /// 新锚点（漂移时保留原锚点）。
    pub anchor: ReviewAnchor,
    /// `true` 表示未能可靠定位（漂移 / 文件缺失），而非静默失效。
    pub stale: bool,
    pub reason: StaleReason,
}

/// stale 原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaleReason {
    None,
    /// 文件不可读（缺失 / 权限）。
    FileUnavailable,
    /// 锚点行超出当前文件范围。
    LineMissing,
    /// 邻近行内容指纹未匹配到（内容被改动）。
    ContextMoved,
}

impl StaleReason {
    pub fn label(self) -> &'static str {
        match self {
            StaleReason::None => "none",
            StaleReason::FileUnavailable => "file_unavailable",
            StaleReason::LineMissing => "line_missing",
            StaleReason::ContextMoved => "context_moved",
        }
    }
}

/// 计算 1-based `line` 周围窗口（`[line-r, line+r]`）的文本指纹。
///
/// 每行做 `trim_end` 归一化（容忍尾部空白变化）；越界槽位用 `\u{0}` 标记，
/// 使文件边界处的窗口指纹区别于内部窗口。
pub fn fingerprint_context(lines: &[&str], line: u32, radius: usize) -> String {
    let radius = radius as u32;
    let start = line.saturating_sub(radius);
    let end = line.saturating_add(radius);
    let mut buf = String::new();
    for ln in start..=end {
        if ln >= 1 && ln <= lines.len() as u32 {
            buf.push_str(lines[(ln - 1) as usize].trim_end());
        } else {
            buf.push('\u{0}');
        }
        buf.push('\n');
    }
    blake3::hash(buf.as_bytes()).to_hex().to_string()
}

/// 行锚点解析器（只读）。
#[derive(Clone, Debug, Default)]
pub struct AnchorResolver {
    workspace_root: Option<PathBuf>,
}

impl AnchorResolver {
    pub fn new(workspace_root: Option<PathBuf>) -> Self {
        Self { workspace_root }
    }

    /// 校验锚点文件路径：必须为 workspace 内相对路径，拒绝绝对路径与 `..` 逃逸。
    pub fn safe_path(&self, file: &str) -> Result<PathBuf, ReviewError> {
        if file.is_empty() {
            return Err(ReviewError::InvalidAnchor {
                anchor: file.to_string(),
                reason: "路径为空".to_string(),
            });
        }
        let path = Path::new(file);
        let escapes = path.is_absolute()
            || path.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            });
        if escapes {
            return Err(ReviewError::TraversalDenied(file.to_string()));
        }
        match &self.workspace_root {
            Some(root) => Ok(root.join(path)),
            None => Err(ReviewError::InvalidAnchor {
                anchor: file.to_string(),
                reason: "workspace root 未配置，无法解析锚点".to_string(),
            }),
        }
    }

    /// 读取文件（只读）并解析锚点：校验行范围并计算上下文指纹。
    pub fn resolve(&self, anchor: &ReviewAnchor) -> Result<ResolvedAnchor, ReviewError> {
        let path = self.safe_path(&anchor.file)?;
        let text = fs::read_to_string(&path)
            .map_err(|e| ReviewError::FileUnavailable(anchor.file.clone(), e.to_string()))?;
        let lines: Vec<&str> = text.lines().collect();
        let line_count = lines.len() as u32;
        if anchor.line == 0 || anchor.line > line_count {
            return Err(ReviewError::InvalidAnchor {
                anchor: format!("{}:{}", anchor.file, anchor.line),
                reason: format!("行号越界（文件共 {line_count} 行）"),
            });
        }
        if let Some(end) = anchor.end_line {
            if end < anchor.line || end > line_count {
                return Err(ReviewError::InvalidAnchor {
                    anchor: format!("{}:{}..{}", anchor.file, anchor.line, end),
                    reason: format!("end_line 越界（文件共 {line_count} 行）"),
                });
            }
        }
        let fingerprint = fingerprint_context(&lines, anchor.line, FINGERPRINT_RADIUS);
        Ok(ResolvedAnchor {
            anchor: anchor.clone(),
            line_count,
            fingerprint: Some(fingerprint),
            unavailable: false,
        })
    }

    /// 同 [`Self::resolve`]，但文件不可读时降级为「无法校验」而非报错：
    /// 允许对暂缺文件打开 finding，快照层会标 `stale` 而不是静默失效。
    pub fn resolve_optional(&self, anchor: &ReviewAnchor) -> Result<ResolvedAnchor, ReviewError> {
        match self.resolve(anchor) {
            Ok(resolved) => Ok(resolved),
            Err(ReviewError::FileUnavailable(_, _)) => Ok(ResolvedAnchor {
                anchor: anchor.clone(),
                line_count: 0,
                fingerprint: None,
                unavailable: true,
            }),
            Err(e) => Err(e),
        }
    }

    /// 用邻近行内容指纹在编辑后的文件中重新定位锚点。
    ///
    /// 策略：1) 原位指纹一致 → 未漂移；2) 在 ±[`REANCHOR_WINDOW`] 内搜索完整
    /// 在界窗口指纹一致的位置（`end_line` 同步平移）；3) 均失败 → `stale=true`
    /// 并保留原锚点（不静默失效）。
    pub fn reanchor(
        &self,
        original: &ReviewAnchor,
        fingerprint: Option<&str>,
    ) -> Result<ReanchorOutcome, ReviewError> {
        let Some(fp) = fingerprint else {
            return Ok(ReanchorOutcome {
                anchor: original.clone(),
                stale: true,
                reason: StaleReason::FileUnavailable,
            });
        };
        let path = self.safe_path(&original.file)?;
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => {
                return Ok(ReanchorOutcome {
                    anchor: original.clone(),
                    stale: true,
                    reason: StaleReason::FileUnavailable,
                });
            }
        };
        let lines: Vec<&str> = text.lines().collect();
        let line_count = lines.len() as u32;
        if line_count == 0 || original.line > line_count {
            return Ok(ReanchorOutcome {
                anchor: original.clone(),
                stale: true,
                reason: StaleReason::LineMissing,
            });
        }
        // 1) 原位指纹一致：未漂移。
        if fingerprint_context(&lines, original.line, FINGERPRINT_RADIUS) == fp {
            return Ok(ReanchorOutcome {
                anchor: original.clone(),
                stale: false,
                reason: StaleReason::None,
            });
        }
        // 2) 窗口内搜索完整在界窗口（指纹含 OOB 标记，越界窗口不参与匹配）。
        let radius = FINGERPRINT_RADIUS as u32;
        let lo = original
            .line
            .saturating_sub(REANCHOR_WINDOW)
            .max(1 + radius);
        let hi = original
            .line
            .saturating_add(REANCHOR_WINDOW)
            .min(line_count.saturating_sub(radius));
        if lo <= hi {
            for candidate in lo..=hi {
                if candidate == original.line {
                    continue;
                }
                if fingerprint_context(&lines, candidate, FINGERPRINT_RADIUS) == fp {
                    let delta = candidate as i64 - original.line as i64;
                    let mut anchor = original.clone();
                    anchor.line = candidate;
                    if let Some(end) = anchor.end_line {
                        let shifted = end as i64 + delta;
                        anchor.end_line = Some(if shifted < 1 { 1 } else { shifted as u32 });
                    }
                    return Ok(ReanchorOutcome {
                        anchor,
                        stale: false,
                        reason: StaleReason::None,
                    });
                }
            }
        }
        // 3) 漂移：标 stale，保留原锚点。
        Ok(ReanchorOutcome {
            anchor: original.clone(),
            stale: true,
            reason: StaleReason::ContextMoved,
        })
    }
}
