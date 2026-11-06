//! fuga TUI. M0 renders a placeholder and proves setup/teardown; the real layout
//! arrives in M3.

mod terminal;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::Paragraph;
use terminal::TerminalGuard;

fn main() -> Result<()> {
    let mut guard = TerminalGuard::new()?;
    loop {
        guard.terminal.draw(|frame| {
            frame.render_widget(Paragraph::new("fuga"), frame.area());
        })?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press && is_quit(key) {
                break;
            }
        }
    }
    Ok(())
}

fn is_quit(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}
