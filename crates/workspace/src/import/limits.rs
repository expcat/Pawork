//! 探测与解析的资源上限。
//!
//! 兼容加载器只读外部配置，上限用于防止单个损坏目录或超大文件拖垮导入；
//! 超过上限的文件被隔离为诊断，不影响其余来源。

/// 单次 scan 的读取与数量上限。
#[derive(Clone, Copy, Debug)]
pub struct CompatLimits {
    /// 单个文件最大字节数（默认 1 MiB）。
    pub max_file_bytes: u64,
    /// 单类候选文件最大数量（rules / skills / agents …，默认 256）。
    pub max_files_per_kind: usize,
    /// 单次 scan 的候选文件总数（默认 2048）。
    pub max_total_files: usize,
    /// AGENTS.md 层级探测最大目录深度（默认 32）。
    pub max_scan_depth: usize,
    /// 单个目录枚举的最大条目数（硬截断，防止无界 Vec，默认 4096）。
    pub max_dir_entries: usize,
}

impl Default for CompatLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 1024 * 1024,
            max_files_per_kind: 256,
            max_total_files: 2048,
            max_scan_depth: 32,
            max_dir_entries: 4096,
        }
    }
}
