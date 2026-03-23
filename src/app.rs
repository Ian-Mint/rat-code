use crate::claude::{self, ClaudeEvent};
use crossterm::event::{KeyCode, KeyModifiers};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// ── Event types ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum AppEvent {
    Key(crossterm::event::KeyEvent),
    #[allow(dead_code)]
    Resize(u16, u16),
    Claude(ClaudeEvent),
    ClaudeError(String),
}

// ── Domain model ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Role {
    User,
    Assistant,
    Tool { name: String, approved: bool },
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn user(content: String) -> Self {
        Self {
            role: Role::User,
            content,
        }
    }

    pub fn assistant(content: String) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }

    pub fn tool(name: String, content: String, approved: bool) -> Self {
        Self {
            role: Role::Tool { name, approved },
            content,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolRequest {
    pub name: String,
    pub input: String, // accumulated JSON
}

#[derive(Debug, PartialEq)]
pub enum Mode {
    Input,
    Responding,
    AwaitingApproval,
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct App {
    pub mode: Mode,
    pub messages: Vec<Message>,
    pub input: String,
    pub scroll_offset: u16, // lines scrolled up from bottom; 0 = pinned to bottom
    pub should_quit: bool,

    pending_tool: Option<ToolRequest>,
    pub current_tool: Option<ToolRequest>, // visible to ui during AwaitingApproval
    buffered_claude_events: Vec<ClaudeEvent>, // events arriving during AwaitingApproval
    claude_task: Option<JoinHandle<()>>,
}

impl App {
    pub fn new() -> Self {
        Self {
            mode: Mode::Input,
            messages: Vec::new(),
            input: String::new(),
            scroll_offset: 0,
            should_quit: false,
            pending_tool: None,
            current_tool: None,
            buffered_claude_events: Vec::new(),
            claude_task: None,
        }
    }

    pub fn handle_event(&mut self, event: AppEvent, tx: &mpsc::UnboundedSender<AppEvent>) {
        match event {
            AppEvent::Key(key) => self.handle_key(key, tx),
            AppEvent::Claude(ce) => self.handle_claude(ce),
            AppEvent::ClaudeError(e) => {
                self.messages
                    .push(Message::assistant(format!("[error: {e}]")));
                self.mode = Mode::Input;
            }
            AppEvent::Resize(_, _) => {}
        }
    }

    fn handle_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        tx: &mpsc::UnboundedSender<AppEvent>,
    ) {
        // Ctrl-C always quits
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.abort_claude();
            self.should_quit = true;
            return;
        }

        match self.mode {
            Mode::Input => self.handle_key_input(key, tx),
            Mode::Responding => self.handle_key_responding(key),
            Mode::AwaitingApproval => self.handle_key_approval(key),
        }
    }

    fn handle_key_input(
        &mut self,
        key: crossterm::event::KeyEvent,
        tx: &mpsc::UnboundedSender<AppEvent>,
    ) {
        match key.code {
            KeyCode::Enter => {
                let prompt = self.input.trim().to_string();
                if !prompt.is_empty() {
                    self.submit(prompt, tx);
                }
            }
            KeyCode::Char(c) => self.input.push(c),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Up => self.scroll_up(),
            KeyCode::Down => self.scroll_down(),
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
            }
            _ => {}
        }
    }

    fn handle_key_responding(&mut self, key: crossterm::event::KeyEvent) {
        match key.code {
            KeyCode::Char('q') => {
                self.abort_claude();
                self.mode = Mode::Input;
            }
            KeyCode::Up => self.scroll_up(),
            KeyCode::Down => self.scroll_down(),
            _ => {}
        }
    }

    fn handle_key_approval(&mut self, key: crossterm::event::KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(tool) = self.current_tool.take() {
                    self.messages
                        .push(Message::tool(tool.name, tool.input, true));
                }
                self.mode = Mode::Responding;
                for event in std::mem::take(&mut self.buffered_claude_events) {
                    self.handle_claude(event);
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                if let Some(tool) = self.current_tool.take() {
                    self.messages
                        .push(Message::tool(tool.name, tool.input, false));
                }
                self.buffered_claude_events.clear();
                self.abort_claude();
                self.mode = Mode::Input;
            }
            _ => {}
        }
    }

    fn handle_claude(&mut self, event: ClaudeEvent) {
        // Buffer events during approval; replay them when mode returns to Responding
        if self.mode == Mode::AwaitingApproval {
            self.buffered_claude_events.push(event);
            return;
        }

        match event {
            ClaudeEvent::TextDelta(text) => {
                match self.messages.last_mut() {
                    Some(m) if matches!(m.role, Role::Assistant) => m.content.push_str(&text),
                    _ => self.messages.push(Message::assistant(text)),
                }
                self.scroll_offset = 0; // pin to bottom while streaming
            }
            ClaudeEvent::ToolUseStart { name } => {
                self.pending_tool = Some(ToolRequest {
                    name,
                    input: String::new(),
                });
            }
            ClaudeEvent::ToolInputDelta(partial) => {
                if let Some(t) = &mut self.pending_tool {
                    t.input.push_str(&partial);
                }
            }
            ClaudeEvent::ToolUseStop => {
                if let Some(tool) = self.pending_tool.take() {
                    self.current_tool = Some(tool);
                    self.mode = Mode::AwaitingApproval;
                }
            }
            ClaudeEvent::MessageStop => {
                self.mode = Mode::Input;
                self.claude_task = None;
            }
        }
    }

    fn submit(&mut self, prompt: String, tx: &mpsc::UnboundedSender<AppEvent>) {
        self.messages.push(Message::user(prompt.clone()));
        self.messages.push(Message::assistant(String::new()));
        self.input.clear();
        self.scroll_offset = 0;
        self.mode = Mode::Responding;
        self.claude_task = Some(claude::spawn(prompt, tx.clone()));
    }

    fn abort_claude(&mut self) {
        if let Some(task) = self.claude_task.take() {
            task.abort();
        }
    }

    fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(3);
    }

    fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tokio::sync::mpsc;

    fn key_event(code: KeyCode) -> AppEvent {
        AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Reproduces the hang: events arriving during AwaitingApproval were silently
    /// dropped, so MessageStop was never processed and the app stayed in Responding.
    #[test]
    fn buffered_events_replayed_on_approval() {
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut app = App::new();
        app.mode = Mode::Responding;

        // Tool call sequence → transitions to AwaitingApproval
        app.handle_event(
            AppEvent::Claude(ClaudeEvent::ToolUseStart {
                name: "shell".to_string(),
            }),
            &tx,
        );
        app.handle_event(
            AppEvent::Claude(ClaudeEvent::ToolInputDelta(
                r#"{"command":"echo $SHELL"}"#.to_string(),
            )),
            &tx,
        );
        app.handle_event(AppEvent::Claude(ClaudeEvent::ToolUseStop), &tx);
        assert_eq!(app.mode, Mode::AwaitingApproval);

        // Follow-up events arrive while waiting for approval (previously dropped).
        app.handle_event(
            AppEvent::Claude(ClaudeEvent::TextDelta("/bin/fish\n".to_string())),
            &tx,
        );
        app.handle_event(AppEvent::Claude(ClaudeEvent::MessageStop), &tx);

        // Still waiting — buffered, not yet processed.
        assert_eq!(app.mode, Mode::AwaitingApproval);

        // User approves → buffered events should replay.
        app.handle_event(key_event(KeyCode::Char('y')), &tx);

        // MessageStop replayed: mode must be Input, not stuck in Responding.
        assert_eq!(
            app.mode,
            Mode::Input,
            "app should reach Input after buffered MessageStop replays on approval"
        );

        // TextDelta replayed: assistant message should contain the tool output.
        let has_response = app
            .messages
            .iter()
            .any(|m| matches!(m.role, Role::Assistant) && m.content.contains("/bin/fish"));
        assert!(
            has_response,
            "assistant response from buffered TextDelta should appear"
        );
    }

    /// Rejecting a tool use clears the buffer and aborts cleanly.
    #[test]
    fn buffered_events_cleared_on_rejection() {
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut app = App::new();
        app.mode = Mode::Responding;

        app.handle_event(
            AppEvent::Claude(ClaudeEvent::ToolUseStart {
                name: "shell".to_string(),
            }),
            &tx,
        );
        app.handle_event(
            AppEvent::Claude(ClaudeEvent::ToolInputDelta("{}".to_string())),
            &tx,
        );
        app.handle_event(AppEvent::Claude(ClaudeEvent::ToolUseStop), &tx);
        app.handle_event(
            AppEvent::Claude(ClaudeEvent::TextDelta("output".to_string())),
            &tx,
        );
        app.handle_event(AppEvent::Claude(ClaudeEvent::MessageStop), &tx);
        assert_eq!(app.mode, Mode::AwaitingApproval);

        app.handle_event(key_event(KeyCode::Char('n')), &tx);

        assert_eq!(app.mode, Mode::Input);
        assert!(app.buffered_claude_events.is_empty());
    }
}
