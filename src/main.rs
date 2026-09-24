//! `conductor`: the command line.

use anyhow::{Context, Result, bail};
use conductor_engine::{Mode, Options, list_runs, read_receipt, verify};
use conductor_model::{Event, Receipt, Verdict, Workflow};
use conductor_tui::{app::App, theme::Theme};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod setup;

const USAGE: &str = "\
conductor — agentic software work with receipts

usage:
  conductor init                     set up this repository: a starter workflow, prompts,
                                     policy and a task file
  conductor doctor                   check this repository and machine are ready to run
  conductor run <workflow.yaml> (--spec <file> | -m <text>) [--base <rev>]
                [--executor herdr|headless]
                                     run a workflow and write its receipt; in herdr
                                     each run gets a tab and each stage a pane
  conductor receipt [<run-id>]       print a run's receipt (the latest by default)
  conductor verify <run-id>          re-check a run's record with no model calls
  conductor stats                    what this repository's runs add up to: pass rate,
                                     how often checks caught an agent, retries, spend
  conductor trace [<run-id>] [--export]
                                     show a run's stages, attempts and checks as a
                                     trace; --export sends it over OTLP
  conductor ui [--run <id>] [--demo] [--light]
                                     open the terminal UI over this repository's runs,
                                     including ones in progress
  conductor validate <workflow.yaml> check a workflow before running it
  conductor pane split [--from <pane>] [--down] | run <pane> <command> |
                 read <pane> [--lines N] | close <pane> | list
                                     for agents inside a run: panes in the run's tab

The workflow is read from the base commit (HEAD by default), never from your
working tree, and the run happens in its own git worktree and branch.";

fn main() -> ExitCode {
    // `conductor trace | head` closes the pipe early: that's the reader being done, not a
    // crash, so leave quietly instead of printing a panic.
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("");
        if msg.contains("Broken pipe") {
            std::process::exit(0);
        }
        default(info);
    }));
    match real_main() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("conductor: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn real_main() -> Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("init") => setup::init(&repo_root()?),
        Some("doctor") => setup::doctor(&repo_root()?, herdr_handle()),
        Some("receipt") => receipt(&args[1..]),
        Some("verify") => verify_cmd(&args[1..]),
        Some("trace") => trace_cmd(&args[1..]),
        Some("stats") => stats_cmd(&args[1..]),
        Some("ui") => ui(&args[1..]),
        Some("validate") => validate(&args[1..]),
        Some("pane") => pane(&args[1..]),
        Some("-h" | "--help" | "help") | None => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        Some(other) => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

/// The repository root containing the current directory.
fn repo_root() -> Result<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!("run conductor inside a git repository");
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

fn relative_to(repo: &Path, path: &str) -> Result<String> {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| repo.join(path));
    let repo = std::fs::canonicalize(repo)?;
    Ok(abs
        .strip_prefix(&repo)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.to_owned()))
}

fn run(args: &[String]) -> Result<ExitCode> {
    let mut workflow = None;
    let mut task = None;
    let mut base = "HEAD".to_string();
    let mut executor: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--spec" => {
                let f = it.next().context("--spec needs a file")?;
                task = Some(std::fs::read_to_string(f).with_context(|| format!("reading {f}"))?);
            }
            "-m" => task = Some(it.next().context("-m needs text")?.clone()),
            "--base" => base = it.next().context("--base needs a revision")?.clone(),
            "--executor" => {
                executor = Some(
                    it.next()
                        .context("--executor needs herdr or headless")?
                        .clone(),
                )
            }
            other if other.starts_with('-') => {
                bail!("unknown option `{other}` for `conductor run`")
            }
            other => workflow = Some(other.to_owned()),
        }
    }
    let Some(workflow) = workflow else {
        bail!("usage: conductor run <workflow.yaml> (--spec <file> | -m <text>)")
    };
    let Some(task) = task else {
        bail!("say what to build with --spec <file> or -m <text>")
    };
    let repo = repo_root()?;
    let workflow = relative_to(&repo, &workflow)?;

    // Inside a herdr pane and not in CI, run in herdr; otherwise headless.
    let in_herdr =
        std::env::var("HERDR_ENV").as_deref() == Ok("1") && std::env::var_os("CI").is_none();
    let mode = match executor.as_deref() {
        Some("herdr") => Mode::Herdr(herdr_handle()),
        Some("headless") => Mode::Headless,
        Some(other) => bail!("unknown executor `{other}`; use herdr or headless"),
        None if in_herdr => Mode::Herdr(herdr_handle()),
        None => Mode::Headless,
    };

    let (tx, rx) = std::sync::mpsc::channel::<Event>();
    let printer = std::thread::spawn(move || {
        for e in rx {
            let stage = e
                .stage
                .as_deref()
                .map(|s| format!("{s:<10}"))
                .unwrap_or_else(|| " ".repeat(10));
            println!(
                "  {}  {stage} {:<10} {}",
                &e.at[11..19.min(e.at.len())],
                e.source.label(),
                e.what
            );
        }
    });
    println!("conductor · {workflow} at {base}\n");
    let outcome = conductor_engine::run(Options {
        repo: repo.clone(),
        workflow,
        base,
        task,
        watcher: Some(tx),
        home: None,
        status_ui: matches!(mode, Mode::Herdr(_))
            .then(|| std::env::current_exe().ok())
            .flatten(),
        mode,
    });
    let _ = printer.join();
    let outcome = outcome?;
    println!();
    print_receipt(&outcome.receipt);
    println!(
        "\n  worktree  {}\n  branch    {}",
        outcome.worktree.display(),
        outcome.branch
    );
    match &outcome.export {
        Some(Ok(id)) => println!("  exported  trace {id} over OTLP"),
        Some(Err(e)) => eprintln!("  warning   the OTLP export failed: {e}"),
        None => {}
    }
    Ok(if outcome.verdict == Verdict::Passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The herdr to drive: `$CONDUCTOR_HERDR_BIN` (default `herdr`), in the session this
/// process runs in, or `$CONDUCTOR_HERDR_SESSION` when set.
fn herdr_handle() -> conductor_herdr::Herdr {
    let bin = std::env::var("CONDUCTOR_HERDR_BIN").unwrap_or_else(|_| "herdr".into());
    let target = match std::env::var("CONDUCTOR_HERDR_SESSION") {
        Ok(s) if !s.is_empty() => conductor_herdr::Target::Session(s),
        _ => conductor_herdr::Target::Current,
    };
    conductor_herdr::Herdr::with(bin, target)
}

fn print_receipt(r: &Receipt) {
    let v = r.verdict();
    println!("{}  {} · {}", v.word().to_uppercase(), r.work, r.run_id);
    println!("\nWhat was proven");
    for c in &r.checks {
        println!("  {}  {:<52} {}", c.verdict.glyph(), c.claim, c.detail);
    }
    if !r.survivors.is_empty() {
        println!("\nLook here first — bugs the tests missed");
        for s in &r.survivors {
            println!("  {:<18} {}", s.at, s.change);
        }
    }
    println!("\nNot checked");
    for n in &r.not_checked {
        println!("  !  {n}");
    }
    println!("\nHow it ran");
    for (k, v) in &r.how {
        println!("  {k:<14} {v}");
    }
    let i = &r.integrity;
    println!(
        "  {:<14} chain {} · anchored in {}",
        "record",
        i.chain_head,
        i.anchored_in.as_deref().unwrap_or("—")
    );
}

fn receipt(args: &[String]) -> Result<ExitCode> {
    let repo = repo_root()?;
    let id = match args.first() {
        Some(id) => id.clone(),
        None => list_runs(&repo)
            .into_iter()
            .next()
            .context("no runs recorded in this repository yet")?,
    };
    let r = read_receipt(&repo, &id).map_err(anyhow::Error::msg)?;
    print_receipt(&r);
    Ok(ExitCode::SUCCESS)
}

fn stats_cmd(args: &[String]) -> Result<ExitCode> {
    if let Some(a) = args.first() {
        bail!("unknown option `{a}` for `conductor stats`");
    }
    let repo = repo_root()?;
    let ids = list_runs(&repo);
    if ids.is_empty() {
        bail!("no runs recorded in this repository yet");
    }
    let mut all = Vec::new();
    for id in &ids {
        let dir = conductor_engine::store::RunDir::for_run(&repo, id);
        if let Ok(events) = dir.read_events() {
            let t = conductor_engine::trace::build(id, &events);
            conductor_engine::metrics::merge(
                &mut all,
                conductor_engine::metrics::collect(&t, &events),
            );
        }
    }
    let plural = if ids.len() == 1 { "" } else { "s" };
    println!("conductor stats · {} run{plural}\n", ids.len());
    print!("{}", conductor_engine::metrics::summary(&all));
    Ok(ExitCode::SUCCESS)
}

fn trace_cmd(args: &[String]) -> Result<ExitCode> {
    let export = args.iter().any(|a| a == "--export");
    if let Some(bad) = args.iter().find(|a| a.starts_with('-') && *a != "--export") {
        bail!("unknown option `{bad}` for `conductor trace`");
    }
    let repo = repo_root()?;
    let id = match args.iter().find(|a| !a.starts_with('-')) {
        Some(id) => id.clone(),
        None => list_runs(&repo)
            .into_iter()
            .next()
            .context("no runs recorded in this repository yet")?,
    };
    let dir = conductor_engine::store::RunDir::for_run(&repo, &id);
    let events = dir
        .read_events()
        .with_context(|| format!("no record for run {id}"))?;
    print!(
        "{}",
        conductor_engine::trace::render(&conductor_engine::trace::build(&id, &events))
    );
    if export {
        let cfg = conductor_engine::otlp::Config::from_env().context(
            "set CONDUCTOR_OTLP_ENDPOINT (or OTEL_EXPORTER_OTLP_ENDPOINT) to export, e.g. http://localhost:4318",
        )?;
        let tid =
            conductor_engine::otlp::export(&cfg, &id, &events, dir.receipt_sha256().as_deref())
                .map_err(anyhow::Error::msg)?;
        println!("\nexported trace {tid} to {}", cfg.endpoint);
    }
    Ok(ExitCode::SUCCESS)
}

fn verify_cmd(args: &[String]) -> Result<ExitCode> {
    let [id] = args else {
        bail!("usage: conductor verify <run-id>")
    };
    let repo = repo_root()?;
    let v = verify(&repo, id);
    println!(
        "chain     {}",
        if v.chain_intact { "intact" } else { "BROKEN" }
    );
    println!(
        "anchored  {}",
        if v.anchored {
            "yes, the commit trailer matches"
        } else {
            "NO"
        }
    );
    if let Some(verdict) = v.verdict {
        println!(
            "verdict   {} (recomputed from the receipt's rows)",
            verdict.word()
        );
    }
    for p in &v.problems {
        println!("problem   {p}");
    }
    Ok(if v.ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The repository's runs, read fresh on every UI tick.
struct RepoRuns(PathBuf);

impl conductor_tui::app::RunSource for RepoRuns {
    fn receipts(&self) -> Vec<Receipt> {
        list_runs(&self.0)
            .iter()
            .filter_map(|id| read_receipt(&self.0, id).ok())
            .collect()
    }
    fn running(&self) -> Vec<conductor_model::view::LiveRun> {
        conductor_engine::live::running(&self.0)
    }
    fn live(&self, run_id: &str) -> Option<conductor_model::view::LiveRun> {
        conductor_engine::live::read(&self.0, run_id)
    }
}

fn ui(args: &[String]) -> Result<ExitCode> {
    let mut demo = false;
    let mut theme = Theme::DARK;
    let mut run_id: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--demo" => demo = true,
            "--light" => theme = Theme::LIGHT,
            "--dark" => theme = Theme::DARK,
            "--run" => run_id = Some(it.next().context("--run needs a run id")?.clone()),
            other => bail!("unknown option `{other}` for `conductor ui`"),
        }
    }
    let app = if demo {
        App::demo()
    } else {
        let repo = repo_root()?;
        let src = RepoRuns(repo.clone());
        use conductor_tui::app::RunSource;
        if src.receipts().is_empty() && src.running().is_empty() && run_id.is_none() {
            bail!(
                "no runs recorded in this repository yet; start one with `conductor run`, or try `conductor ui --demo`"
            );
        }
        let live = match &run_id {
            Some(id) => Some(
                src.live(id)
                    .with_context(|| format!("no run `{id}` in this repository"))?,
            ),
            None => None,
        };
        let mut app = App::from_source(Box::new(src));
        if let Some(l) = live {
            app.live = l;
            app.go(conductor_tui::app::Screen::Live);
        }
        app
    };
    conductor_tui::run(app, theme)?;
    Ok(ExitCode::SUCCESS)
}

/// `conductor pane …`, for an agent inside a run in herdr.
fn pane(args: &[String]) -> Result<ExitCode> {
    let g = conductor_engine::agent_panes::Grant::from_env(herdr_handle())
        .map_err(anyhow::Error::msg)?;
    let usage = "usage: conductor pane split [--from <pane>] [--down] | run <pane> <command> | read <pane> [--lines N] | close <pane> | list";
    match args.first().map(String::as_str) {
        Some("split") => {
            let mut from = None;
            let mut down = false;
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--from" => from = Some(it.next().context("--from needs a pane id")?.clone()),
                    "--down" => down = true,
                    "--right" => down = false,
                    other => bail!("unknown option `{other}`\n{usage}"),
                }
            }
            let cwd = std::env::current_dir()?;
            let p = g
                .split(from.as_deref(), down, &cwd)
                .map_err(anyhow::Error::msg)?;
            println!("{p}");
        }
        Some("run") => {
            let [_, pane, cmd @ ..] = args else {
                bail!("{usage}")
            };
            if cmd.is_empty() {
                bail!("{usage}");
            }
            g.run(pane, &cmd.join(" ")).map_err(anyhow::Error::msg)?;
        }
        Some("read") => {
            let (pane, lines) = match args {
                [_, pane] => (pane, 80),
                [_, pane, flag, n] if flag == "--lines" => {
                    (pane, n.parse().context("--lines needs a number")?)
                }
                _ => bail!("{usage}"),
            };
            print!("{}", g.read(pane, lines).map_err(anyhow::Error::msg)?);
        }
        Some("close") => {
            let [_, pane] = args else { bail!("{usage}") };
            g.close(pane).map_err(anyhow::Error::msg)?;
        }
        Some("list") => {
            for p in g.list().map_err(anyhow::Error::msg)? {
                println!(
                    "{}  {}  {}",
                    p.pane_id,
                    p.agent_status.unwrap_or_default(),
                    p.cwd.unwrap_or_default()
                );
            }
        }
        _ => bail!("{usage}"),
    }
    Ok(ExitCode::SUCCESS)
}

fn validate(args: &[String]) -> Result<ExitCode> {
    let [path] = args else {
        bail!("usage: conductor validate <workflow.yaml>")
    };
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
    let wf = Workflow::parse(&text).with_context(|| format!("{path} is not a valid workflow"))?;
    let report = wf.validate();
    for w in &report.warnings {
        println!("warning  {}: {}", w.at, w.message);
    }
    for e in &report.errors {
        println!("error    {}: {}", e.at, e.message);
    }
    if report.is_ok() {
        println!("ok       {path}: `{}`, {} stages", wf.id, wf.stages.len());
        Ok(ExitCode::SUCCESS)
    } else {
        println!("{} error(s) in {path}", report.errors.len());
        Ok(ExitCode::FAILURE)
    }
}
