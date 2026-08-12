//! 可选的真实文件监听 driver（P16-6）。
//!
//! 把 `notify` 去抖事件归一为 [`crate::Observation::FileChange`]，供
//! [`crate::MonitorService::evaluate`] 消费。本 driver 只负责「外部事件 ->
//! Observation」归一；命中判定与节流由 monitor-service 的确定性核心处理，
//! 保证核心可独立单测。
//!
//! 不启动任何子进程；文件监听基于 OS fs 事件（notify），不触碰 ProcessRuntime。

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify_debouncer_full::{
    new_debouncer,
    notify::{RecommendedWatcher, RecursiveMode},
    Debouncer, RecommendedCache,
};

use crate::config::Observation;

/// 把一组变更路径归一为一条 FileChange 观测（空列表返回 None）。
///
/// 抽出为纯函数，便于独立单测；driver 内部回调调用它。
pub fn paths_to_observation(paths: Vec<String>) -> Option<Observation> {
    if paths.is_empty() {
        None
    } else {
        Some(Observation::FileChange { paths })
    }
}

/// 真实文件监听 driver：持有 notify debouncer，drop 即停止监听。
///
/// 回调把去抖后的变更路径归一为 [`Observation`] 推入内部缓冲；
/// [`FileWatchDriver::try_drain`] 非阻塞取出，喂给 MonitorService::evaluate。
pub struct FileWatchDriver {
    // 持有 debouncer 保活；drop 时停止监听。
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    pending: Arc<Mutex<VecDeque<Observation>>>,
}

impl FileWatchDriver {
    /// 在给定路径上递归监听；`debounce` 为去抖窗口。
    pub fn new<I, P>(paths: I, debounce: Duration) -> Result<Self, String>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let pending: Arc<Mutex<VecDeque<Observation>>> = Arc::new(Mutex::new(VecDeque::new()));
        let pending_for_cb = Arc::clone(&pending);
        let mut debouncer = new_debouncer(
            debounce,
            None,
            move |result: notify_debouncer_full::DebounceEventResult| {
                if let Ok(events) = result {
                    let paths: Vec<String> = events
                        .into_iter()
                        .flat_map(|event| {
                            event
                                .event
                                .paths
                                .into_iter()
                                .map(|p| p.to_string_lossy().into_owned())
                        })
                        .collect();
                    if let Some(observation) = paths_to_observation(paths) {
                        if let Ok(mut buf) = pending_for_cb.lock() {
                            buf.push_back(observation);
                        }
                    }
                }
            },
        )
        .map_err(|err| err.to_string())?;

        for path in paths {
            debouncer
                .watch(path.as_ref(), RecursiveMode::Recursive)
                .map_err(|err| err.to_string())?;
        }

        Ok(Self {
            _debouncer: debouncer,
            pending,
        })
    }

    /// 非阻塞取出已归一的观测样本（按到达顺序）。
    pub fn try_drain(&self) -> Vec<Observation> {
        match self.pending.lock() {
            Ok(mut buf) => buf.drain(..).collect(),
            Err(poisoned) => poisoned.into_inner().drain(..).collect(),
        }
    }

    /// 当前待消费观测数（诊断用）。
    pub fn pending_len(&self) -> usize {
        self.pending
            .lock()
            .map(|buf| buf.len())
            .unwrap_or_else(|poisoned| poisoned.into_inner().len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_to_observation_dedupes_empty() {
        assert_eq!(paths_to_observation(vec![]), None);
        let obs = paths_to_observation(vec!["/a".into(), "/b".into()]).unwrap();
        match obs {
            Observation::FileChange { paths } => {
                assert_eq!(paths, vec!["/a".to_string(), "/b".to_string()])
            }
            other => panic!("expected FileChange, got {other:?}"),
        }
    }

    #[test]
    fn driver_constructs_and_drops_on_tempdir() {
        // 构造 + drop 烟雾测试：保证 notify 接线可编译可运行，不依赖具体 fs 事件时序。
        let dir = tempfile::tempdir().unwrap();
        let driver = FileWatchDriver::new([dir.path()], Duration::from_millis(10));
        assert!(driver.is_ok(), "driver should construct on existing path");
        let driver = driver.unwrap();
        assert_eq!(driver.try_drain().len(), 0);
        assert_eq!(driver.pending_len(), 0);
        drop(driver);
    }
}
