//! 把 `AgentEventEnvelope` 打到终端。
//!
//! 文本模式：`AssistantTextDelta` → stdout，`AssistantThinkingDelta` → stderr。
//! `--json`：每行一个信封 JSON，只写 stdout。

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use pawork_domain::{AgentEvent, AgentEventEnvelope};
use pawork_engine::{AgentEventSink, EngineError};

pub struct TextSink {
    text: Mutex<String>,
    thinking_open: AtomicBool,
}

impl Default for TextSink {
    fn default() -> Self {
        Self {
            text: Mutex::new(String::new()),
            thinking_open: AtomicBool::new(false),
        }
    }
}

impl TextSink {
    #[allow(dead_code)]
    pub fn collected_text(&self) -> String {
        self.text.lock().expect("sink text mutex").clone()
    }
}

#[async_trait]
impl AgentEventSink for TextSink {
    async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
        match envelope.payload {
            AgentEvent::AssistantTextDelta { delta, .. } => {
                close_thinking(self)?;
                print!("{delta}");
                io::stdout()
                    .flush()
                    .map_err(|error| EngineError::sink(error.to_string()))?;
                self.text.lock().expect("sink text mutex").push_str(&delta);
            }
            AgentEvent::AssistantThinkingDelta { delta, .. } => {
                if !self.thinking_open.swap(true, Ordering::AcqRel) {
                    eprint!("thinking: ");
                }
                eprint!("{delta}");
                io::stderr()
                    .flush()
                    .map_err(|error| EngineError::sink(error.to_string()))?;
            }
            _ => {}
        }
        Ok(())
    }
}

pub struct JsonlSink;

#[async_trait]
impl AgentEventSink for JsonlSink {
    async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
        let line = serde_json::to_string(&envelope)
            .map_err(|error| EngineError::sink(error.to_string()))?;
        println!("{line}");
        io::stdout()
            .flush()
            .map_err(|error| EngineError::sink(error.to_string()))?;
        Ok(())
    }
}

fn close_thinking(sink: &TextSink) -> Result<(), EngineError> {
    if sink.thinking_open.swap(false, Ordering::AcqRel) {
        eprintln!();
        io::stderr()
            .flush()
            .map_err(|error| EngineError::sink(error.to_string()))?;
    }
    Ok(())
}
