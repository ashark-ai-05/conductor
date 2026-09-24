//! When a `RunSource` reports its own headline numbers, `App` shows those instead of the
//! receipt-derived ones, both on construction and after every tick. Sources that don't
//! override `stats()` keep today's behaviour.

use conductor_model::view::LiveRun;
use conductor_model::{Receipt, demo};
use conductor_tui::app::{App, RunSource};
use std::cell::Cell;
use std::rc::Rc;

fn metric_stats() -> [(String, String); 3] {
    [
        (
            "9 of 10 passed".into(),
            "runs recorded in this repository".into(),
        ),
        (
            "42% caught".into(),
            "the agent said done, a check said no".into(),
        ),
        ("$3.21".into(), "12k tokens across all runs".into()),
    ]
}

struct Fixed(Option<[(String, String); 3]>);

impl RunSource for Fixed {
    fn receipts(&self) -> Vec<Receipt> {
        vec![demo::receipt()]
    }
    fn running(&self) -> Vec<LiveRun> {
        vec![]
    }
    fn live(&self, _run_id: &str) -> Option<LiveRun> {
        None
    }
    fn stats(&self) -> Option<[(String, String); 3]> {
        self.0.clone()
    }
}

/// A source whose `stats()` starts at `None` and switches to `Some` once told to, so the same
/// `App` can be observed across a tick that changes what the source reports.
struct Toggle(Rc<Cell<bool>>);

impl RunSource for Toggle {
    fn receipts(&self) -> Vec<Receipt> {
        vec![demo::receipt()]
    }
    fn running(&self) -> Vec<LiveRun> {
        vec![]
    }
    fn live(&self, _run_id: &str) -> Option<LiveRun> {
        None
    }
    fn stats(&self) -> Option<[(String, String); 3]> {
        self.0.get().then(metric_stats)
    }
}

/// A source that implements only the required methods, leaving `stats()` at its default.
struct Bare;

impl RunSource for Bare {
    fn receipts(&self) -> Vec<Receipt> {
        vec![]
    }
    fn running(&self) -> Vec<LiveRun> {
        vec![]
    }
    fn live(&self, _run_id: &str) -> Option<LiveRun> {
        None
    }
}

#[test]
fn the_default_stats_implementation_is_none() {
    assert_eq!(Bare.stats(), None);
}

#[test]
fn from_source_prefers_the_sources_own_stats_when_present() {
    let app = App::from_source(Box::new(Fixed(Some(metric_stats()))));
    assert_eq!(app.stats, metric_stats());
}

#[test]
fn tick_switches_to_metric_stats_once_the_source_starts_reporting_them() {
    let flag = Rc::new(Cell::new(false));
    let mut app = App::from_source(Box::new(Toggle(flag.clone())));
    assert_ne!(app.stats, metric_stats(), "not opted in yet");

    flag.set(true);
    app.tick();
    assert_eq!(app.stats, metric_stats());
}

#[test]
fn tick_falls_back_again_once_the_source_stops_reporting_metric_stats() {
    let flag = Rc::new(Cell::new(true));
    let mut app = App::from_source(Box::new(Toggle(flag.clone())));
    assert_eq!(app.stats, metric_stats());

    flag.set(false);
    app.tick();
    assert_ne!(
        app.stats,
        metric_stats(),
        "the source withdrew its own numbers; App must fall back to receipt-derived ones"
    );
}
