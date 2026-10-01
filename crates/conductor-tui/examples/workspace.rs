//! Render the actual workspace through Ratatui's test backend, for layout review.
use conductor_tui::{
    theme::Theme,
    workspace::{self, Panel, Workspace},
};
use ratatui::{Terminal, backend::TestBackend};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let width = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(140);
    let height = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(32);
    let mut app = Workspace::demo();
    if args.iter().any(|s| s == "clarify") {
        app.open("q-clarification");
    }
    if args.iter().any(|s| s == "comparison") {
        app.open("q-comparison");
    }
    if args.iter().any(|s| s == "forecast") {
        app.open("q-forecast");
    }
    if args.iter().any(|s| s == "series") {
        app.open("q-series");
    }
    if args.iter().any(|s| s == "tests") {
        app.open("test-demo");
    }
    if args.iter().any(|s| s == "bug") {
        app.open("bug-demo");
    }
    if args.iter().any(|s| s == "patch") {
        app.change_focus = true;
    }
    app.panel = match args.get(3).map(String::as_str) {
        Some("sources") => Some(Panel::Sources),
        Some("activity") => Some(Panel::Activity),
        _ => None,
    };
    if let Some(index) = args.iter().position(|a| a == "--question") {
        let path = args
            .get(index + 1)
            .expect("--question requires a saved question.json");
        let question: conductor_model::task::Question =
            serde_json::from_slice(&std::fs::read(path).expect("read saved question"))
                .expect("parse saved question");
        let mut detail = workspace::demo_task();
        detail.summary.id = question.id.clone();
        detail.summary.title = question.title.clone();
        detail.summary.state = question
            .turns
            .last()
            .map(|t| t.state)
            .unwrap_or(conductor_model::task::State::Stopped);
        app.turn = question.turns.len().saturating_sub(1);
        detail.question = Some(question);
        app.detail = Some(detail);
        app.demo = false;
    }
    app.focused = args.iter().any(|s| s == "focused");
    if args.iter().any(|s| s == "receipt") {
        app.toggle_panel(Panel::Record);
    }
    if args.iter().any(|s| s == "picker") {
        app.detail = None;
        app.on_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::F(3),
            crossterm::event::KeyModifiers::NONE,
        ));
    }
    app.expanded = args.iter().any(|s| s == "expanded");
    // Script actual key handling for documentation captures; no runtime behavior changes.
    if let Some(index) = args.iter().position(|a| a == "--keys") {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for key in args
            .get(index + 1)
            .expect("--keys requires comma-separated keys")
            .split(',')
        {
            let code = match key {
                "down" => KeyCode::Down,
                "up" => KeyCode::Up,
                "left" => KeyCode::Left,
                "right" => KeyCode::Right,
                "enter" => KeyCode::Enter,
                "esc" => KeyCode::Esc,
                "tab" => KeyCode::Tab,
                "space" => KeyCode::Char(' '),
                "f3" => KeyCode::F(3),
                text if text.chars().count() == 1 => KeyCode::Char(text.chars().next().unwrap()),
                _ => panic!("unknown capture key: {key}"),
            };
            app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
    }
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    if args.iter().any(|a| a == "--bench") {
        let mut timings = Vec::new();
        for i in 0..120 {
            app.result_row = i % 3;
            let started = std::time::Instant::now();
            terminal
                .draw(|f| workspace::draw(f, &app, &Theme::DARK))
                .unwrap();
            if i >= 20 {
                timings.push(started.elapsed().as_micros());
            }
        }
        timings.sort_unstable();
        println!(
            "{}",
            serde_json::json!({"backend":"Ratatui TestBackend", "width":width,"height":height,"samples":timings.len(),"p50_us":timings[49],"p95_us":timings[94],"max_us":timings[99],"scope":"render only; excludes terminal I/O, saved-task loading and agent latency"})
        );
        return;
    }
    terminal
        .draw(|f| {
            workspace::draw(
                f,
                &app,
                &if args.iter().any(|s| s == "light") {
                    Theme::LIGHT
                } else {
                    Theme::DARK
                },
            )
        })
        .unwrap();
    let b = terminal.backend().buffer();
    if args.iter().any(|a| a == "--json") {
        let rows:Vec<_>=(0..height).map(|y|(0..width).map(|x| {
            let c=&b[(x,y)];
            serde_json::json!({"text":c.symbol(),"fg":format!("{:?}",c.fg),"bg":format!("{:?}",c.bg),"modifiers":format!("{:?}",c.modifier)})
        }).collect::<Vec<_>>()).collect();
        println!(
            "{}",
            serde_json::json!({"width":width,"height":height,"rows":rows})
        );
        return;
    }
    for y in 0..height {
        println!(
            "{}",
            (0..width).map(|x| b[(x, y)].symbol()).collect::<String>()
        );
    }
}
