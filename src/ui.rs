use crate::app::{App, Message, Mode, Role};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub fn render(f: &mut Frame, app: &mut App) {
    let [conv_area, input_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(3)]).areas(f.area());

    render_conversation(f, app, conv_area);
    render_input(f, app, input_area);

    if let Mode::AwaitingApproval = &app.mode
        && let Some(tool) = &app.current_tool
    {
        let name = tool.name.clone();
        let input = tool.input.clone();
        render_approval_modal(f, &name, &input, f.area());
    }

    if matches!(app.mode, Mode::Input) {
        let x = input_area.x + 1 + app.input.len() as u16;
        let y = input_area.y + 1;
        f.set_cursor_position((x.min(input_area.x + input_area.width - 2), y));
    }
}

// ── Conversation ─────────────────────────────────────────────────────────────

fn build_lines(messages: &[Message]) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();

    for msg in messages {
        let (prefix, first_prefix): (String, Vec<Span<'static>>) = match &msg.role {
            Role::User => {
                let style = Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD);
                let p = "you    ".to_string();
                let spans = vec![Span::styled(p.clone(), style)];
                (p, spans)
            }
            Role::Assistant => {
                let style = Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD);
                let p = "claude ".to_string();
                let spans = vec![Span::styled(p.clone(), style)];
                (p, spans)
            }
            Role::Tool { name, approved } => {
                let color = if *approved { Color::Yellow } else { Color::Red };
                let mark = if *approved { "✓" } else { "✗" };
                let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
                let p = "tool   ".to_string();
                let spans = vec![
                    Span::styled(p.clone(), style),
                    Span::styled(format!("{name} {mark} "), Style::default().fg(color)),
                ];
                (p, spans)
            }
        };

        // Split content on newlines so each logical line scrolls independently
        let content_lines: Vec<&str> = if msg.content.is_empty() {
            vec![""]
        } else {
            msg.content.split('\n').collect()
        };

        let indent = " ".repeat(prefix.len());

        for (i, line_text) in content_lines.iter().enumerate() {
            if i == 0 {
                let mut spans = first_prefix.clone();
                spans.push(Span::raw(line_text.to_string()));
                out.push(Line::from(spans));
            } else {
                out.push(Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::raw(line_text.to_string()),
                ]));
            }
        }

        // Blank line between messages
        out.push(Line::raw(""));
    }

    out
}

fn render_conversation(f: &mut Frame, app: &App, area: Rect) {
    let title = match app.mode {
        Mode::Responding => " rat [responding…] ",
        _ => " rat ",
    };

    let lines = build_lines(&app.messages);
    let total = lines.len() as u16;
    let visible = area.height.saturating_sub(2); // subtract borders
    let base_scroll = total.saturating_sub(visible);
    let scroll = base_scroll.saturating_sub(app.scroll_offset);

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));

    f.render_widget(para, area);
}

// ── Input ────────────────────────────────────────────────────────────────────

fn render_input(f: &mut Frame, app: &App, area: Rect) {
    let (title, border_style) = match app.mode {
        Mode::Input => (" input ", Style::default()),
        Mode::Responding => (" input [q to cancel] ", Style::default().fg(Color::Yellow)),
        Mode::AwaitingApproval => (
            " input [approval pending] ",
            Style::default().fg(Color::Magenta),
        ),
    };

    let para = Paragraph::new(app.input.as_str()).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(border_style),
    );

    f.render_widget(para, area);
}

// ── Approval modal ───────────────────────────────────────────────────────────

fn render_approval_modal(f: &mut Frame, tool_name: &str, tool_input: &str, area: Rect) {
    let modal_area = centered_rect(65, 55, area);
    f.render_widget(Clear, modal_area);

    let pretty = serde_json::from_str::<serde_json::Value>(tool_input)
        .and_then(|v| serde_json::to_string_pretty(&v))
        .unwrap_or_else(|_| tool_input.to_string());

    let body = format!("tool: {tool_name}\n\n{pretty}\n\n[y] approve    [n / esc] reject");

    let para = Paragraph::new(body)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" tool use request ")
                .border_style(
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(para, modal_area);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let [_, mid, _] = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .areas(area);

    let [_, center, _] = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .areas(mid);

    center
}
