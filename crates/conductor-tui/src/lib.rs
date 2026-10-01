//! The conductor terminal UI. It is built to run as a herdr pane, and also runs on its own.

pub mod app;
mod clarification;
mod document;
mod result_view;
pub mod theme;
pub mod ui;
mod workbench;
pub mod workspace;

use app::App;
use crossterm::event::{self, Event, KeyEventKind};
use std::io;
use std::time::{Duration, Instant};
use theme::Theme;

/// How often the demo run advances while its screen is open.
const TICK: Duration = Duration::from_millis(1500);

/// Runs the UI until the user quits. Restores the terminal on every exit path, including an
/// error from drawing.
pub fn run(mut app: App, theme: Theme) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = (|| {
        let mut last_tick = Instant::now();
        while !app.quit {
            terminal.draw(|f| ui::draw(f, &app, &theme))?;
            let wait = TICK.saturating_sub(last_tick.elapsed());
            if event::poll(wait)?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                app.on_key(key);
            }
            if last_tick.elapsed() >= TICK {
                app.tick();
                last_tick = Instant::now();
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

/// The task workspace, including bracketed paste and responsive inspectors.
pub fn run_workspace(mut app: workspace::Workspace, theme: Theme) -> io::Result<()> {
    use crossterm::{
        event::{DisableBracketedPaste, EnableBracketedPaste},
        execute,
    };
    let mut terminal = ratatui::init();
    let result = (|| {
        execute!(io::stdout(), EnableBracketedPaste)?;
        let mut last_tick = Instant::now();
        let mut dirty = true;
        while !app.quit {
            if dirty {
                terminal.draw(|f| workspace::draw(f, &app, &theme))?;
                dirty = false;
            }
            // A spinner or a newly arrived step redraws every poll; a still screen does not.
            if app.animating() {
                app.advance();
                dirty = true;
            }
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        app.on_key(key);
                        dirty = true;
                    }
                    Event::Paste(text) => {
                        app.paste(&text);
                        dirty = true;
                    }
                    Event::Resize(_, _) => dirty = true,
                    _ => {}
                }
            }
            if last_tick.elapsed() >= Duration::from_millis(500) {
                app.tick();
                last_tick = Instant::now();
                dirty |= !app.demo;
            }
        }
        Ok(())
    })();
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}
