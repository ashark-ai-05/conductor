//! `conductor`: the command line.

use anyhow::{Context, Result, bail};
use conductor_engine::{Options, list_runs, read_receipt, verify};
use conductor_model::{Event, Receipt, Verdict, Workflow};
use conductor_tui::{app::App, theme::Theme};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
conductor — agentic software work with receipts

usage:
  conductor run <workflow.yaml> (--spec <file> | -m <text>) [--base <rev>]
                                     run a workflow headless and write its receipt
  conductor receipt [<run-id>]       print a run's receipt (the latest by default)
  conductor verify <run-id>          re-check a run's record with no model calls
  conductor ui [--demo] [--light]    open the terminal UI over this repository's runs
  conductor validate <workflow.yaml> check a workflow before running it

The workflow is read from the base commit (HEAD by default), never from your
working tree, and the run happens in its own git worktree and branch.";

fn main() -> ExitCode {
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
        Some("receipt") => receipt(&args[1..]),
        Some("verify") => verify_cmd(&args[1..]),
        Some("ui") => ui(&args[1..]),
        Some("validate") => validate(&args[1..]),
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
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--spec" => {
                let f = it.next().context("--spec needs a file")?;
                task = Some(std::fs::read_to_string(f).with_context(|| format!("reading {f}"))?);
            }
            "-m" => task = Some(it.next().context("-m needs text")?.clone()),
            "--base" => base = it.next().context("--base needs a revision")?.clone(),
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
    Ok(if outcome.verdict == Verdict::Passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
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

fn ui(args: &[String]) -> Result<ExitCode> {
    let mut demo = false;
    let mut theme = Theme::DARK;
    for a in args {
        match a.as_str() {
            "--demo" => demo = true,
            "--light" => theme = Theme::LIGHT,
            "--dark" => theme = Theme::DARK,
            other => bail!("unknown option `{other}` for `conductor ui`"),
        }
    }
    let app = if demo {
        App::demo()
    } else {
        let repo = repo_root()?;
        let receipts: Vec<Receipt> = list_runs(&repo)
            .iter()
            .filter_map(|id| read_receipt(&repo, id).ok())
            .collect();
        if receipts.is_empty() {
            bail!(
                "no runs recorded in this repository yet; start one with `conductor run`, or try `conductor ui --demo`"
            );
        }
        App::from_receipts(receipts)
    };
    conductor_tui::run(app, theme)?;
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
