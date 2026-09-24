//! Unix Domain Socket 端点（macOS/Linux）。

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::{ConnectOptions, GuiConnection, GuiListener, TransportError, TransportErrorKind};
use async_trait::async_trait;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

use super::{connection_closed, connection_info, transport_error, StreamConnection};

pub(super) fn bind(
    address: &str,
    max_frame_bytes: u64,
) -> Result<Box<dyn GuiListener>, TransportError> {
    let path = Path::new(address);
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            // 清理上一次进程遗留的 socket 文件。
            std::fs::remove_file(path).map_err(|error| {
                transport_error(
                    TransportErrorKind::BindFailed,
                    format!("failed to remove stale socket {address}: {error}"),
                )
            })?;
        }
        Ok(_) => {
            return Err(transport_error(
                TransportErrorKind::BindFailed,
                format!("path {address} exists and is not a socket file"),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(transport_error(
                TransportErrorKind::BindFailed,
                format!("failed to inspect socket path {address}: {error}"),
            ));
        }
    }
    let listener = UnixListener::bind(path).map_err(|error| {
        transport_error(
            TransportErrorKind::BindFailed,
            format!("failed to bind unix socket {address}: {error}"),
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(
            |error| {
                transport_error(
                    TransportErrorKind::BindFailed,
                    format!("failed to restrict unix socket mode {address}: {error}"),
                )
            },
        )?;
    }
    // R-04：记录本次 bind 创建的 socket 文件身份（dev/ino），close 时据此
    // 判断路径是否仍属于本 listener——fstat(socket fd) 拿不到文件 inode，
    // 必须存 bind 时的 lstat 结果。
    let bound_file_id = std::fs::symlink_metadata(path)
        .map(|metadata| {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        })
        .map_err(|error| {
            transport_error(
                TransportErrorKind::BindFailed,
                format!("failed to stat bound socket {address}: {error}"),
            )
        })?;
    Ok(Box::new(UnixSocketListener {
        path: path.to_path_buf(),
        bound_file_id,
        listener: Mutex::new(Some(listener)),
        max_frame_bytes,
        next_connection_id: AtomicU64::new(0),
        closed: AtomicBool::new(false),
        close_notify: tokio::sync::Notify::new(),
    }))
}

pub(super) async fn connect(
    address: &str,
    options: &ConnectOptions,
) -> Result<Box<dyn GuiConnection>, TransportError> {
    let max_frame_bytes = options.max_frame_bytes;
    let stream = tokio::time::timeout(
        std::time::Duration::from_millis(options.timeout_ms),
        UnixStream::connect(Path::new(address)),
    )
    .await
    .map_err(|_| {
        transport_error(
            TransportErrorKind::Timeout,
            format!("connect to unix socket {address} timed out"),
        )
    })?
    .map_err(|error| {
        transport_error(
            TransportErrorKind::ConnectionFailed,
            format!("failed to connect to unix socket {address}: {error}"),
        )
    })?;
    let (reader, writer) = tokio::io::split(stream);
    let info = connection_info(
        format!(
            "client-{}",
            super::NEXT_CLIENT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
        ),
        max_frame_bytes,
    );
    Ok(Box::new(StreamConnection::new(reader, writer, info)))
}

struct UnixSocketListener {
    path: std::path::PathBuf,
    /// 本次 bind 创建的 socket 文件 (dev, ino)；close 的归属校验基准。
    bound_file_id: (u64, u64),
    listener: Mutex<Option<UnixListener>>,
    max_frame_bytes: u64,
    next_connection_id: AtomicU64,
    closed: AtomicBool,
    /// R-17：close 通知——挂起的 accept 持锁等待时被唤醒并释放锁，
    /// close 因此能在有界时间内拿到锁完成收口。
    close_notify: tokio::sync::Notify,
}

#[async_trait]
impl GuiListener for UnixSocketListener {
    async fn accept(&self) -> Result<Box<dyn GuiConnection>, TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(connection_closed("listener is closed"));
        }
        let guard = self.listener.lock().await;
        // 排队的 accept 可能已通过入口检查；close 唤醒前一等待者后，
        // 后续等待者不得再次占锁等待连接，阻塞 close 收口。
        if self.closed.load(Ordering::Acquire) {
            return Err(connection_closed("listener is closed"));
        }
        let listener = guard
            .as_ref()
            .ok_or_else(|| connection_closed("listener is closed"))?;
        // R-17：accept 与 close 通知 select——持锁等待期间 close 到达即放弃
        // 本次 accept 并释放锁，close 得以完成（原先 close 与 pending accept
        // 等同一把锁，无客户端时并发 close 无法结束）。select 收进内层块：
        // 两个 future 均在块尾 drop、解除对 guard 的借用后才允许 move guard。
        let accepted = {
            let accept = listener.accept();
            tokio::pin!(accept);
            let notified = self.close_notify.notified();
            tokio::pin!(notified);
            tokio::select! {
                biased;
                result = &mut accept => Some(result.map_err(|error| {
                    transport_error(
                        TransportErrorKind::ConnectionFailed,
                        format!("accept failed: {error}"),
                    )
                })?),
                () = &mut notified => None,
            }
        };
        drop(guard);
        let (stream, _peer_address) = match accepted {
            Some(pair) => pair,
            None => return Err(connection_closed("listener is closed")),
        };
        let (reader, writer) = tokio::io::split(stream);
        let info = connection_info(
            format!(
                "connection-{}",
                self.next_connection_id.fetch_add(1, Ordering::Relaxed)
            ),
            self.max_frame_bytes,
        );
        Ok(Box::new(StreamConnection::new(reader, writer, info)))
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        // R-17：先唤醒可能持锁挂起的 accept，再等锁收口——accept 侧
        // select 到通知即放锁，此处等锁有界。
        self.close_notify.notify_one();
        let mut guard = self.listener.lock().await;
        if guard.is_none() {
            // 已关闭：幂等早退。
            return Ok(());
        }
        guard.take(); // drop 监听器，停止接受新连接
        drop(guard);
        // R-04：仅当 socket 路径仍是本 listener bind 时创建的那个文件
        // （dev/ino 一致）才删除——路径可能已被新所有者重新绑定，旧
        // listener 的 close 不得删除赢家端点；无法证明归属宁可留下文件。
        let owned = std::fs::symlink_metadata(&self.path)
            .map(|metadata| {
                use std::os::unix::fs::MetadataExt;
                (metadata.dev(), metadata.ino()) == self.bound_file_id
            })
            .unwrap_or(false);
        if !owned {
            return Ok(());
        }
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(transport_error(
                TransportErrorKind::Internal,
                format!(
                    "failed to remove socket file {}: {error}",
                    self.path.display()
                ),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        ConnectOptions, GuiTransportClient, GuiTransportServer, TransportEndpoint,
        TransportErrorKind, TransportFrame,
    };
    use crate::{LocalTransport, DEFAULT_MAX_FRAME_BYTES};

    fn options(max_frame_bytes: u64) -> ConnectOptions {
        ConnectOptions {
            timeout_ms: 5_000,
            client_label: None,
            max_frame_bytes,
        }
    }

    fn local_endpoint(path: &std::path::Path) -> TransportEndpoint {
        TransportEndpoint::Local {
            address: path.to_string_lossy().into_owned(),
        }
    }

    #[tokio::test]
    async fn unix_socket_is_owner_only_after_bind() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("gui.sock");
        let endpoint = local_endpoint(&path);
        let server = LocalTransport::default();
        let listener = server.bind(endpoint).await.expect("bind");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "bound socket must be owner-only");
        listener.close().await.expect("close");
    }

    #[tokio::test]
    async fn pending_accept_is_woken_by_close() {
        // R-17：无客户端时挂起的 accept 必须被并发 close 唤醒，两者都在
        // 有界时间内结束；重复 close 幂等。
        let temp = tempfile::tempdir().expect("tempdir");
        let endpoint = local_endpoint(&temp.path().join("gui.sock"));
        let server = LocalTransport::default();
        let listener: std::sync::Arc<dyn crate::GuiListener> =
            std::sync::Arc::from(server.bind(endpoint).await.expect("bind"));

        let accept = tokio::spawn({
            let listener = std::sync::Arc::clone(&listener);
            async move { listener.accept().await }
        });
        tokio::task::yield_now().await;
        let queued_accept = tokio::spawn({
            let listener = std::sync::Arc::clone(&listener);
            async move { listener.accept().await }
        });
        tokio::task::yield_now().await;
        let closer = tokio::spawn({
            let listener = std::sync::Arc::clone(&listener);
            async move { listener.close().await }
        });
        let (accept_result, queued_result, close_result) =
            tokio::time::timeout(std::time::Duration::from_secs(5), async move {
                tokio::join!(accept, queued_accept, closer)
            })
            .await
            .expect("pending accept 与 close 必须在有界时间内结束");
        let accept_error = match accept_result.expect("accept task panicked") {
            Err(error) => error,
            Ok(_) => panic!("pending accept 不应成功"),
        };
        assert_eq!(
            accept_error.kind,
            TransportErrorKind::ConnectionClosed,
            "被 close 唤醒的 accept 应报 ConnectionClosed"
        );
        assert!(
            matches!(queued_result.expect("queued accept task panicked"),
            Err(error) if error.kind == TransportErrorKind::ConnectionClosed)
        );
        close_result
            .expect("close task panicked")
            .expect("close ok");
        listener
            .close()
            .await
            .expect("repeated close is idempotent");
    }

    #[tokio::test]
    async fn unix_socket_frame_round_trip() {
        let temp = tempfile::tempdir().expect("tempdir");
        let endpoint = local_endpoint(&temp.path().join("gui.sock"));
        let server = LocalTransport::default();
        let client = LocalTransport::default();
        let listener = server.bind(endpoint.clone()).await.expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let client_conn = client
            .connect(endpoint.clone(), options(DEFAULT_MAX_FRAME_BYTES))
            .await
            .expect("connect");
        let server_conn = accept.await.expect("accept task").expect("accept");

        let payload = b"hello over unix socket".to_vec();
        client_conn
            .send(TransportFrame::new(payload.clone()))
            .await
            .expect("client send");
        assert_eq!(
            server_conn
                .receive()
                .await
                .expect("server receive")
                .as_bytes(),
            &payload
        );

        server_conn
            .send(TransportFrame::new(vec![1, 2, 3]))
            .await
            .expect("server send");
        assert_eq!(
            client_conn
                .receive()
                .await
                .expect("client receive")
                .as_bytes(),
            &[1, 2, 3]
        );

        client_conn.close().await.expect("client close");
        server_conn.close().await.expect("server close");
    }

    #[tokio::test]
    async fn oversized_send_is_rejected_before_writing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let endpoint = local_endpoint(&temp.path().join("gui.sock"));
        let server = LocalTransport::new(64);
        let client = LocalTransport::new(64);
        let listener = server.bind(endpoint.clone()).await.expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let client_conn = client
            .connect(endpoint, options(64))
            .await
            .expect("connect");
        let server_conn = accept.await.expect("accept task").expect("accept");

        let error = client_conn
            .send(TransportFrame::new(vec![0u8; 100]))
            .await
            .expect_err("oversized frame must be rejected");
        assert_eq!(error.kind, TransportErrorKind::FrameTooLarge);

        server_conn.close().await.expect("server close");
        client_conn.close().await.expect("client close");
    }

    #[tokio::test]
    async fn oversized_declared_length_is_rejected_before_allocation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let endpoint = local_endpoint(&temp.path().join("gui.sock"));
        let server = LocalTransport::default();
        let listener = server.bind(endpoint.clone()).await.expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });

        // 用原始 std socket 直接写帧头，绕过客户端 send 侧的校验。
        let mut raw = std::os::unix::net::UnixStream::connect(temp.path().join("gui.sock"))
            .expect("raw connect");
        let server_conn = accept.await.expect("accept task").expect("accept");
        let declared = (DEFAULT_MAX_FRAME_BYTES + 1) as u32;
        std::io::Write::write_all(&mut raw, &declared.to_le_bytes()).expect("write header");

        let error = server_conn.receive().await.expect_err("must reject");
        assert_eq!(error.kind, TransportErrorKind::FrameTooLarge);
        drop(raw);
        server_conn.close().await.expect("server close");
    }

    #[tokio::test]
    async fn peer_close_returns_connection_closed_and_listener_close_stops_accepts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let endpoint = local_endpoint(&temp.path().join("gui.sock"));
        let server = LocalTransport::default();
        let client = LocalTransport::default();
        let listener = server.bind(endpoint.clone()).await.expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let client_conn = client
            .connect(endpoint, options(DEFAULT_MAX_FRAME_BYTES))
            .await
            .expect("connect");
        let server_conn = accept.await.expect("accept task").expect("accept");

        client_conn.close().await.expect("client close");
        let error = server_conn.receive().await.expect_err("peer closed");
        assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);

        let listener = server
            .bind(local_endpoint(&temp.path().join("gui2.sock")))
            .await
            .expect("rebind");
        listener.close().await.expect("listener close");
        let error = match listener.accept().await {
            Err(error) => error,
            Ok(_) => panic!("closed listener"),
        };
        assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
    }

    /// R-04：路径被新所有者重新绑定后，旧 listener 的 close 不得删除
    /// 新端点（dev/ino 归属比对），且重复 close 幂等。
    #[tokio::test]
    async fn close_does_not_remove_socket_rebound_by_new_owner() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("gui.sock");
        let endpoint = local_endpoint(&path);
        let server = LocalTransport::default();
        let client = LocalTransport::default();
        let old = server.bind(endpoint.clone()).await.expect("bind old");

        // 模拟新所有者抢占同一路径（旧文件的 inode 已与新端点不同）。
        std::fs::remove_file(&path).expect("unlink path");
        let new = server.bind(endpoint.clone()).await.expect("bind new");

        old.close().await.expect("old close must succeed");
        assert!(
            std::fs::symlink_metadata(&path).is_ok(),
            "old close must not remove the new owners socket"
        );

        // 新端点仍可接受连接（connect 在内核 backlog 即完成，accept 随后取出）。
        let client_conn = client
            .connect(endpoint, options(DEFAULT_MAX_FRAME_BYTES))
            .await
            .expect("connect to new owner");
        let server_conn = new.accept().await.expect("accept");
        server_conn.close().await.expect("server close");
        client_conn.close().await.expect("client close");

        // 正常路径：新所有者 close 自己的端点，文件被删除；重复 close 幂等。
        new.close().await.expect("new close");
        assert!(
            std::fs::symlink_metadata(&path).is_err(),
            "owner close must remove its own socket file"
        );
        new.close().await.expect("second close is a no-op");
    }
}
