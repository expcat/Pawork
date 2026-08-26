//! UI fixture barrier 发射器（R1 Wave B / W4）。
//!
//! 仅当启动时设置了 `PAWORK_UI_BARRIER_DIR` 才启用；None 状态下所有方法
//! 直通返回（零 IO、零开销）。projection 保持纯状态机，本模块是 Desktop
//! 侧唯一的 barrier 文件写点（语义冻结见 Wave B brief §6：文件名即合同，
//! 内容 JSON 附 at_ms / detail）。

use std::path::{Path, PathBuf};

/// barrier 目录状态：`dir` 为 None 表示未启用（全程零开销直通）。
pub(crate) struct BarrierSink {
    dir: Option<PathBuf>,
    settle_seq: u64,
}

impl BarrierSink {
    pub(crate) fn new(dir: Option<PathBuf>) -> Self {
        // 外部 driver 可能指向尚不存在的目录：惰性建目录一次，
        // 避免写入全部静默丢弃、driver 空等 barrier。失败仍静默。
        if let Some(dir) = dir.as_deref() {
            let _ = std::fs::create_dir_all(dir);
        }
        Self { dir, settle_seq: 0 }
    }

    /// 是否启用（决定 ui/mod.rs 的 1s tick 是否需要常驻）。
    pub(crate) fn is_active(&self) -> bool {
        self.dir.is_some()
    }

    /// 重写 `timeline_stable`：settle_seq 单调自增，读侧据此区分新一轮
    /// 静默（每满足条件的 tick 都重写，内容 / mtime 兼作活性心跳）。
    pub(crate) fn write_timeline_stable(&mut self, session_id: &str, entry_count: usize) {
        let Some(dir) = self.dir.as_deref() else {
            return;
        };
        self.settle_seq += 1;
        let payload = serde_json::json!({
            "settle_seq": self.settle_seq,
            "session_id": session_id,
            "entry_count": entry_count,
            "at_ms": super::now_unix_ms(),
            "detail": "timeline settled",
        });
        write_barrier(dir, "timeline_stable", &payload);
    }

    /// 新连接 / 新会话开始加载时移除上一轮稳定信号，避免只等待文件存在的
    /// driver 把陈旧 barrier 误判为本轮已经 settle。
    pub(crate) fn remove_timeline_stable(&self) {
        let Some(dir) = self.dir.as_deref() else {
            return;
        };
        let _ = std::fs::remove_file(dir.join("timeline_stable"));
    }

    /// 写 `approval_visible`（含 tool 名）；审批卡消失由调用侧触发删除。
    pub(crate) fn write_approval_visible(&self, tool_name: &str, run_id: &str) {
        let Some(dir) = self.dir.as_deref() else {
            return;
        };
        let payload = serde_json::json!({
            "tool": tool_name,
            "run_id": run_id,
            "at_ms": super::now_unix_ms(),
            "detail": "pending approval visible",
        });
        write_barrier(dir, "approval_visible", &payload);
    }

    /// 删除 `approval_visible`（路径仅由本模块在 barrier 目录内拼出，
    /// 满足「消失 → 删除、限 barrier dir 内」的合同）。
    pub(crate) fn remove_approval_visible(&self) {
        let Some(dir) = self.dir.as_deref() else {
            return;
        };
        let _ = std::fs::remove_file(dir.join("approval_visible"));
    }
}

/// 写 barrier 文件：tmp + rename（同目录原子替换，读侧轮询不会读到半截）。
/// 目录由 BarrierSink::new 惰性创建（fixture seed 也会预建）；写失败静默
/// 跳过——barrier 是测试辅助信号，任何 IO 失败都不影响 UI 主路径。
fn write_barrier(dir: &Path, name: &str, payload: &serde_json::Value) {
    let Ok(bytes) = serde_json::to_vec(payload) else {
        return;
    };
    let tmp = dir.join(format!("{name}.tmp"));
    let path = dir.join(name);
    if std::fs::write(&tmp, &bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W4 合同：timeline_stable 逐次重写且 settle_seq 单调自增、字段齐全；
    /// approval_visible 写入含 tool 名、消失即删除；未启用（None）零写入。
    #[test]
    fn barrier_sink_writes_and_removes_contract_files() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().to_path_buf();
        let mut sink = BarrierSink::new(Some(dir.clone()));

        sink.write_timeline_stable("sess-fixture", 50);
        let first: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("timeline_stable")).expect("timeline_stable written"),
        )
        .expect("timeline_stable json");
        assert_eq!(first["settle_seq"].as_u64(), Some(1));
        assert_eq!(first["session_id"].as_str(), Some("sess-fixture"));
        assert_eq!(first["entry_count"].as_u64(), Some(50));
        assert!(first["at_ms"].as_u64().is_some());
        assert!(first["detail"].as_str().is_some());
        assert!(
            !dir.join("timeline_stable.tmp").exists(),
            "tmp must be renamed away"
        );

        sink.write_timeline_stable("sess-fixture", 52);
        let second: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("timeline_stable"))
                .expect("timeline_stable rewritten"),
        )
        .expect("timeline_stable json");
        assert_eq!(
            second["settle_seq"].as_u64(),
            Some(2),
            "settle_seq must be monotonic"
        );
        assert_eq!(second["entry_count"].as_u64(), Some(52));

        sink.remove_timeline_stable();
        assert!(
            !dir.join("timeline_stable").exists(),
            "new load must invalidate stale timeline_stable"
        );

        sink.write_approval_visible("write_file", "run-1");
        let approval: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("approval_visible"))
                .expect("approval_visible written"),
        )
        .expect("approval_visible json");
        assert_eq!(approval["tool"].as_str(), Some("write_file"));
        assert_eq!(approval["run_id"].as_str(), Some("run-1"));
        assert!(approval["at_ms"].as_u64().is_some());

        sink.remove_approval_visible();
        assert!(
            !dir.join("approval_visible").exists(),
            "approval_visible must be removed on disappearance"
        );

        let mut inactive = BarrierSink::new(None);
        assert!(!inactive.is_active());
        inactive.write_timeline_stable("sess", 1);
        inactive.remove_timeline_stable();
        inactive.write_approval_visible("tool", "run");
        inactive.remove_approval_visible();
        assert!(!dir.join("approval_visible").exists());

        // 目录不存在时由 new 惰性创建，写入不静默丢失（P3 加固）。
        let nested = dir.join("not-yet-created/nested");
        let mut late = BarrierSink::new(Some(nested.clone()));
        late.write_timeline_stable("sess-fixture", 1);
        assert!(
            nested.join("timeline_stable").exists(),
            "timeline_stable must be written after lazy create_dir_all"
        );
    }
}
