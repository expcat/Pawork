//! Pawork 存储：串行 SQLite Actor、Session Event Store 与内容寻址 Blob。
//!
//! 模块按 feature 分层：sqlite 基座常开；session / blob 默认开启；
//! compaction / checkpoint / protected 保持 opt-in。根级不做 re-export。

pub mod sqlite;

#[cfg(feature = "session")]
pub mod session;

#[cfg(feature = "blob")]
pub mod blob;
