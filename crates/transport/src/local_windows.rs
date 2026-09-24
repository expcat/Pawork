//! Named Pipe 端点（Windows）。

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::{ConnectOptions, GuiConnection, GuiListener, TransportError, TransportErrorKind};
use async_trait::async_trait;
use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

use super::{connection_closed, connection_info, transport_error, StreamConnection};

/// 管道名前缀；地址经转义后拼接，避免直接拼接导致路径穿越。
const PIPE_PREFIX: &str = r"\\.\pipe\";

/// Named Pipe 路径上限（`\\.\pipe\` 之后最多 256 字符）。
const MAX_PIPE_NAME_BYTES: usize = 256;

fn pipe_path(address: &str) -> Result<String, TransportError> {
    if address.is_empty() || address.len() > MAX_PIPE_NAME_BYTES || address.contains('\0') {
        return Err(transport_error(
            TransportErrorKind::InvalidEndpoint,
            format!("invalid named pipe address {address:?}"),
        ));
    }
    Ok(format!("{PIPE_PREFIX}{address}"))
}

pub(super) fn bind(
    address: &str,
    max_frame_bytes: u64,
) -> Result<Box<dyn GuiListener>, TransportError> {
    let path = pipe_path(address)?;
    Ok(Box::new(NamedPipeListener {
        path,
        max_frame_bytes,
        first_instance: AtomicU32::new(0),
        next_connection_id: AtomicU64::new(0),
        closed: AtomicBool::new(false),
        close_watch: tokio::sync::watch::channel(false).0,
    }))
}

pub(super) async fn connect(
    address: &str,
    options: &ConnectOptions,
) -> Result<Box<dyn GuiConnection>, TransportError> {
    let path = pipe_path(address)?;
    let max_frame_bytes = options.max_frame_bytes;
    // Named Pipe 实例由服务器在 accept 时创建：客户端在服务器尚未创建
    // 首实例（ERROR_FILE_NOT_FOUND）或全部实例忙（ERROR_PIPE_BUSY）时
    // 需重试，直到 timeout_ms 到期。
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(options.timeout_ms);
    let client = loop {
        match ClientOptions::new().open(&path) {
            Ok(client) => break client,
            Err(error) => {
                let retryable = matches!(error.raw_os_error(), Some(2 | 231));
                if !retryable || tokio::time::Instant::now() >= deadline {
                    return Err(transport_error(
                        if retryable {
                            TransportErrorKind::Timeout
                        } else {
                            TransportErrorKind::ConnectionFailed
                        },
                        format!("failed to connect to named pipe {path}: {error}"),
                    ));
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    };
    let (reader, writer) = tokio::io::split(client);
    let info = connection_info(
        format!(
            "client-{}",
            super::NEXT_CLIENT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
        ),
        max_frame_bytes,
    );
    Ok(Box::new(StreamConnection::new(reader, writer, info)))
}

/// Owner-only DACL (`D:P(A;;GA;;;OW)`) via SDDL, then
/// [`ServerOptions::create_with_security_attributes_raw`].
fn create_owner_only_pipe(
    path: &str,
    first: bool,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    #[repr(C)]
    struct SecurityAttributes {
        n_length: u32,
        lp_security_descriptor: *mut c_void,
        inherit_handle: i32,
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            string_security_descriptor: *const u16,
            string_sd_revision: u32,
            security_descriptor: *mut *mut c_void,
            security_descriptor_size: *mut u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(mem: *mut c_void) -> *mut c_void;
    }

    const SDDL_REVISION_1: u32 = 1;
    // Protected DACL: Generic All for the creating owner only.
    let sddl: Vec<u16> = std::ffi::OsStr::new("D:P(A;;GA;;;OW)")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut descriptor: *mut c_void = ptr::null_mut();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    };
    if converted == 0 || descriptor.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let mut attrs = SecurityAttributes {
        n_length: std::mem::size_of::<SecurityAttributes>() as u32,
        lp_security_descriptor: descriptor,
        inherit_handle: 0,
    };
    let created = unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .create_with_security_attributes_raw(path, (&raw mut attrs).cast::<c_void>())
    };
    unsafe {
        LocalFree(descriptor);
    }
    created
}

struct NamedPipeListener {
    path: String,
    max_frame_bytes: u64,
    /// 0 号实例使用 `first_pipe_instance(true)`，之后全部为 false；
    /// Named Pipe 每连接一个实例，支持并发 accept。
    first_instance: AtomicU32,
    next_connection_id: AtomicU64,
    closed: AtomicBool,
    /// R-17：close watch——挂起的 connect 经 select 被唤醒；watch 保留当前值，
    /// 即使通知先于等待注册也不丢（并发 accept 场景 Notify 会漏唤醒）。
    close_watch: tokio::sync::watch::Sender<bool>,
}

#[async_trait]
impl GuiListener for NamedPipeListener {
    async fn accept(&self) -> Result<Box<dyn GuiConnection>, TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(connection_closed("listener is closed"));
        }
        let first = self.first_instance.fetch_add(1, Ordering::Relaxed) == 0;
        let server = create_owner_only_pipe(&self.path, first).map_err(|error| {
            transport_error(
                TransportErrorKind::BindFailed,
                format!(
                    "failed to create named pipe instance {}: {error}",
                    self.path
                ),
            )
        })?;
        // R-17：connect 与 close select——挂起的 connect 在 close 到达时被
        // 唤醒（原先 close 只置标记，已挂起的 connect 无法退出）。
        let connect = server.connect();
        tokio::pin!(connect);
        let mut closed_rx = self.close_watch.subscribe();
        if *closed_rx.borrow() {
            return Err(connection_closed("listener is closed"));
        }
        tokio::select! {
            biased;
            result = &mut connect => result,
            _ = closed_rx.changed() => {
                return Err(connection_closed("listener is closed"));
            }
        }
        .map_err(|error| {
            transport_error(
                TransportErrorKind::ConnectionFailed,
                format!("named pipe client disconnected during connect: {error}"),
            )
        })?;
        let (reader, writer) = tokio::io::split(server);
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
        // R-17：唤醒所有挂起的 accept（Named Pipe 支持并发 accept）；
        // watch 值留存，晚到的 accept 经入口 closed 检查与 borrow 双保险拒绝。
        self.close_watch.send_replace(true);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn unique_address() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        format!(
            "pawork-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[tokio::test]
    async fn named_pipe_frame_round_trip() {
        let address = unique_address();
        let server = LocalTransport::default();
        let client = LocalTransport::default();
        let listener = server
            .bind(TransportEndpoint::Local {
                address: address.clone(),
            })
            .await
            .expect("bind");
        // Named Pipe 实例在 accept 时创建：先启动 accept，再连接。
        let accept = tokio::spawn(async move { listener.accept().await });
        let client_conn = client
            .connect(
                TransportEndpoint::Local {
                    address: address.clone(),
                },
                options(DEFAULT_MAX_FRAME_BYTES),
            )
            .await
            .expect("connect");
        let server_conn = accept.await.expect("accept task").expect("accept");

        let payload = b"hello over named pipe".to_vec();
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
            .send(TransportFrame::new(vec![4, 5, 6]))
            .await
            .expect("server send");
        assert_eq!(
            client_conn
                .receive()
                .await
                .expect("client receive")
                .as_bytes(),
            &[4, 5, 6]
        );

        client_conn.close().await.expect("client close");
        server_conn.close().await.expect("server close");
    }

    #[tokio::test]
    async fn oversized_send_is_rejected_before_writing() {
        let address = unique_address();
        let server = LocalTransport::new(64);
        let client = LocalTransport::new(64);
        let listener = server
            .bind(TransportEndpoint::Local {
                address: address.clone(),
            })
            .await
            .expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let client_conn = client
            .connect(
                TransportEndpoint::Local {
                    address: address.clone(),
                },
                options(64),
            )
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
    async fn listener_close_stops_accepts() {
        let address = unique_address();
        let server = LocalTransport::default();
        let listener = server
            .bind(TransportEndpoint::Local {
                address: address.clone(),
            })
            .await
            .expect("bind");
        listener.close().await.expect("listener close");
        let error = match listener.accept().await {
            Err(error) => error,
            Ok(_) => panic!("accept on closed listener must fail"),
        };
        assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
    }
}
