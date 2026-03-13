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
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                if let Some(tool) = self.current_tool.take() {
                    self.messages
                        .push(Message::tool(tool.name, tool.input, false));
                }
                self.abort_claude();
                self.mode = Mode::Input;
            }
            _ => {}
        }
    }

    fn handle_claude(&mut self, event: ClaudeEvent) {
        // Buffer events during approval; they'll be processed when mode returns to Responding
        if self.mode == Mode::AwaitingApproval {
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
