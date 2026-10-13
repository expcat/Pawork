//! 进程级唯一 id。同一进程里的多份 `AppCore` 共用这一份计数器。

use std::sync::atomic::{AtomicU64, Ordering};

/// `{prefix}-{pid:x}-{nanos:x}-{n}`。
///
/// 实例计数器都从 1 起，同一时钟嘀嗒里纳秒也可能相同，主键和账本去重键
/// 不能靠时钟分辨率兜底。pid 区分进程，static 计数器区分同进程调用。
pub(crate) fn process_unique(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "{prefix}-{:x}-{nanos:x}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
