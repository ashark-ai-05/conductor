//! When a `RunSource` computes its own headline numbers (from run metrics, for a real
//! repository), `App` must show those instead of deriving them from receipts.

use conductor_model::view::LiveRun;
use conductor_model::{Receipt, demo};
use conductor_tui::app::{App, RunSource};
use std::cell::RefCell;
use std::rc::Rc;

fn engine_stats() -> [(String, String); 3] {
    [
        (
            "1 of 1 passed".to_string(),
            "runs recorded in this repository".to_string(),
        ),
        (
            "33% caught".to_string(),
            "the agent said done, a check said no".to_string(),
        ),
        (
            "$0.44".to_string(),
            "672k tokens across all runs".to_string(),
        ),
    ]
}

struct WithStats(Option<[(String, String); 3]>);

impl RunSource for WithStats {
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

#[test]
fn from_source_prefers_the_sources_own_stats_over_receipt_derived_ones() {
    let app = App::from_source(Box::new(WithStats(Some(engine_stats()))));
    assert_eq!(app.stats, engine_stats());
}

#[test]
fn tick_keeps_showing_the_sources_stats() {
    let mut app = App::from_source(Box::new(WithStats(Some(engine_stats()))));
    app.tick();
    app.tick();
    assert_eq!(app.stats, engine_stats());
}

struct Changing(Rc<RefCell<u32>>);

impl RunSource for Changing {
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
        let n = *self.0.borrow();
        Some([
            (
                format!("{n} of {n} passed"),
                "runs recorded in this repository".to_string(),
            ),
            (
                "0% caught".to_string(),
                "the agent said done, a check said no".to_string(),
            ),
            (
                "$0.00".to_string(),
                "0 tokens across all runs".to_string(),
            ),
        ])
    }
}

#[test]
fn tick_re_reads_the_sources_stats_rather_than_caching_them() {
    let shared = Rc::new(RefCell::new(1));
    let mut app = App::from_source(Box::new(Changing(shared.clone())));
    assert_eq!(app.stats[0].0, "1 of 1 passed");
    *shared.borrow_mut() = 2;
    app.tick();
    assert_eq!(app.stats[0].0, "2 of 2 passed");
}
