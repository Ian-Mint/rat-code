use crate::app::{App, Message, Mode, Role};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let inner_width = area.width.saturating_sub(2).max(1);

    // Expand the input box up to 5 content lines as text wraps
    let cursor_line = app.input.len() as u16 / inner_width;
    let content_lines = (cursor_line + 1).min(5);
    let input_height = content_lines + 2; // + top/bottom borders
    let scroll_row = cursor_line.saturating_sub(content_lines - 1);

    let [conv_area, input_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(input_height)]).areas(area);

    render_conversation(f, app, conv_area);
    render_input(f, app, input_area, scroll_row);

    if let Mode::AwaitingApproval = &app.mode
        && let Some(tool) = &app.current_tool
    {
        let name = tool.name.clone();
        let input = tool.input.clone();
        render_approval_modal(f, &name, &input, area);
    }

    if matches!(app.mode, Mode::Input) {
        let cursor_col = app.input.len() as u16 % inner_width;
        let x = input_area.x + 1 + cursor_col;
        let y = input_area.y + 1 + cursor_line - scroll_row;
        f.set_cursor_position((x, y));
    }
}

// ── Markdown renderer ────────────────────────────────────────────────────────

fn md_to_lines(text: &str) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut bold = false;
    let mut italic = false;
    let mut heading = false;
    let mut in_code_block = false;
    let mut in_list_item = false;
    // None = unordered, Some(n) = ordered with n as next item number
    let mut list_stack: Vec<Option<u64>> = Vec::new();

    macro_rules! flush {
        () => {
            if !spans.is_empty() {
                out.push(Line::from(std::mem::take(&mut spans)));
            }
        };
    }

    for event in Parser::new_ext(text, Options::all()) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { .. } => heading = true,
                Tag::Strong => bold = true,
                Tag::Emphasis => italic = true,
                Tag::CodeBlock(_) => {
                    flush!();
                    in_code_block = true;
                }
                Tag::List(start) => {
                    list_stack.push(start);
                }
                Tag::Item => {
                    flush!();
                    in_list_item = true;
                    let depth = list_stack.len().saturating_sub(1);
                    let indent = "  ".repeat(depth);
                    let prefix = match list_stack.last_mut() {
                        Some(None) => format!("{indent}• "),
                        Some(Some(n)) => {
                            let p = format!("{indent}{}. ", n);
                            *n += 1;
                            p
                        }
                        None => String::new(),
                    };
                    spans.push(Span::raw(prefix));
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    flush!();
                    if !in_list_item {
                        out.push(Line::raw(""));
                    }
                }
                TagEnd::Heading(_) => {
                    flush!();
                    heading = false;
                    out.push(Line::raw(""));
                }
                TagEnd::Strong => bold = false,
                TagEnd::Emphasis => italic = false,
                TagEnd::CodeBlock => {
                    flush!();
                    in_code_block = false;
                    out.push(Line::raw(""));
                }
                TagEnd::Item => {
                    flush!();
                    in_list_item = false;
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                    if list_stack.is_empty() {
                        out.push(Line::raw(""));
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                let t = t.into_string();
                if in_code_block {
                    for (i, line) in t.split('\n').enumerate() {
                        if i > 0 {
                            flush!();
                        }
                        if !line.is_empty() {
                            spans.push(Span::styled(
                                line.to_string(),
                                Style::default().fg(Color::Yellow),
                            ));
                        }
                    }
                } else {
                    let mut style = Style::default();
                    if bold {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    if italic {
                        style = style.add_modifier(Modifier::ITALIC);
                    }
                    if heading {
                        style = style.add_modifier(Modifier::BOLD).fg(Color::Cyan);
                    }
                    spans.push(Span::styled(t, style));
                }
            }
            Event::Code(code) => {
                spans.push(Span::styled(
                    code.into_string(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            Event::SoftBreak => spans.push(Span::raw(" ")),
            Event::HardBreak => flush!(),
            Event::Rule => {
                flush!();
                out.push(Line::raw("─".repeat(40)));
                out.push(Line::raw(""));
            }
            _ => {}
        }
    }

    flush!();

    // Remove trailing blank line
    if out
        .last()
        .map(|l: &Line| l.spans.is_empty())
        .unwrap_or(false)
    {
        out.pop();
    }

    out
}

// ── Conversation ─────────────────────────────────────────────────────────────

fn build_lines(messages: &[Message]) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();

    for msg in messages {
        match &msg.role {
            Role::Assistant => {
                let style = Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD);
                let prefix = Span::styled("claude ".to_string(), style);

                let md_lines = md_to_lines(&msg.content);
                if md_lines.is_empty() {
                    out.push(Line::from(vec![prefix]));
                } else {
                    for (i, mut line) in md_lines.into_iter().enumerate() {
                        if i == 0 {
                            line.spans.insert(0, prefix.clone());
                        } else {
                            line.spans.insert(0, Span::raw("       ")); // 7 spaces
                        }
                        out.push(line);
                    }
                }
                out.push(Line::raw(""));
            }
            _ => {
                let (prefix, first_prefix): (String, Vec<Span<'static>>) = match &msg.role {
                    Role::User => {
                        let style = Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD);
                        let p = "you    ".to_string();
                        (p.clone(), vec![Span::styled(p, style)])
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
                    Role::Assistant => unreachable!(),
                };

                let content_lines: Vec<&str> = if msg.content.is_empty() {
                    vec![""]
                } else {
                    msg.content.split('\n').collect()
                };

                let indent = " ".repeat(prefix.len());

                for (i, line_text) in content_lines.iter().enumerate() {
                    if i == 0 {
                        let mut line_spans = first_prefix.clone();
                        line_spans.push(Span::raw(line_text.to_string()));
                        out.push(Line::from(line_spans));
                    } else {
                        out.push(Line::from(vec![
                            Span::raw(indent.clone()),
                            Span::raw(line_text.to_string()),
                        ]));
                    }
                }

                out.push(Line::raw(""));
            }
        }
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

fn render_input(f: &mut Frame, app: &App, area: Rect, scroll_row: u16) {
    let quit_hint = if app.pending_quit {
        " [again to exit]"
    } else {
        ""
    };

    let (title, border_style) = match app.mode {
        Mode::Input => (format!(" input{quit_hint} "), Style::default()),
        Mode::Responding => (
            format!(" input [q to cancel]{quit_hint} "),
            Style::default().fg(Color::Yellow),
        ),
        Mode::AwaitingApproval => (
            format!(" input [approval pending]{quit_hint} "),
            Style::default().fg(Color::Magenta),
        ),
    };

    let para = Paragraph::new(app.input.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll_row, 0));

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
