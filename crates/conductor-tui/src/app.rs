//! Screen state and what each key does. No drawing, no terminal: that is what makes the
//! behaviour testable without one.

use conductor_model::view::LiveRun;
use conductor_model::{Receipt, RunSummary, demo};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Runs,
    Live,
    Receipt,
    Launch,
}

impl Screen {
    pub const ALL: [Screen; 4] = [Screen::Runs, Screen::Live, Screen::Receipt, Screen::Launch];

    pub fn title(self) -> &'static str {
        match self {
            Screen::Runs => "runs",
            Screen::Live => "live",
            Screen::Receipt => "receipt",
            Screen::Launch => "launch",
        }
    }
}

/// The three launch questions and their current answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// Which question has focus: 0 kind, 1 spec, 2 where.
    pub focus: usize,
    pub spec: String,
    pub headless: bool,
}

impl Default for Launch {
    fn default() -> Self {
        Launch {
            focus: 0,
            spec: "tickets/PTT-1240.md".into(),
            headless: false,
        }
    }
}

pub struct App {
    pub screen: Screen,
    pub runs: Vec<RunSummary>,
    pub selected: usize,
    pub live: LiveRun,
    live_step: usize,
    pub receipt: Receipt,
    pub show_how: bool,
    pub launch: Launch,
    /// A one-line message in the key bar, such as what a key would do in herdr.
    pub status: Option<String>,
    pub quit: bool,
}

impl App {
    /// The demo app. The engine will build the same state from real runs.
    pub fn demo() -> Self {
        App {
            screen: Screen::Runs,
            runs: demo::runs(),
            selected: 0,
            live: demo::live(),
            live_step: 0,
            receipt: demo::receipt(),
            show_how: false,
            launch: Launch::default(),
            status: None,
            quit: false,
        }
    }

    pub fn go(&mut self, screen: Screen) {
        self.screen = screen;
        self.status = None;
    }

    /// Called on a timer. Advances the demo run while its screen is open.
    pub fn tick(&mut self) {
        if self.screen == Screen::Live && demo::step(&mut self.live, self.live_step) {
            self.live_step += 1;
        }
    }

    pub fn live_done(&self) -> bool {
        self.live.is_done()
    }

    fn restart_live(&mut self) {
        self.live = demo::live();
        self.live_step = 0;
    }

    /// The keys shown along the bottom for the current screen. `true` marks the receipt key,
    /// which is highlighted everywhere it appears.
    pub fn keys(&self) -> &'static [(&'static str, &'static str, bool)] {
        match self.screen {
            Screen::Runs => &[
                ("↑↓", "move", false),
                ("⏎", "open", false),
                ("r", "receipt", true),
                ("n", "new run", false),
                ("t", "go to run's tab", false),
                ("q", "quit", false),
            ],
            Screen::Live => &[
                ("t", "watch in herdr tab", false),
                ("r", "receipt so far", true),
                ("p", "pause", false),
                ("x", "stop", false),
                ("esc", "all runs", false),
            ],
            Screen::Receipt => &[
                ("h", "how it ran", false),
                ("o", "open PR", false),
                ("d", "diff", false),
                ("v", "re-check", false),
                ("esc", "all runs", false),
            ],
            Screen::Launch => &[
                ("↑↓", "question", false),
                ("←→", "choose", false),
                ("⏎", "start run", false),
                ("esc", "cancel", false),
            ],
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        // Typing a spec path owns every printable key.
        if self.screen == Screen::Launch && self.launch.focus == 1 {
            match key.code {
                KeyCode::Char(c) => {
                    self.launch.spec.push(c);
                    return;
                }
                KeyCode::Backspace => {
                    self.launch.spec.pop();
                    return;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Char(c @ '1'..='4') => self.go(Screen::ALL[(c as usize) - ('1' as usize)]),
            KeyCode::Char('r') => self.go(Screen::Receipt),
            KeyCode::Esc => self.go(Screen::Runs),
            KeyCode::Char('q') if self.screen == Screen::Runs => self.quit = true,
            _ => match self.screen {
                Screen::Runs => self.runs_key(key.code),
                Screen::Live => self.live_key(key.code),
                Screen::Receipt => self.receipt_key(key.code),
                Screen::Launch => self.launch_key(key.code),
            },
        }
    }

    fn runs_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.runs.len().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Enter => {
                let run = &self.runs[self.selected];
                if run.is_finished() {
                    self.go(Screen::Receipt)
                } else {
                    self.go(Screen::Live)
                }
            }
            KeyCode::Char('n') => self.go(Screen::Launch),
            KeyCode::Char('t') => {
                self.status = Some(match self.runs[self.selected].place {
                    conductor_model::Place::HerdrTab(n) => format!("herdr would focus tab {n}"),
                    _ => "this run has no herdr tab".into(),
                })
            }
            _ => {}
        }
    }

    fn live_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('t') => {
                self.status = Some(format!(
                    "herdr would focus tab {}",
                    self.live.herdr_tab.unwrap_or(2)
                ))
            }
            KeyCode::Enter if self.live_done() => self.go(Screen::Receipt),
            KeyCode::Char('p') => self.status = Some("paused — p again to resume (demo)".into()),
            _ => {}
        }
    }

    fn receipt_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('h') => self.show_how = !self.show_how,
            KeyCode::Char('v') => {
                self.status = Some("re-checked: the verdict reproduces from stored evidence".into())
            }
            _ => {}
        }
    }

    fn launch_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Down | KeyCode::Tab => self.launch.focus = (self.launch.focus + 1).min(2),
            KeyCode::Up | KeyCode::BackTab => {
                self.launch.focus = self.launch.focus.saturating_sub(1)
            }
            KeyCode::Left | KeyCode::Right if self.launch.focus == 2 => {
                self.launch.headless = !self.launch.headless
            }
            KeyCode::Enter => {
                self.restart_live();
                self.go(Screen::Live);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new_with_kind(
            code,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        ));
    }

    #[test]
    fn number_keys_switch_screens() {
        let mut app = App::demo();
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(app.screen, Screen::Receipt);
        press(&mut app, KeyCode::Char('4'));
        assert_eq!(app.screen, Screen::Launch);
    }

    #[test]
    fn r_opens_the_receipt_from_every_screen() {
        for s in [Screen::Runs, Screen::Live, Screen::Launch] {
            let mut app = App::demo();
            app.go(s);
            press(&mut app, KeyCode::Char('r'));
            assert_eq!(app.screen, Screen::Receipt, "from {s:?}");
        }
    }

    #[test]
    fn enter_opens_a_running_run_live_and_a_finished_one_as_a_receipt() {
        let mut app = App::demo();
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen, Screen::Live);
        app.go(Screen::Runs);
        let finished = app.runs.iter().position(|r| r.is_finished()).unwrap();
        app.selected = finished;
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen, Screen::Receipt);
    }

    #[test]
    fn selection_stays_inside_the_list() {
        let mut app = App::demo();
        press(&mut app, KeyCode::Up);
        assert_eq!(app.selected, 0);
        for _ in 0..50 {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.selected, app.runs.len() - 1);
    }

    #[test]
    fn typing_a_spec_path_does_not_trigger_shortcuts() {
        let mut app = App::demo();
        app.go(Screen::Launch);
        press(&mut app, KeyCode::Down);
        for c in "r1q".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.screen, Screen::Launch);
        assert!(app.launch.spec.ends_with("r1q"));
        assert!(!app.quit);
    }

    #[test]
    fn starting_a_run_restarts_the_live_view() {
        let mut app = App::demo();
        app.go(Screen::Live);
        for _ in 0..20 {
            app.tick();
        }
        assert!(app.live_done());
        app.go(Screen::Launch);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen, Screen::Live);
        assert!(!app.live_done());
    }

    #[test]
    fn the_demo_only_advances_while_you_watch() {
        let mut app = App::demo();
        for _ in 0..20 {
            app.tick();
        }
        assert!(
            !app.live_done(),
            "ticks on the runs screen must not move the live run"
        );
    }
}
