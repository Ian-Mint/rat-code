use crate::app::AppEvent;
use futures::StreamExt;
use rig::agent::MultiTurnStreamItem;
use rig::client::CompletionClient;
use rig::providers::anthropic;
use rig::streaming::{StreamedAssistantContent, StreamingPrompt};
use tokio::sync::mpsc;

const MODEL: &str = "claude-sonnet-4-5-20251001";

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
        let api_key = match std::env::var("CLAUDE_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                let _ = tx.send(AppEvent::ClaudeError(
                    "CLAUDE_API_KEY environment variable not set".to_string(),
                ));
                return;
            }
        };

        let client = match anthropic::Client::new(&api_key) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(AppEvent::ClaudeError(format!("client error: {e}")));
                return;
            }
        };
        let agent = client.agent(MODEL).max_tokens(8096).build();

        let mut stream = agent.stream_prompt(&prompt).await;

        let mut sent_stop = false;

        while let Some(result) = stream.next().await {
            let item = match result {
                Ok(i) => i,
                Err(e) => {
                    let _ = tx.send(AppEvent::ClaudeError(format!("stream error: {e}")));
                    return;
                }
            };
            match item {
                MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text)) => {
                    if !text.text.is_empty() {
                        let _ = tx.send(AppEvent::Claude(ClaudeEvent::TextDelta(text.text)));
                    }
                }
                MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ToolCall {
                    tool_call,
                    ..
                }) => {
                    let name = tool_call.function.name.clone();
                    let input = serde_json::to_string(&tool_call.function).unwrap_or_default();
                    let _ = tx.send(AppEvent::Claude(ClaudeEvent::ToolUseStart { name }));
                    let _ = tx.send(AppEvent::Claude(ClaudeEvent::ToolInputDelta(input)));
                    let _ = tx.send(AppEvent::Claude(ClaudeEvent::ToolUseStop));
                }
                MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Final(_)) => {
                    let _ = tx.send(AppEvent::Claude(ClaudeEvent::MessageStop));
                    sent_stop = true;
                    break;
                }
                _ => {}
            }
        }

        if !sent_stop {
            let _ = tx.send(AppEvent::ClaudeError(
                "stream ended without a final message".to_string(),
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn missing_api_key_sends_claude_error() {
        // Temporarily ensure CLAUDE_API_KEY is unset for this test.
        // If it happens to be set in CI, the test is skipped.
        if std::env::var("CLAUDE_API_KEY").is_ok() {
            return;
        }
        let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
        let handle = spawn("hello".to_string(), tx);
        handle.await.unwrap();
        let event = rx.recv().await.expect("expected an event");
        assert!(
            matches!(event, AppEvent::ClaudeError(_)),
            "expected ClaudeError when API key is missing, got: {event:?}"
        );
    }
}
