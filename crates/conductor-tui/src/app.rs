//! Screen state and what each key does. No drawing, no terminal: that is what makes the
//! behaviour testable without one.

use conductor_model::view::{LiveRun, Review};
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
    /// Tab order: start something, see what is running, watch one, read what it proved.
    pub const ALL: [Screen; 4] = [Screen::Launch, Screen::Runs, Screen::Live, Screen::Receipt];

    pub fn title(self) -> &'static str {
        match self {
            Screen::Runs => "runs",
            Screen::Live => "live",
            Screen::Receipt => "receipt",
            Screen::Launch => "launch",
        }
    }
}

/// The launch questions and their current answers: which workflow, and what to work on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// Which question has focus: 0 workflow, 1 the input.
    pub focus: usize,
    /// The repository's workflows, as paths, and which one is chosen.
    pub workflows: Vec<String>,
    pub workflow: usize,
    /// A ticket path, or the task in the person's words.
    pub spec: String,
    /// Where the run will go: a herdr tab when herdr is there, else headless.
    pub herdr: bool,
}

impl Default for Launch {
    fn default() -> Self {
        Launch {
            focus: 0,
            workflows: vec![".conductor/workflows/build.yaml".into()],
            workflow: 0,
            spec: "tickets/PTT-1240.md".into(),
            herdr: true,
        }
    }
}

/// Where a real app reads runs from. The command line implements it over the repository's
/// `.conductor/runs`; the UI only reads.
pub trait RunSource {
    /// Finished runs' receipts, newest first.
    fn receipts(&self) -> Vec<Receipt>;
    /// Runs still in progress (no receipt yet), newest first.
    fn running(&self) -> Vec<LiveRun>;
    /// One run's live view.
    fn live(&self, run_id: &str) -> Option<LiveRun>;
    /// The runs screen's three headline numbers, computed from the source's own metrics.
    /// `None` keeps the receipt-derived numbers `App` computes on its own.
    fn stats(&self) -> Option<[(String, String); 3]> {
        None
    }
    /// Records a person's decision on a `human` stage the run is waiting for, with the note
    /// that says what they checked.
    fn decide(
        &self,
        _run_id: &str,
        _stage: &str,
        _approved: bool,
        _note: &str,
    ) -> Result<(), String> {
        Err("deciding is not available here".into())
    }
    /// Records a person's answer to a tool the run's agent asked for.
    fn answer(&self, _run_id: &str, _ask: u32, _allowed: bool) -> Result<(), String> {
        Err("answering is not available here".into())
    }
    /// What a run waiting on a decision has to show for itself (`review.json`).
    fn review(&self, _run_id: &str) -> Option<Review> {
        None
    }
    /// The repository's workflows, as paths relative to it.
    fn workflows(&self) -> Vec<String> {
        vec![]
    }
    /// Whether a run started from here goes into a herdr tab.
    fn in_herdr(&self) -> bool {
        false
    }
    /// Starts a run of `workflow` on `input` (a ticket path if it names a file, else the
    /// task text) and returns its id once it exists.
    fn start(&self, _workflow: &str, _input: &str) -> Result<String, String> {
        Err("starting runs is not available here".into())
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
    /// Receipts behind the runs list, in the same order. Empty in the demo.
    pub receipts: Vec<Receipt>,
    /// A run waiting on a person: work, what it asks, herdr tab.
    pub needs_you: Option<(String, String, u16)>,
    /// A decision being typed on the live screen: approved or not, and the note so far.
    /// Enter records it; Esc drops it.
    pub deciding: Option<(bool, String)>,
    /// Runs whose receipt has no check at all (halted before any check ran), newest first.
    /// They are not rows in the list: nothing about them is worth reviewing.
    pub unchecked: Vec<String>,
    /// The review of the live run while it waits on a decision: the live screen shows it
    /// instead of the working view.
    pub review: Option<Review>,
    /// Three headline numbers for the runs screen: value and label.
    pub stats: [(String, String); 3],
    /// Sample data that plays itself, or real runs read from disk.
    pub demo: bool,
    source: Option<Box<dyn RunSource>>,
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
            receipts: vec![],
            deciding: None,
            unchecked: vec![],
            review: None,
            needs_you: demo::needs_you().map(|(w, a, t)| (w.into(), a.into(), t)),
            stats: [
                ("3 running".into(), "in herdr tabs 2–4".into()),
                (
                    "17%  ▁▂▁▃▃▅▄▆".into(),
                    "caught: the agent said done, a check said no".into(),
                ),
                ("23".into(), "runs in the last 7 days".into()),
            ],
            demo: true,
            source: None,
        }
    }

    /// The app over a repository's runs, refreshed on every tick.
    pub fn from_source(source: Box<dyn RunSource>) -> Self {
        let mut app = App::from_runs(source.receipts(), source.running());
        if let Some(stats) = source.stats() {
            app.stats = stats;
        }
        app.launch.workflows = source.workflows();
        app.launch.herdr = source.in_herdr();
        app.launch.spec.clear();
        app.source = Some(source);
        app
    }

    /// The app over real runs, newest first.
    pub fn from_receipts(receipts: Vec<Receipt>) -> Self {
        App::from_runs(receipts, vec![])
    }

    /// The app over runs in progress (listed first) and finished runs.
    pub fn from_runs(receipts: Vec<Receipt>, running: Vec<LiveRun>) -> Self {
        let mut app = App::demo();
        app.demo = false;
        app.live_step = usize::MAX;
        app.needs_you = None;
        app.receipt = receipts.first().cloned().unwrap_or_else(demo::receipt);
        if let Some(l) = running.first() {
            app.live = l.clone();
        }
        app.runs.clear();
        app.selected = 0;
        app.set_runs(receipts, running);
        app
    }

    fn set_runs(&mut self, receipts: Vec<Receipt>, running: Vec<LiveRun>) {
        let live_rows = running.iter().map(|l| RunSummary {
            run_id: l.run_id.clone(),
            work: l.work.clone(),
            kind: l.kind.clone(),
            status: l.ended.unwrap_or(conductor_model::Verdict::Running),
            checks: l.checks.iter().map(|c| c.status).collect(),
            place: match l.herdr_tab {
                Some(n) => conductor_model::Place::HerdrTab(n),
                None => conductor_model::Place::Headless,
            },
        });
        self.unchecked = receipts
            .iter()
            .filter(|r| r.checks.is_empty())
            .map(|r| r.run_id.clone())
            .collect();
        let done_rows = receipts
            .iter()
            .filter(|r| !r.checks.is_empty())
            .map(|r| RunSummary {
                run_id: r.run_id.clone(),
                work: r.work.clone(),
                kind: r.kind.clone(),
                status: r.verdict(),
                checks: r.checks.iter().map(|c| c.verdict).collect(),
                place: conductor_model::Place::Headless,
            });
        let runs: Vec<RunSummary> = live_rows.chain(done_rows).collect();
        let passed = receipts
            .iter()
            .filter(|r| r.verdict() == conductor_model::Verdict::Passed)
            .count();
        self.stats = [
            if running.is_empty() {
                (
                    format!("{}", receipts.len()),
                    "runs recorded in this repository".into(),
                )
            } else {
                (
                    format!("{} running", running.len()),
                    format!("{} recorded in this repository", receipts.len()),
                )
            },
            (
                format!("{passed} passed"),
                "every check witnessed by conductor".into(),
            ),
            (
                format!("{} did not pass", receipts.len() - passed),
                "open one to see which check stopped it".into(),
            ),
        ];
        // Keep the same run selected as the list changes under it.
        let keep = self.runs.get(self.selected).map(|r| r.run_id.clone());
        self.runs = runs;
        self.receipts = receipts;
        self.selected = keep
            .and_then(|id| self.runs.iter().position(|r| r.run_id == id))
            .unwrap_or(0)
            .min(self.runs.len().saturating_sub(1));
    }

    /// Shows the receipt of the selected run, in real mode.
    fn select_receipt(&mut self) -> bool {
        let Some(id) = self.runs.get(self.selected).map(|r| r.run_id.clone()) else {
            return false;
        };
        self.receipt_for(&id)
    }

    fn receipt_for(&mut self, run_id: &str) -> bool {
        match self.receipts.iter().find(|r| r.run_id == run_id) {
            Some(r) => {
                self.receipt = r.clone();
                true
            }
            None => false,
        }
    }

    pub fn go(&mut self, screen: Screen) {
        self.screen = screen;
        self.status = None;
    }

    /// Called on a timer. Advances the demo run while its screen is open, or re-reads real
    /// runs from their source.
    pub fn tick(&mut self) {
        if self.demo && self.screen == Screen::Live && demo::step(&mut self.live, self.live_step) {
            self.live_step += 1;
        }
        let Some(src) = &self.source else { return };
        let (receipts, running) = (src.receipts(), src.running());
        let live = src.live(&self.live.run_id);
        let stats = src.stats();
        // While the run waits on a decision, what the decision needs comes along.
        let waiting_on_decision = live
            .as_ref()
            .and_then(|l| l.waiting.as_ref())
            .is_some_and(|w| w.ask.is_none());
        let review = if waiting_on_decision {
            src.review(&self.live.run_id)
        } else {
            None
        };
        self.set_runs(receipts, running);
        if let Some(stats) = stats {
            self.stats = stats;
        }
        if let Some(l) = live {
            self.live = l;
        }
        self.review = review;
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
    /// Over real runs, keys that only the demo acts out (pause, stop, open PR, …) are left out.
    pub fn keys(&self) -> Vec<(&'static str, &'static str, bool)> {
        let all: &[(&str, &str, bool)] = match self.screen {
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
                ("←→", "workflow", false),
                ("⏎", "start run", false),
                ("esc", "runs", false),
            ],
        };
        if self.demo {
            return all.to_vec();
        }
        let demo_only: &[&str] = match self.screen {
            Screen::Runs => &["t"],
            Screen::Live if self.live.herdr_tab.is_none() => &["t", "p", "x"],
            Screen::Live => &["p", "x"],
            Screen::Receipt => &["o", "d", "v"],
            Screen::Launch => &[],
        };
        all.iter()
            .filter(|(k, _, _)| !demo_only.contains(k))
            .copied()
            .collect()
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        // Typing a decision's note owns every key until Enter or Esc.
        if self.screen == Screen::Live
            && let Some((yes, note)) = &mut self.deciding
        {
            match key.code {
                KeyCode::Char(c) => note.push(c),
                KeyCode::Backspace => {
                    note.pop();
                }
                KeyCode::Esc => {
                    self.deciding = None;
                    self.status = Some("not recorded".into());
                }
                KeyCode::Enter => {
                    let (yes, note) = (*yes, note.clone());
                    self.record_decision(yes, &note);
                }
                _ => {}
            }
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
            KeyCode::Char('r') => {
                let found = match self.screen {
                    Screen::Runs => self.select_receipt(),
                    Screen::Live if !self.demo => {
                        let id = self.live.run_id.clone();
                        self.receipt_for(&id)
                    }
                    _ => true,
                };
                if found || self.demo {
                    self.go(Screen::Receipt)
                } else {
                    self.status = Some("the receipt is written when the run ends".into());
                }
            }
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
                let Some(run) = self.runs.get(self.selected) else {
                    return;
                };
                let id = run.run_id.clone();
                if self.demo {
                    if run.is_finished() {
                        self.go(Screen::Receipt)
                    } else {
                        self.go(Screen::Live)
                    }
                } else if self.receipt_for(&id) {
                    self.go(Screen::Receipt)
                } else if let Some(l) = self.source.as_ref().and_then(|s| s.live(&id)) {
                    self.live = l;
                    self.go(Screen::Live)
                } else if self.live.run_id == id {
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
            KeyCode::Enter if self.live_done() => {
                let id = self.live.run_id.clone();
                if self.demo || self.receipt_for(&id) {
                    self.go(Screen::Receipt)
                }
            }
            KeyCode::Char('p') if self.demo => {
                self.status = Some("paused — p again to resume (demo)".into())
            }
            KeyCode::Char(c @ ('y' | 'n')) if self.live.waiting.is_some() => {
                let yes = c == 'y';
                let w = self.live.waiting.clone().unwrap();
                match w.ask {
                    // A tool the agent asked for: the answer is enough.
                    Some(ask) => {
                        let word = if yes { "allowed" } else { "denied" };
                        self.status = Some(match &self.source {
                            Some(src) => match src.answer(&self.live.run_id, ask, yes) {
                                Ok(()) => {
                                    format!("{}: {word} as {}; the run continues", w.stage, w.who)
                                }
                                Err(e) => format!("not recorded: {e}"),
                            },
                            None => format!("{}: {word} (demo)", w.stage),
                        });
                    }
                    // A stage's decision needs the note that says what was checked.
                    None => {
                        self.deciding = Some((yes, String::new()));
                        self.status = Some(format!(
                            "{}: type what you checked{}, then Enter · Esc to drop it",
                            if yes { "approving" } else { "rejecting" },
                            if yes { "" } else { ", or why not" }
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    fn record_decision(&mut self, yes: bool, note: &str) {
        let Some(w) = self.live.waiting.clone() else {
            self.deciding = None;
            return;
        };
        if note.trim().is_empty() {
            self.status = Some("a decision needs a note: what you checked, or why not".into());
            return;
        }
        let word = if yes { "approved" } else { "rejected" };
        self.status = Some(match &self.source {
            Some(src) => match src.decide(&self.live.run_id, &w.stage, yes, note) {
                Ok(()) => format!("{}: {word} as {}; the run continues", w.stage, w.who),
                Err(e) => format!("not recorded: {e}"),
            },
            None => format!("{}: {word} (demo): {note}", w.stage),
        });
        self.deciding = None;
    }

    fn receipt_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('h') => self.show_how = !self.show_how,
            KeyCode::Char('v') if self.demo => {
                self.status = Some("re-checked: the verdict reproduces from stored evidence".into())
            }
            _ => {}
        }
    }

    fn launch_key(&mut self, code: KeyCode) {
        let n = self.launch.workflows.len();
        match code {
            KeyCode::Down | KeyCode::Tab => self.launch.focus = (self.launch.focus + 1).min(1),
            KeyCode::Up | KeyCode::BackTab => {
                self.launch.focus = self.launch.focus.saturating_sub(1)
            }
            KeyCode::Left if self.launch.focus == 0 && n > 0 => {
                self.launch.workflow = (self.launch.workflow + n - 1) % n
            }
            KeyCode::Right if self.launch.focus == 0 && n > 0 => {
                self.launch.workflow = (self.launch.workflow + 1) % n
            }
            KeyCode::Enter if self.demo => {
                self.restart_live();
                self.go(Screen::Live);
            }
            KeyCode::Enter => self.start_run(),
            _ => {}
        }
    }

    /// Starts the chosen workflow on what was typed, and follows it on the live screen.
    fn start_run(&mut self) {
        let input = self.launch.spec.trim().to_owned();
        if input.is_empty() {
            self.status =
                Some("say what to work on: a ticket path, or the task in your words".into());
            self.launch.focus = 1;
            return;
        }
        let Some(workflow) = self.launch.workflows.get(self.launch.workflow).cloned() else {
            self.status =
                Some("no workflow under .conductor/workflows/; `conductor init` writes one".into());
            return;
        };
        let started = match &self.source {
            Some(src) => src.start(&workflow, &input),
            None => Err("starting runs is not available here".into()),
        };
        match started {
            Ok(id) => {
                let status = format!(
                    "started {id}{}",
                    if self.launch.herdr {
                        " in a herdr tab"
                    } else {
                        ""
                    }
                );
                self.live = LiveRun {
                    run_id: id,
                    work: input,
                    kind: String::new(),
                    herdr_tab: None,
                    stages: vec![],
                    checks: vec![],
                    note: Some("starting…".into()),
                    log: vec![],
                    panes: vec![],
                    ended: None,
                    pid: None,
                    waiting: None,
                };
                self.go(Screen::Live);
                self.status = Some(status);
            }
            Err(e) => self.status = Some(format!("not started: {e}")),
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
        press(&mut app, KeyCode::Char('4'));
        assert_eq!(app.screen, Screen::Receipt);
        press(&mut app, KeyCode::Char('1'));
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

    /// A source that starts runs and remembers what it was asked to start.
    struct Starter(std::cell::RefCell<Vec<(String, String)>>);
    impl RunSource for Starter {
        fn receipts(&self) -> Vec<Receipt> {
            vec![]
        }
        fn running(&self) -> Vec<LiveRun> {
            vec![]
        }
        fn live(&self, _id: &str) -> Option<LiveRun> {
            None
        }
        fn workflows(&self) -> Vec<String> {
            vec![
                ".conductor/workflows/bugfix.yaml".into(),
                ".conductor/workflows/ask.yaml".into(),
            ]
        }
        fn in_herdr(&self) -> bool {
            true
        }
        fn start(&self, workflow: &str, input: &str) -> Result<String, String> {
            self.0.borrow_mut().push((workflow.into(), input.into()));
            Ok("NEW1".into())
        }
    }

    #[test]
    fn the_launch_tab_starts_the_chosen_workflow_on_what_was_typed_and_follows_it() {
        let asked = std::rc::Rc::new(Starter(std::cell::RefCell::new(vec![])));
        struct Shared(std::rc::Rc<Starter>);
        impl RunSource for Shared {
            fn receipts(&self) -> Vec<Receipt> {
                self.0.receipts()
            }
            fn running(&self) -> Vec<LiveRun> {
                self.0.running()
            }
            fn live(&self, id: &str) -> Option<LiveRun> {
                self.0.live(id)
            }
            fn workflows(&self) -> Vec<String> {
                self.0.workflows()
            }
            fn in_herdr(&self) -> bool {
                self.0.in_herdr()
            }
            fn start(&self, w: &str, i: &str) -> Result<String, String> {
                self.0.start(w, i)
            }
        }
        let mut app = App::from_source(Box::new(Shared(asked.clone())));
        app.go(Screen::Launch);
        assert_eq!(app.launch.workflows.len(), 2);
        assert!(app.launch.herdr);
        // Nothing typed: nothing starts.
        press(&mut app, KeyCode::Enter);
        assert!(asked.0.borrow().is_empty());
        assert_eq!(app.launch.focus, 1);
        // Pick the second workflow, type the ticket, start.
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Down);
        for c in "tickets/BUG-102.md".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            asked.0.borrow().as_slice(),
            [(
                ".conductor/workflows/ask.yaml".to_string(),
                "tickets/BUG-102.md".to_string()
            )]
        );
        assert_eq!(app.screen, Screen::Live);
        assert_eq!(app.live.run_id, "NEW1");
        assert!(
            app.status
                .as_deref()
                .unwrap()
                .starts_with("started NEW1 in a herdr tab")
        );
    }

    #[test]
    fn real_runs_open_their_own_receipts() {
        let mut first = demo::receipt();
        first.run_id = "A".into();
        let mut second = demo::receipt();
        second.run_id = "B".into();
        second.checks[0].verdict = conductor_model::Verdict::Failed;
        let mut app = App::from_receipts(vec![first, second]);
        assert_eq!(app.runs[1].status, conductor_model::Verdict::Failed);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen, Screen::Receipt);
        assert_eq!(app.receipt.run_id, "B");
        assert!(app.needs_you.is_none());
    }

    struct Fixed(Vec<LiveRun>);

    impl RunSource for Fixed {
        fn receipts(&self) -> Vec<Receipt> {
            vec![demo::receipt()]
        }
        fn running(&self) -> Vec<LiveRun> {
            self.0.clone()
        }
        fn live(&self, id: &str) -> Option<LiveRun> {
            self.0.iter().find(|l| l.run_id == id).cloned()
        }
    }

    /// A source that remembers what was decided.
    struct Deciding(LiveRun, std::cell::RefCell<Vec<(String, bool, String)>>);

    impl RunSource for Deciding {
        fn receipts(&self) -> Vec<Receipt> {
            vec![]
        }
        fn running(&self) -> Vec<LiveRun> {
            vec![self.0.clone()]
        }
        fn live(&self, _id: &str) -> Option<LiveRun> {
            Some(self.0.clone())
        }
        fn decide(&self, _run: &str, stage: &str, yes: bool, note: &str) -> Result<(), String> {
            self.1.borrow_mut().push((stage.into(), yes, note.into()));
            Ok(())
        }
    }

    #[test]
    fn a_decision_is_typed_with_its_note_before_it_is_recorded() {
        let mut l = demo::live();
        l.run_id = "W1".into();
        l.waiting = Some(conductor_model::view::Waiting {
            stage: "review".into(),
            who: "PO".into(),
            question: "Approve if AC1 is shown.".into(),
            since: "2026-09-25T00:00:00Z".into(),
            ask: None,
        });
        let src = std::rc::Rc::new(Deciding(l, std::cell::RefCell::new(vec![])));
        struct Shared(std::rc::Rc<Deciding>);
        impl RunSource for Shared {
            fn receipts(&self) -> Vec<Receipt> {
                self.0.receipts()
            }
            fn running(&self) -> Vec<LiveRun> {
                self.0.running()
            }
            fn live(&self, id: &str) -> Option<LiveRun> {
                self.0.live(id)
            }
            fn decide(&self, r: &str, s: &str, y: bool, n: &str) -> Result<(), String> {
                self.0.decide(r, s, y, n)
            }
        }
        let mut app = App::from_source(Box::new(Shared(src.clone())));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen, Screen::Live);
        // `y` alone records nothing: the note is required.
        press(&mut app, KeyCode::Char('y'));
        assert!(app.deciding.is_some());
        press(&mut app, KeyCode::Enter);
        assert!(src.1.borrow().is_empty());
        assert!(app.status.as_deref().unwrap().contains("needs a note"));
        for c in "AC1 shown".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            src.1.borrow().as_slice(),
            [("review".to_string(), true, "AC1 shown".to_string())]
        );
        assert!(app.deciding.is_none());
        assert!(app.status.as_deref().unwrap().contains("approved as PO"));
        // Esc drops a decision being typed.
        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Esc);
        assert!(app.deciding.is_none());
        assert_eq!(src.1.borrow().len(), 1);
    }

    #[test]
    fn a_run_that_never_reached_a_check_is_folded_under_the_list() {
        let mut ok = demo::receipt();
        ok.run_id = "OK1".into();
        let mut halted = demo::receipt();
        halted.run_id = "HALT1".into();
        halted.checks.clear();
        halted.survivors.clear();
        let app = App::from_receipts(vec![halted, ok]);
        assert_eq!(app.runs.len(), 1);
        assert_eq!(app.runs[0].run_id, "OK1");
        assert_eq!(app.unchecked, vec!["HALT1".to_string()]);
        assert!(app.stats[0].0.starts_with('2'));
    }

    #[test]
    fn a_run_in_progress_is_listed_first_and_opens_live() {
        let mut l = demo::live();
        l.run_id = "LIVE1".into();
        let mut app = App::from_source(Box::new(Fixed(vec![l])));
        assert_eq!(app.runs[0].run_id, "LIVE1");
        assert_eq!(app.runs[0].status, conductor_model::Verdict::Running);
        assert!(app.stats[0].0.contains("1 running"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.screen, Screen::Live);
        assert_eq!(app.live.run_id, "LIVE1");
        // No receipt yet: `r` says so instead of showing another run's.
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.screen, Screen::Live);
        assert!(app.status.as_deref().unwrap().contains("when the run ends"));
    }

    #[test]
    fn ticks_keep_the_selection_on_the_same_run() {
        let mut a = demo::live();
        a.run_id = "A".into();
        let mut app = App::from_source(Box::new(Fixed(vec![a])));
        press(&mut app, KeyCode::Down);
        let chosen = app.runs[app.selected].run_id.clone();
        app.tick();
        assert_eq!(app.runs[app.selected].run_id, chosen);
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
