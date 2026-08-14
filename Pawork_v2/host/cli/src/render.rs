//! 把 `ProviderStreamEvent` 打到终端：`TextDelta` 走 stdout，`ThinkingDelta` 走 stderr。

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use pawork_api::{ProviderError, ProviderEventSink, ProviderStreamEvent};

#[derive(Default)]
pub struct StdoutSink {
    text: Mutex<String>,
    thinking_open: AtomicBool,
}

impl StdoutSink {
    pub fn collected_text(&self) -> String {
        self.text.lock().expect("sink text mutex").clone()
    }
}

#[async_trait]
impl ProviderEventSink for StdoutSink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        match event {
            ProviderStreamEvent::TextDelta(delta) => {
                close_thinking(self)?;
                print!("{delta}");
                io::stdout().flush().map_err(io_to_provider)?;
                self.text
                    .lock()
                    .expect("sink text mutex")
                    .push_str(&delta);
            }
            ProviderStreamEvent::ThinkingDelta(delta) => {
                if !self.thinking_open.swap(true, Ordering::AcqRel) {
                    eprint!("thinking: ");
                }
                eprint!("{delta}");
                io::stderr().flush().map_err(io_to_provider)?;
            }
            ProviderStreamEvent::Error(err) => return Err(err),
            _ => {}
        }
        Ok(())
    }
}

fn close_thinking(sink: &StdoutSink) -> Result<(), ProviderError> {
    if sink.thinking_open.swap(false, Ordering::AcqRel) {
        eprintln!();
        io::stderr().flush().map_err(io_to_provider)?;
    }
    Ok(())
}

fn io_to_provider(err: io::Error) -> ProviderError {
    ProviderError::new(pawork_api::ProviderErrorKind::Unknown, err.to_string())
}
