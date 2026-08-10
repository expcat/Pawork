//! `pawork` 的 GUI Server 宿主（P13-4 接线）。
//!
//! 把 [`gui_server::GuiServer`] 装进 [`cli_host::GuiServerHost`] trait：
//! `serve` 模式经 [`ServeGuiHost::start`] 绑定本地端点并跑 accept 循环，
//! [`ServeGuiHost::stop`] 中止循环并关闭监听器。连接会话（握手 → 帧循环）
//! 由 `GuiServerListener::accept` 内部自行 spawn，宿主不做二次派发。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cli_host::GuiServerHost;
use gui_server::GuiServer;
use tokio::task::JoinHandle;
use transport_api::{GuiListener, TransportEndpoint};

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

/// [`GuiServerHost`] 实现：持有共享 [`GuiServer`]，维护 accept 循环任务与
/// 监听器的生命周期。
pub struct ServeGuiHost {
    server: Arc<GuiServer>,
    listener: Mutex<Option<Arc<dyn GuiListener>>>,
    accept_task: Mutex<Option<JoinHandle<()>>>,
}

impl ServeGuiHost {
    pub fn new(server: Arc<GuiServer>) -> Self {
        Self {
            server,
            listener: Mutex::new(None),
            accept_task: Mutex::new(None),
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
        let accept_listener = Arc::clone(&listener);
        // GuiServerListener::accept 内部已 spawn 会话任务（握手 → 帧循环），
        // 这里只负责接受连接；返回的宿主句柄 drop 会释放 close 通道导致
        // 会话断线，因此持有到循环结束（stop 中止任务时统一释放）。
        let task = tokio::spawn(async move {
            let mut sessions: Vec<Box<dyn transport_api::GuiConnection>> = Vec::new();
            loop {
                match accept_listener.accept().await {
                    Ok(connection) => sessions.push(connection),
                    Err(error) => {
                        tracing::warn!(%error, "gui accept loop exited");
                        break;
                    }
                }
            }
        });
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
        Ok(())
    }
}
