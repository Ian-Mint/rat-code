use crate::app::AppEvent;
use serde_json::Value;
use std::process::Stdio;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::mpsc,
};

#[derive(Debug, Clone)]
pub enum ClaudeEvent {
    TextDelta(String),
    ToolUseStart { name: String },
    ToolInputDelta(String),
    ToolUseStop,
    MessageStop,
}

pub fn spawn(prompt: String, tx: mpsc::UnboundedSender<AppEvent>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut child = match Command::new("claude")
            .args(["-p", &prompt, "--output-format", "stream-json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(AppEvent::ClaudeError(format!(
                    "failed to spawn claude: {e}"
                )));
                return;
            }
        };

        let stdout = child.stdout.take().expect("stdout piped");
        let mut lines = BufReader::new(stdout).lines();

        'outer: while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            for event in parse_line(&line) {
                let done = matches!(event, ClaudeEvent::MessageStop);
                let _ = tx.send(AppEvent::Claude(event));
                if done {
                    break 'outer;
                }
            }
        }

        let _ = child.wait().await;
    })
}

// Parses a line from `claude --output-format stream-json`.
// The CLI emits {"type":"assistant","message":{...}} for content and
// {"type":"result",...} to signal completion — NOT the raw Anthropic API
// SSE event types (content_block_start / content_block_delta / message_stop).
fn parse_line(line: &str) -> Vec<ClaudeEvent> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return vec![];
    };
    match v["type"].as_str() {
        Some("assistant") => {
            let mut events = vec![];
            if let Some(blocks) = v["message"]["content"].as_array() {
                for block in blocks {
                    match block["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = block["text"].as_str()
                                && !text.is_empty()
                            {
                                events.push(ClaudeEvent::TextDelta(text.to_owned()));
                            }
                        }
                        Some("tool_use") => {
                            let name = block["name"].as_str().unwrap_or("unknown").to_owned();
                            let input = serde_json::to_string(&block["input"]).unwrap_or_default();
                            events.push(ClaudeEvent::ToolUseStart { name });
                            events.push(ClaudeEvent::ToolInputDelta(input));
                            events.push(ClaudeEvent::ToolUseStop);
                        }
                        _ => {}
                    }
                }
            }
            events
        }
        Some("result") => vec![ClaudeEvent::MessageStop],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_world_response_emits_text_delta() {
        // Reproduces the hang: old parse_line returned None for the CLI format,
        // so MessageStop was never sent and the app stayed stuck in Responding.
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello, world!"}],"stop_reason":"end_turn"},"session_id":"x"}"#;
        let events = parse_line(line);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::TextDelta(t) if t == "Hello, world!")),
            "expected TextDelta with response text, got: {events:?}"
        );
    }

    #[test]
    fn result_event_emits_message_stop() {
        let line =
            r#"{"type":"result","subtype":"success","result":"Hello, world!","session_id":"x"}"#;
        let events = parse_line(line);
        assert!(
            events.iter().any(|e| matches!(e, ClaudeEvent::MessageStop)),
            "expected MessageStop from result event, got: {events:?}"
        );
    }

    #[test]
    fn system_init_event_ignored() {
        let line = r#"{"type":"system","subtype":"init","session_id":"x","tools":[]}"#;
        assert!(parse_line(line).is_empty());
    }

    #[test]
    fn tool_use_block_emits_start_delta_stop() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"bash","input":{"command":"echo hi"}}],"stop_reason":"tool_use"},"session_id":"x"}"#;
        let events = parse_line(line);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::ToolUseStart { name } if name == "bash"))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClaudeEvent::ToolInputDelta(_)))
        );
        assert!(events.iter().any(|e| matches!(e, ClaudeEvent::ToolUseStop)));
    }
}
