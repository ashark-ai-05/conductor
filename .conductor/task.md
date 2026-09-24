# Runs-screen headline numbers from run metrics

The runs screen of `conductor ui` shows three headline numbers (`App::stats` in
crates/conductor-tui/src/app.rs). Today `App::set_runs` derives them from receipts only.
Make them come from the run metrics instead, so they show how often checks caught an agent.

1. In crates/conductor-engine, add
   `pub fn headline_stats(repo: &std::path::Path) -> Option<[(String, String); 3]>` to
   `conductor_engine::metrics`. It reads every run recorded in the repository (the run ids
   from `conductor_engine::list_runs`, each run's events from
   `conductor_engine::store::RunDir::for_run(repo, id).read_events()`), builds each run's
   trace with `conductor_engine::trace::build`, collects its metrics with
   `metrics::collect`, and adds them up with `metrics::merge`. It returns `None` when the
   repository has no recorded runs. Otherwise, in this order:
   - `("{passed} of {runs} passed", "runs recorded in this repository")`
   - `("{pct}% caught", "the agent said done, a check said no")`, where `pct` is
     `conductor.catches` divided by finished attempts (`conductor.attempts` with outcome
     `passed` or `check_failed`), times 100, rounded to the nearest whole number. Use
     `"— caught"` when there are no finished attempts.
   - `("${cost:.2}", "{tokens} tokens across all runs")`, with the summed `conductor.cost`
     and `conductor.tokens`. Format tokens the way `conductor stats` does: `672k`, `2.6M`,
     or the plain number below 1000.

   For the recorded run in docs/examples/live-run-durations/events.jsonl (as the only run in
   a repository), that is `("1 of 1 passed", …)`, `("33% caught", …)` and
   `("$0.44", "672k tokens across all runs")`.

2. In crates/conductor-tui/src/app.rs, add `fn stats(&self) -> Option<[(String, String); 3]>`
   to the `RunSource` trait with a default body returning `None`, so existing sources keep
   compiling. When an app built with `App::from_source` has a source whose `stats()` returns
   `Some`, `from_source` and every `tick` use those three pairs for `App::stats`. When it
   returns `None`, keep today's receipt-derived numbers.

3. In src/main.rs, `RepoRuns` implements `stats` by calling
   `conductor_engine::metrics::headline_stats` on its repository.
