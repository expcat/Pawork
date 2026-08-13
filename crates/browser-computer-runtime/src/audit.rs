//! 版本化、可持久化的审计记录与 durable sink（P17-10 review）。
//!
//! 所有后端选择 / 执行 / hosted 事件都产生 [`BrowserComputerAudit`]，经
//! [`AuditSink`] 持久化（缺省实现 [`FileAuditSink`]：JSONL，逐条带格式版本
//! 与单调序号），可跨重启 replay。审计缺失不得静默：sink 写入失败以
//! `tracing::error` 暴露，内存中最近记录仍保留供进程内查询。
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::policy::BrowserComputerAudit;

/// 审计记录格式版本。读取到不匹配版本时 replay 显式失败（不静默降级）。
pub const AUDIT_FORMAT_VERSION: u32 = 1;

/// 一条已落盘的审计记录：格式版本 + 单调序号 + 审计载荷。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub version: u32,
    pub seq: u64,
    #[serde(flatten)]
    pub audit: BrowserComputerAudit,
}

/// durable audit sink：追加 + 全量 replay。
pub trait AuditSink: Send + Sync {
    /// 追加一条审计记录，返回带版本与序号的落盘记录。
    fn append(&self, audit: &BrowserComputerAudit) -> Result<AuditRecord, AuditSinkError>;
    /// 全量回放（含历史记录；顺序即写入顺序）。
    fn replay(&self) -> Result<Vec<AuditRecord>, AuditSinkError>;
}

/// 审计 sink 错误。
#[derive(Debug, Clone, Error)]
pub enum AuditSinkError {
    #[error("audit sink io error: {0}")]
    Io(String),
    #[error("audit record version {found} unsupported (expected {expected}) at line {line}")]
    UnsupportedVersion {
        line: usize,
        found: u32,
        expected: u32,
    },
    #[error("corrupt audit record at line {line}: {message}")]
    Corrupt { line: usize, message: String },
    #[error("audit seq non-contiguous at line {line}: expected {expected}, found {found}")]
    NonContiguousSeq {
        line: usize,
        expected: u64,
        found: u64,
    },
}

/// 文件型 durable audit sink（JSONL）。
///
/// - 每条记录一行：`{"version":1,"seq":N,...audit 字段}`；
/// - seq 在锁内分配（checked_add），写入并 `sync_all` 后才提交；失败回滚序号，
///   不留洞、不乱序；
/// - replay 与 append 同一把锁；replay 校验 seq 严格连续（1..=N，无洞无重复），
///   打开已有文件时从 replay 结果推导续写序号（跨重启单调递增）。
#[derive(Debug)]
pub struct FileAuditSink {
    path: PathBuf,
    state: Mutex<SinkState>,
}

#[derive(Debug)]
struct SinkState {
    next_seq: u64,
}

impl FileAuditSink {
    /// 打开（必要时创建）审计文件；已有文件会先验证并推导续写序号。
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, AuditSinkError> {
        let path = path.into();
        let sink = Self {
            path,
            state: Mutex::new(SinkState { next_seq: 1 }),
        };
        let records = sink.replay()?;
        if let Some(last) = records.last() {
            let next = last
                .seq
                .checked_add(1)
                .ok_or_else(|| AuditSinkError::Io("audit seq overflow".into()))?;
            sink.state
                .lock()
                .map_err(|_| AuditSinkError::Io("audit sink state lock poisoned".into()))?
                .next_seq = next;
        }
        Ok(sink)
    }

    #[cfg(test)]
    fn open_at(path: impl Into<PathBuf>, next_seq: u64) -> Self {
        Self {
            path: path.into(),
            state: Mutex::new(SinkState { next_seq }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AuditSink for FileAuditSink {
    fn append(&self, audit: &BrowserComputerAudit) -> Result<AuditRecord, AuditSinkError> {
        // seq 在锁内分配：分配 + 写入 + sync 原子成组，避免并发乱序/留洞。
        let mut state = self
            .state
            .lock()
            .map_err(|_| AuditSinkError::Io("audit sink state lock poisoned".into()))?;
        let seq = state.next_seq;
        let next = state
            .next_seq
            .checked_add(1)
            .ok_or_else(|| AuditSinkError::Io("audit seq overflow".into()))?;
        let record = AuditRecord {
            version: AUDIT_FORMAT_VERSION,
            seq,
            audit: audit.clone(),
        };
        let line =
            serde_json::to_string(&record).map_err(|err| AuditSinkError::Io(err.to_string()))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|err| AuditSinkError::Io(err.to_string()))?;
        if let Err(err) = writeln!(file, "{line}").and_then(|_| file.sync_all()) {
            // 写入/sync 失败：回滚序号（seq 不前进），避免后续留洞。
            state.next_seq = seq;
            return Err(AuditSinkError::Io(err.to_string()));
        }
        state.next_seq = next;
        Ok(record)
    }

    fn replay(&self) -> Result<Vec<AuditRecord>, AuditSinkError> {
        // replay 与 append 同锁，保证读到的是已提交的连续序列。
        let _guard = self
            .state
            .lock()
            .map_err(|_| AuditSinkError::Io("audit sink state lock poisoned".into()))?;
        // 首次打开时文件尚不存在 → 视为空审计。
        let content = match fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(AuditSinkError::Io(err.to_string())),
        };
        let mut records = Vec::new();
        let mut expected: u64 = 1;
        for (idx, line) in content.lines().enumerate() {
            let line_no = idx + 1;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let record: AuditRecord =
                serde_json::from_str(trimmed).map_err(|err| AuditSinkError::Corrupt {
                    line: line_no,
                    message: err.to_string(),
                })?;
            if record.version != AUDIT_FORMAT_VERSION {
                return Err(AuditSinkError::UnsupportedVersion {
                    line: line_no,
                    found: record.version,
                    expected: AUDIT_FORMAT_VERSION,
                });
            }
            // 拒绝非连续 seq（洞 / 重复 / 乱序）。
            if record.seq != expected {
                return Err(AuditSinkError::NonContiguousSeq {
                    line: line_no,
                    expected,
                    found: record.seq,
                });
            }
            expected = expected
                .checked_add(1)
                .ok_or_else(|| AuditSinkError::Io("audit seq overflow".into()))?;
            records.push(record);
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::BrowserComputerAudit;

    fn audit(action: &str) -> BrowserComputerAudit {
        BrowserComputerAudit {
            action: action.into(),
            backend: Some("local".into()),
            site: Some("client_function".into()),
            trust: Some("core_owned".into()),
            cross_trust_fallback: false,
            policy: "allow".into(),
            note: "unit".into(),
        }
    }

    #[test]
    fn append_and_replay_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let sink = FileAuditSink::open(&path).unwrap();
        let a = sink.append(&audit("navigate")).unwrap();
        let b = sink.append(&audit("title")).unwrap();
        assert_eq!(a.seq, 1);
        assert_eq!(b.seq, 2);
        assert_eq!(a.version, AUDIT_FORMAT_VERSION);
        let records = sink.replay().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].audit.action, "navigate");
        assert_eq!(records[1].audit.action, "title");
    }

    #[test]
    fn reopen_continues_seq_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let sink = FileAuditSink::open(&path).unwrap();
        sink.append(&audit("navigate")).unwrap();
        drop(sink);
        // 模拟重启：同一路径重新打开。
        let restarted = FileAuditSink::open(&path).unwrap();
        restarted.append(&audit("title")).unwrap();
        let records = restarted.replay().unwrap();
        let seqs: Vec<u64> = records.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![1, 2]);
        assert!(records.iter().all(|r| r.version == AUDIT_FORMAT_VERSION));
    }

    #[test]
    fn unsupported_version_fails_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        fs::write(&path, r#"{"version":99,"seq":1,"action":"x","backend":null,"site":null,"trust":null,"cross_trust_fallback":false,"policy":"allow","note":""}"#)
            .unwrap();
        let sink = FileAuditSink::open(&path).unwrap_err();
        assert!(matches!(
            sink,
            AuditSinkError::UnsupportedVersion { found: 99, .. }
        ));
    }

    fn line(seq: u64) -> String {
        let record = AuditRecord {
            version: AUDIT_FORMAT_VERSION,
            seq,
            audit: audit("navigate"),
        };
        serde_json::to_string(&record).unwrap()
    }

    #[test]
    fn concurrent_appends_are_contiguous_and_ordered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let sink = std::sync::Arc::new(FileAuditSink::open(&path).unwrap());
        let mut handles = Vec::new();
        for _ in 0..4 {
            let sink = sink.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..50 {
                    sink.append(&audit("navigate")).unwrap();
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let records = sink.replay().unwrap();
        let seqs: Vec<u64> = records.iter().map(|r| r.seq).collect();
        // 锁内分配 seq：200 条严格连续、按序落盘，无洞无重复。
        assert_eq!(seqs, (1..=200).collect::<Vec<u64>>());
    }

    #[test]
    fn replay_rejects_gap_in_seq() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        fs::write(&path, format!("{}\n{}\n", line(1), line(3))).unwrap();
        let err = FileAuditSink::open(&path).unwrap_err();
        assert!(matches!(
            err,
            AuditSinkError::NonContiguousSeq {
                expected: 2,
                found: 3,
                ..
            }
        ));
    }

    #[test]
    fn replay_rejects_duplicate_seq() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        fs::write(&path, format!("{}\n{}\n", line(1), line(1))).unwrap();
        let err = FileAuditSink::open(&path).unwrap_err();
        assert!(matches!(
            err,
            AuditSinkError::NonContiguousSeq {
                expected: 2,
                found: 1,
                ..
            }
        ));
    }

    #[test]
    fn seq_overflow_fails_closed_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let sink = FileAuditSink::open_at(&path, u64::MAX);
        let err = sink.append(&audit("navigate")).unwrap_err();
        assert!(matches!(err, AuditSinkError::Io(_)));
        // 未落任何记录（checked_add 拒绝，回滚保持空文件）。
        assert!(sink.replay().unwrap().is_empty());
    }
}
