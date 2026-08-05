//! Pawork 的 SQLite Actor。
//!
//! 每个 [`DatabaseActor`] 在专用线程持有唯一的 `rusqlite::Connection`。异步调用方
//! 只能通过有界命令通道提交闭包，不能取得或共享连接，因此数据库写入天然串行化。

use std::{
    any::Any,
    fs,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

use rusqlite::{backup::Backup, Connection, OpenFlags};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

const DEFAULT_QUEUE_CAPACITY: usize = 128;
const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

type ErasedResult = Result<Box<dyn Any + Send>, DatabaseError>;
type Operation = Box<dyn FnOnce(&mut Connection) -> ErasedResult + Send + 'static>;

enum Command {
    Execute {
        operation: Operation,
        response: oneshot::Sender<ErasedResult>,
    },
    Shutdown {
        response: oneshot::Sender<()>,
    },
}

/// SQLite Actor 的打开选项。
#[derive(Clone, Debug)]
pub struct DatabaseOptions {
    pub path: PathBuf,
    pub read_only: bool,
    pub create_if_missing: bool,
    pub queue_capacity: usize,
    pub busy_timeout: Duration,
}

impl DatabaseOptions {
    pub fn read_write(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            read_only: false,
            create_if_missing: true,
            queue_capacity: DEFAULT_QUEUE_CAPACITY,
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        }
    }

    pub fn read_only(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            read_only: true,
            create_if_missing: false,
            queue_capacity: DEFAULT_QUEUE_CAPACITY,
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        }
    }
}

#[derive(Clone)]
pub struct DatabaseActor {
    inner: Arc<Inner>,
}

struct Inner {
    sender: mpsc::Sender<Command>,
    worker: Mutex<Option<JoinHandle<()>>>,
    path: PathBuf,
    read_only: bool,
}

impl DatabaseActor {
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, DatabaseError> {
        Self::open_with_options(DatabaseOptions::read_write(path)).await
    }

    pub async fn open_read_only(path: impl Into<PathBuf>) -> Result<Self, DatabaseError> {
        Self::open_with_options(DatabaseOptions::read_only(path)).await
    }

    pub async fn open_with_options(options: DatabaseOptions) -> Result<Self, DatabaseError> {
        if options.queue_capacity == 0 {
            return Err(DatabaseError::InvalidQueueCapacity);
        }
        if !options.read_only {
            if let Some(parent) = options.path.parent() {
                fs::create_dir_all(parent)?;
            }
        }

        let (sender, receiver) = mpsc::channel(options.queue_capacity);
        let (ready_tx, ready_rx) = oneshot::channel();
        let thread_options = options.clone();
        let worker = thread::Builder::new()
            .name("pawork-sqlite-actor".into())
            .spawn(move || actor_main(thread_options, receiver, ready_tx))?;

        ready_rx.await.map_err(|_| DatabaseError::ActorClosed)??;
        Ok(Self {
            inner: Arc::new(Inner {
                sender,
                worker: Mutex::new(Some(worker)),
                path: options.path,
                read_only: options.read_only,
            }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn is_read_only(&self) -> bool {
        self.inner.read_only
    }

    /// 在 Actor 唯一连接上执行操作。连接不会离开专用线程。
    pub async fn call<T, F>(&self, operation: F) -> Result<T, DatabaseError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> T + Send + 'static,
    {
        let operation: Operation = Box::new(move |connection| {
            catch_unwind(AssertUnwindSafe(|| operation(connection)))
                .map(|value| Box::new(value) as Box<dyn Any + Send>)
                .map_err(|_| DatabaseError::OperationPanicked)
        });
        let (response_tx, response_rx) = oneshot::channel();
        self.inner
            .sender
            .send(Command::Execute {
                operation,
                response: response_tx,
            })
            .await
            .map_err(|_| DatabaseError::ActorClosed)?;
        let erased = response_rx
            .await
            .map_err(|_| DatabaseError::ActorClosed)??;
        erased
            .downcast::<T>()
            .map(|value| *value)
            .map_err(|_| DatabaseError::ResponseTypeMismatch)
    }

    /// 使用 SQLite online backup API 生成一致性备份。
    pub async fn backup_to(&self, destination: impl Into<PathBuf>) -> Result<(), DatabaseError> {
        let destination = destination.into();
        if destination == self.inner.path {
            return Err(DatabaseError::BackupTargetsSource);
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        self.call(move |connection| -> rusqlite::Result<()> {
            let mut destination_connection = Connection::open(&destination)?;
            let backup = Backup::new(connection, &mut destination_connection)?;
            backup.run_to_completion(64, Duration::from_millis(5), None)
        })
        .await??;
        Ok(())
    }

    /// 从一致性备份恢复当前数据库。只读 Actor 不允许恢复。
    pub async fn restore_from(&self, source: impl Into<PathBuf>) -> Result<(), DatabaseError> {
        if self.inner.read_only {
            return Err(DatabaseError::ReadOnly);
        }
        let source = source.into();
        self.call(move |connection| -> rusqlite::Result<()> {
            let source_connection =
                Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let backup = Backup::new(&source_connection, connection)?;
            backup.run_to_completion(64, Duration::from_millis(5), None)
        })
        .await??;
        Ok(())
    }

    /// 显式关闭 Actor，并等待专用线程释放连接。
    pub async fn shutdown(self) -> Result<(), DatabaseError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.inner
            .sender
            .send(Command::Shutdown {
                response: response_tx,
            })
            .await
            .map_err(|_| DatabaseError::ActorClosed)?;
        response_rx.await.map_err(|_| DatabaseError::ActorClosed)?;
        join_worker(&self.inner);
        Ok(())
    }
}

fn join_worker(inner: &Inner) {
    if let Ok(mut slot) = inner.worker.lock() {
        if let Some(worker) = slot.take() {
            let _ = worker.join();
        }
    }
}

fn actor_main(
    options: DatabaseOptions,
    mut receiver: mpsc::Receiver<Command>,
    ready: oneshot::Sender<Result<(), DatabaseError>>,
) {
    let mut connection = match open_connection(&options) {
        Ok(connection) => {
            let _ = ready.send(Ok(()));
            connection
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };

    while let Some(command) = receiver.blocking_recv() {
        match command {
            Command::Execute {
                operation,
                response,
            } => {
                let _ = response.send(operation(&mut connection));
            }
            Command::Shutdown { response } => {
                drop(connection);
                let _ = response.send(());
                return;
            }
        }
    }
}

fn open_connection(options: &DatabaseOptions) -> Result<Connection, DatabaseError> {
    let flags = if options.read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        if options.create_if_missing {
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        flags
    };
    let connection = Connection::open_with_flags(&options.path, flags)?;
    connection.busy_timeout(options.busy_timeout)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    if !options.read_only {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
    }
    Ok(connection)
}

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database actor is closed")]
    ActorClosed,
    #[error("database operation panicked")]
    OperationPanicked,
    #[error("database actor returned an unexpected response type")]
    ResponseTypeMismatch,
    #[error("database queue capacity must be greater than zero")]
    InvalidQueueCapacity,
    #[error("backup destination must differ from the source database")]
    BackupTargetsSource,
    #[error("operation is not available in read-only recovery mode")]
    ReadOnly,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path(name: &str) -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-app-database-{}-{unique}-{name}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn actor_enables_required_pragmas_and_serializes_calls() {
        let path = temp_path("actor.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open actor");
        let pragmas = actor
            .call(|connection| {
                let journal: String = connection
                    .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                    .expect("journal");
                let foreign_keys: i64 = connection
                    .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
                    .expect("foreign keys");
                (journal, foreign_keys)
            })
            .await
            .expect("query pragmas");
        assert_eq!(pragmas.0.to_ascii_lowercase(), "wal");
        assert_eq!(pragmas.1, 1);

        actor.call(|connection| connection.execute_batch("CREATE TABLE counters(value INTEGER NOT NULL); INSERT INTO counters VALUES (0);")).await.expect("actor").expect("create");
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let actor = actor.clone();
            tasks.push(tokio::spawn(async move {
                actor
                    .call(|connection| {
                        connection.execute("UPDATE counters SET value = value + 1", [])
                    })
                    .await
                    .expect("actor")
                    .expect("update");
            }));
        }
        for task in tasks {
            task.await.expect("task");
        }
        let value: i64 = actor
            .call(|connection| {
                connection.query_row("SELECT value FROM counters", [], |row| row.get(0))
            })
            .await
            .expect("actor")
            .expect("value");
        assert_eq!(value, 16);
        actor.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn backup_restore_and_read_only_recovery_are_consistent() {
        let path = temp_path("source.sqlite3");
        let backup = temp_path("backup.sqlite3");
        let actor = DatabaseActor::open(&path).await.expect("open actor");
        actor.call(|connection| connection.execute_batch("CREATE TABLE values_table(value TEXT); INSERT INTO values_table VALUES ('before');")).await.expect("actor").expect("seed");
        actor.backup_to(&backup).await.expect("backup");
        actor
            .call(|connection| connection.execute("UPDATE values_table SET value='after'", []))
            .await
            .expect("actor")
            .expect("update");
        actor.restore_from(&backup).await.expect("restore");
        let value: String = actor
            .call(|connection| {
                connection.query_row("SELECT value FROM values_table", [], |row| row.get(0))
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(value, "before");
        actor.shutdown().await.expect("shutdown");

        let recovery = DatabaseActor::open_read_only(&path)
            .await
            .expect("read only");
        assert!(recovery.is_read_only());
        let value: String = recovery
            .call(|connection| {
                connection.query_row("SELECT value FROM values_table", [], |row| row.get(0))
            })
            .await
            .expect("actor")
            .expect("query");
        assert_eq!(value, "before");
        assert!(matches!(
            recovery.restore_from(&backup).await,
            Err(DatabaseError::ReadOnly)
        ));
        recovery.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(backup);
    }
}
