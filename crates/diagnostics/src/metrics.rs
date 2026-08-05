use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MetricName {
    CoreInitMillis,
    DatabaseOperationMillis,
    ProviderFirstTokenMillis,
    ProviderTotalMillis,
    ToolExecutionMillis,
    ContextTokens,
    CompactionTotal,
    SessionOpenMillis,
    DiffGenerationMillis,
    FileIndexMillis,
    MemoryBytes,
    ActiveTasks,
    ChannelBacklog,
    BlobStoreBytes,
}

impl MetricName {
    pub const ALL: [Self; 14] = [
        Self::CoreInitMillis,
        Self::DatabaseOperationMillis,
        Self::ProviderFirstTokenMillis,
        Self::ProviderTotalMillis,
        Self::ToolExecutionMillis,
        Self::ContextTokens,
        Self::CompactionTotal,
        Self::SessionOpenMillis,
        Self::DiffGenerationMillis,
        Self::FileIndexMillis,
        Self::MemoryBytes,
        Self::ActiveTasks,
        Self::ChannelBacklog,
        Self::BlobStoreBytes,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CoreInitMillis => "core_init_millis",
            Self::DatabaseOperationMillis => "database_operation_millis",
            Self::ProviderFirstTokenMillis => "provider_first_token_millis",
            Self::ProviderTotalMillis => "provider_total_millis",
            Self::ToolExecutionMillis => "tool_execution_millis",
            Self::ContextTokens => "context_tokens",
            Self::CompactionTotal => "compaction_total",
            Self::SessionOpenMillis => "session_open_millis",
            Self::DiffGenerationMillis => "diff_generation_millis",
            Self::FileIndexMillis => "file_index_millis",
            Self::MemoryBytes => "memory_bytes",
            Self::ActiveTasks => "active_tasks",
            Self::ChannelBacklog => "channel_backlog",
            Self::BlobStoreBytes => "blob_store_bytes",
        }
    }

    const fn kind(self) -> MetricKind {
        match self {
            Self::CompactionTotal => MetricKind::Counter,
            Self::ContextTokens
            | Self::MemoryBytes
            | Self::ActiveTasks
            | Self::ChannelBacklog
            | Self::BlobStoreBytes => MetricKind::Gauge,
            _ => MetricKind::Histogram,
        }
    }
}

#[derive(Clone, Copy)]
enum MetricKind {
    Counter,
    Gauge,
    Histogram,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HistogramSnapshot {
    pub count: u64,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MetricSnapshot {
    pub counters: BTreeMap<String, u64>,
    pub gauges: BTreeMap<String, f64>,
    pub histograms: BTreeMap<String, HistogramSnapshot>,
}

#[derive(Default)]
struct MetricState {
    counters: BTreeMap<String, u64>,
    gauges: BTreeMap<String, f64>,
    histograms: BTreeMap<String, HistogramSnapshot>,
}

#[derive(Clone)]
pub struct Metrics {
    state: Arc<Mutex<MetricState>>,
}

impl Default for Metrics {
    fn default() -> Self {
        let metrics = Self {
            state: Arc::new(Mutex::new(MetricState::default())),
        };
        {
            let mut state = metrics.state();
            for name in MetricName::ALL {
                match name.kind() {
                    MetricKind::Counter => {
                        state.counters.insert(name.as_str().into(), 0);
                    }
                    MetricKind::Gauge => {
                        state.gauges.insert(name.as_str().into(), 0.0);
                    }
                    MetricKind::Histogram => {
                        state
                            .histograms
                            .insert(name.as_str().into(), HistogramSnapshot::default());
                    }
                }
            }
        }
        metrics
    }
}

impl Metrics {
    pub fn increment(&self, name: MetricName, amount: u64) {
        debug_assert!(matches!(name.kind(), MetricKind::Counter));
        let mut state = self.state();
        let counter = state.counters.entry(name.as_str().into()).or_default();
        *counter = counter.saturating_add(amount);
    }

    pub fn set_gauge(&self, name: MetricName, value: f64) {
        debug_assert!(matches!(name.kind(), MetricKind::Gauge));
        self.state().gauges.insert(name.as_str().into(), value);
    }

    pub fn observe(&self, name: MetricName, value: f64) {
        debug_assert!(matches!(name.kind(), MetricKind::Histogram));
        let mut state = self.state();
        let histogram = state.histograms.entry(name.as_str().into()).or_default();
        histogram.count = histogram.count.saturating_add(1);
        histogram.sum += value;
        if histogram.count == 1 {
            histogram.min = value;
            histogram.max = value;
        } else {
            histogram.min = histogram.min.min(value);
            histogram.max = histogram.max.max(value);
        }
    }

    pub fn timer(&self, name: MetricName) -> MetricsTimer {
        debug_assert!(matches!(name.kind(), MetricKind::Histogram));
        MetricsTimer {
            metrics: self.clone(),
            name,
            started_at: Instant::now(),
        }
    }

    pub fn snapshot(&self) -> MetricSnapshot {
        let state = self.state();
        MetricSnapshot {
            counters: state.counters.clone(),
            gauges: state.gauges.clone(),
            histograms: state.histograms.clone(),
        }
    }

    fn state(&self) -> MutexGuard<'_, MetricState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub struct MetricsTimer {
    metrics: Metrics,
    name: MetricName,
    started_at: Instant,
}

impl Drop for MetricsTimer {
    fn drop(&mut self) {
        let millis = self.started_at.elapsed().as_secs_f64() * 1_000.0;
        self.metrics.observe(self.name, millis);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_the_complete_metric_set_and_collects_values() {
        let metrics = Metrics::default();
        metrics.increment(MetricName::CompactionTotal, 2);
        metrics.set_gauge(MetricName::ContextTokens, 512.0);
        metrics.observe(MetricName::ProviderFirstTokenMillis, 23.0);
        {
            let _timer = metrics.timer(MetricName::ToolExecutionMillis);
        }
        let snapshot = metrics.snapshot();
        let total = snapshot.counters.len() + snapshot.gauges.len() + snapshot.histograms.len();
        assert_eq!(total, MetricName::ALL.len());
        assert_eq!(snapshot.counters["compaction_total"], 2);
        assert_eq!(snapshot.gauges["context_tokens"], 512.0);
        assert_eq!(snapshot.histograms["provider_first_token_millis"].count, 1);
        assert_eq!(snapshot.histograms["tool_execution_millis"].count, 1);
    }
}
