//! Prints every screen of the demo UI as plain text: `cargo run -p conductor-tui --example snap`.
use conductor_tui::{
    app::{App, Screen},
    theme::Theme,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
fn main() {
    let mut app = App::demo();
    for s in Screen::ALL {
        app.go(s);
        if s == Screen::Live {
            for _ in 0..3 {
                app.tick();
            }
        }
        let (w, h) = (130, 38);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| ui::draw(f, &app, &Theme::DARK)).unwrap();
        let b = t.backend().buffer().clone();
        println!("===== {s:?}");
        for y in 0..h {
            let mut l = String::new();
            for x in 0..w {
                l.push_str(b[(x, y)].symbol());
            }
            println!("{}", l.trim_end());
        }
    }
}
