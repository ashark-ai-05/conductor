//! The runs screen's headline numbers, when a `RunSource` can compute them from run metrics
//! instead of from receipts alone (`App::stats`, driven by `RunSource::stats`).

use conductor_model::view::LiveRun;
use conductor_model::{Receipt, demo};
use conductor_tui::app::{App, RunSource};

/// A source whose receipts would, on their own, produce different headline numbers than the
/// metrics-based ones it reports through `stats()`.
struct MetricsBacked {
    receipts: Vec<Receipt>,
    stats: [(String, String); 3],
}

impl RunSource for MetricsBacked {
    fn receipts(&self) -> Vec<Receipt> {
        self.receipts.clone()
    }
    fn running(&self) -> Vec<LiveRun> {
        vec![]
    }
    fn live(&self, _run_id: &str) -> Option<LiveRun> {
        None
    }
    fn stats(&self) -> Option<[(String, String); 3]> {
        Some(self.stats.clone())
    }
}

fn metrics_stats() -> [(String, String); 3] {
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

#[test]
fn a_source_with_metrics_stats_is_used_over_receipt_derived_numbers() {
    let source = MetricsBacked {
        receipts: vec![demo::receipt()],
        stats: metrics_stats(),
    };
    let app = App::from_source(Box::new(source));
    assert_eq!(app.stats, metrics_stats());
}

#[test]
fn ticking_keeps_the_sources_metrics_stats() {
    let source = MetricsBacked {
        receipts: vec![demo::receipt()],
        stats: metrics_stats(),
    };
    let mut app = App::from_source(Box::new(source));
    app.tick();
    assert_eq!(app.stats, metrics_stats());
    app.tick();
    assert_eq!(app.stats, metrics_stats());
}

/// A source with two receipts whose metrics-based stats change between ticks, the way a
/// repository's runs grow while the UI is open.
struct Growing(std::cell::Cell<u32>);

impl RunSource for Growing {
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
        let n = self.0.get();
        self.0.set(n + 1);
        Some([
            (format!("{n} of {n} passed"), "runs recorded in this repository".to_string()),
            ("0% caught".to_string(), "the agent said done, a check said no".to_string()),
            ("$0.00".to_string(), "0 tokens across all runs".to_string()),
        ])
    }
}

#[test]
fn each_tick_re_reads_the_sources_metrics_stats() {
    let mut app = App::from_source(Box::new(Growing(std::cell::Cell::new(1))));
    assert_eq!(app.stats[0].0, "1 of 1 passed");
    app.tick();
    assert_eq!(app.stats[0].0, "2 of 2 passed");
    app.tick();
    assert_eq!(app.stats[0].0, "3 of 3 passed");
}
