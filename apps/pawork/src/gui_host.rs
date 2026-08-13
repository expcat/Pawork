//! `pawork` 的 GUI Server 宿主（P13-4 接线）。
//!
//! 把 [`gui_server::GuiServer`] 装进 [`cli_host::GuiServerHost`] trait：
//! `serve` 模式经 [`ServeGuiHost::start`] 绑定本地端点并跑 accept 循环，
//! [`ServeGuiHost::stop`] 中止循环并关闭监听器。连接会话（握手 → 帧循环）
//! 由 `GuiServerListener::accept` 内部自行 spawn，宿主不做二次派发。
//!
//! P17-11：`remote publish` 后经 [`ServeGuiHost::bind_remote`] 绑定远程端点并
//! 跑 accept 循环；`unpublish` / `revoke` 经 [`ServeGuiHost::close_remote`]
//! 关闭监听器。[`CompositeGuiTransport`] 按端点类型把本地端点路由到
//! `transport-local`、远程端点路由到与 Provider 共享的 `transport-remote`
//! 实例，替换实现不修改 GUI Protocol（[ADR-028]）。

//! [ADR-028]: ../../docs/adr/ADR-028-replaceable-remote-transport.md

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cli_host::GuiServerHost;
use gui_server::GuiServer;
use tokio::task::JoinHandle;
use transport_api::{GuiListener, GuiTransportServer, TransportEndpoint, TransportError};
use transport_remote::RealRemoteTransport;

/// 每个实例的本地 GUI 端点地址：
/// - Unix：`<tempdir>/pawork-<instance>.sock`
/// - Windows：named pipe 名 `pawork-<instance>`（transport-local 负责加 `\\.\pipe\` 前缀）
pub fn endpoint_for(instance: &str) -> TransportEndpoint {
    let name = sanitize_instance(instance);
    #[cfg(unix)]
    {
        let address = std::env::temp_dir()
            .join(format!("pawork-{name}.sock"))
            .to_string_lossy()
            .into_owned();
        TransportEndpoint::Local { address }
    }
    #[cfg(windows)]
    {
        TransportEndpoint::Local {
            address: format!("pawork-{name}"),
        }
    }
}

/// 每实例数据目录（token 等状态落盘位置）：`PAWORK_DATA_DIR` 覆盖；
/// 否则平台默认（Windows `%LOCALAPPDATA%\pawork`，其他 `~/.pawork`），
/// 均不可用时回退到系统临时目录。
pub fn instance_dir(instance: &str) -> PathBuf {
    let base = std::env::var_os("PAWORK_DATA_DIR")
        .map(PathBuf::from)
        .or_else(platform_data_dir)
        .unwrap_or_else(std::env::temp_dir);
    base.join(sanitize_instance(instance))
}

fn sanitize_instance(instance: &str) -> String {
    instance
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn platform_data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("pawork"))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(|dir| PathBuf::from(dir).join(".pawork"))
    }
}

type RemoteListenerTask = (Arc<dyn GuiListener>, JoinHandle<()>);

/// [`GuiServerHost`] 实现：持有共享 [`GuiServer`]，维护 accept 循环任务与
/// 监听器的生命周期。
pub struct ServeGuiHost {
    server: Arc<GuiServer>,
    listener: Mutex<Option<Arc<dyn GuiListener>>>,
    accept_task: Mutex<Option<JoinHandle<()>>>,
    /// handle id → （远程监听器，accept 循环任务）：`remote publish` 后登记，
    /// `unpublish` / `revoke` 时按 handle 关闭。
    remote_listeners: Mutex<HashMap<String, RemoteListenerTask>>,
    active_sessions: Arc<AtomicUsize>,
}

impl ServeGuiHost {
    pub fn new(server: Arc<GuiServer>) -> Self {
        Self {
            server,
            listener: Mutex::new(None),
            accept_task: Mutex::new(None),
            remote_listeners: Mutex::new(HashMap::new()),
            active_sessions: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 当前 accept loops 持有的活跃 GUI 会话句柄数（诊断与生命周期测试）。
    #[allow(dead_code)]
    pub fn active_session_count(&self) -> usize {
        self.active_sessions.load(Ordering::Acquire)
    }
}

struct ActiveSessionGuard(Arc<AtomicUsize>);

impl Drop for ActiveSessionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 一个监听器的 accept 循环：`GuiServerListener::accept` 内部已 spawn 会话任务
/// （握手 → 帧循环），这里只负责接受连接；每个会话句柄由一个监控任务持有，
/// 连接结束即回收，关闭监听器时再统一取消仍存活的任务。
fn spawn_accept_loop(
    listener: Arc<dyn GuiListener>,
    active_sessions: Arc<AtomicUsize>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut sessions = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok(connection) => {
                        // 每个任务持有一个 SessionHandle；receive 等待底层会话
                        // 结束，随后 handle 被 drop 回收。JoinSet 只保留活跃任务。
                        active_sessions.fetch_add(1, Ordering::AcqRel);
                        let guard = ActiveSessionGuard(Arc::clone(&active_sessions));
                        sessions.spawn(async move {
                            let _guard = guard;
                            let _ = connection.receive().await;
                        });
                    }
                    Err(error) => {
                        tracing::debug!(%error, "gui accept loop exited");
                        break;
                    }
                },
                Some(result) = sessions.join_next(), if !sessions.is_empty() => {
                    if let Err(error) = result {
                        tracing::debug!(%error, "gui session monitor exited");
                    }
                }
            }
        }
        sessions.abort_all();
        while sessions.join_next().await.is_some() {}
    })
}

/// 按端点类型路由的复合 GUI Transport（P17-11）：本地端点 → `LocalTransport`，
/// 远程端点 → 与 [`RealRemoteTransportProvider`] 共享同一实例的
/// [`RealRemoteTransport`]。发布后的远程端点由同一 Core 经 `GuiServer` 绑定并
/// 接受连接，本地与远程复用同一 GUI Connection Protocol（[ADR-027]）。
///
/// [ADR-027]: ../../docs/adr/ADR-027-local-remote-same-protocol.md
pub struct CompositeGuiTransport {
    local: Arc<dyn GuiTransportServer>,
    remote: Arc<RealRemoteTransport>,
}

impl CompositeGuiTransport {
    pub fn new(local: Arc<dyn GuiTransportServer>, remote: Arc<RealRemoteTransport>) -> Self {
        Self { local, remote }
    }
}

#[async_trait]
impl GuiTransportServer for CompositeGuiTransport {
    async fn bind(
        &self,
        endpoint: TransportEndpoint,
    ) -> Result<Box<dyn GuiListener>, TransportError> {
        match &endpoint {
            TransportEndpoint::Local { .. } => self.local.bind(endpoint).await,
            TransportEndpoint::Remote { .. } => self.remote.bind(endpoint).await,
            TransportEndpoint::Memory { .. } => self.local.bind(endpoint).await,
        }
    }
}

impl GuiServerHost for ServeGuiHost {
    fn start(&self, instance: &str) -> Result<(), String> {
        if self.accept_task.lock().unwrap().is_some() {
            return Err("gui server is already running".into());
        }
        let endpoint = endpoint_for(instance);
        let server = Arc::clone(&self.server);
        // trait 要求同步，而 bind 是异步的；宿主在 tokio 多线程 runtime 上
        // 调用，block_in_place + block_on 是标准做法。
        let listener = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                server
                    .bind(endpoint)
                    .await
                    .map_err(|error| error.to_string())
            })
        })?;
        let listener: Arc<dyn GuiListener> = Arc::from(listener);
        let task = spawn_accept_loop(Arc::clone(&listener), Arc::clone(&self.active_sessions));
        *self.accept_task.lock().unwrap() = Some(task);
        *self.listener.lock().unwrap() = Some(listener);
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        if let Some(task) = self.accept_task.lock().unwrap().take() {
            task.abort();
        }
        if let Some(listener) = self.listener.lock().unwrap().take() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    if let Err(error) = listener.close().await {
                        tracing::warn!(%error, "gui listener close failed");
                    }
                });
            });
        }
        // 关闭所有已绑定的远程端点监听器（serve 退出时的清理路径）。
        for (handle_id, (listener, task)) in self.remote_listeners.lock().unwrap().drain() {
            task.abort();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    if let Err(error) = listener.close().await {
                        tracing::warn!(%handle_id, %error, "remote gui listener close failed");
                    }
                });
            });
        }
        Ok(())
    }

    fn bind_remote(&self, handle_id: &str, endpoint: &TransportEndpoint) -> Result<(), String> {
        if self
            .remote_listeners
            .lock()
            .unwrap()
            .contains_key(handle_id)
        {
            return Err(format!(
                "remote listener for handle {handle_id:?} is already bound"
            ));
        }
        let server = Arc::clone(&self.server);
        let endpoint = endpoint.clone();
        let listener = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                server
                    .bind(endpoint)
                    .await
                    .map_err(|error| error.to_string())
            })
        })?;
        let listener: Arc<dyn GuiListener> = Arc::from(listener);
        let task = spawn_accept_loop(Arc::clone(&listener), Arc::clone(&self.active_sessions));
        self.remote_listeners
            .lock()
            .unwrap()
            .insert(handle_id.to_string(), (listener, task));
        Ok(())
    }

    fn close_remote(&self, handle_id: &str) -> Result<(), String> {
        let Some((listener, task)) = self.remote_listeners.lock().unwrap().remove(handle_id) else {
            return Err(format!(
                "no remote listener is bound for handle {handle_id:?}"
            ));
        };
        task.abort();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                if let Err(error) = listener.close().await {
                    tracing::warn!(%handle_id, %error, "remote gui listener close failed");
                }
            });
        });
        Ok(())
    }
}
// TEMP-P17-7-VERIFY-END
