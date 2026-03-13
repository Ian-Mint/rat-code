mod app;
mod claude;
mod ui;

use anyhow::Result;
use app::{App, AppEvent};
use crossterm::event::{Event, EventStream};
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    // Keyboard / resize event forwarder
    let tx2 = tx.clone();
    tokio::spawn(async move {
        let mut stream = EventStream::new();
        while let Some(Ok(event)) = stream.next().await {
            match event {
                Event::Key(k) => {
                    let _ = tx2.send(AppEvent::Key(k));
                }
                Event::Resize(w, h) => {
                    let _ = tx2.send(AppEvent::Resize(w, h));
                }
                _ => {}
            }
        }
    });

    let mut app = App::new();

    loop {
        terminal.draw(|f| ui::render(f, &mut app))?;
        match rx.recv().await {
            Some(event) => app.handle_event(event, &tx),
            None => break,
        }
        if app.should_quit {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
