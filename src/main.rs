//! `conductor`: the command line.

use anyhow::{Context, Result, bail};
use conductor_model::Workflow;
use conductor_tui::{app::App, theme::Theme};
use std::process::ExitCode;

const USAGE: &str = "\
conductor — agentic software work with receipts

usage:
  conductor ui --demo [--light]      open the terminal UI with sample data
  conductor validate <workflow.yaml> check a workflow before running it

Running workflows arrives next; this build ships the UI and workflow checks.";

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
        Some("ui") => ui(&args[1..]),
        Some("validate") => validate(&args[1..]),
        Some("-h" | "--help" | "help") | None => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        Some(other) => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
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
    if !demo {
        bail!("there are no runs to show yet; try `conductor ui --demo`");
    }
    conductor_tui::run(App::demo(), theme)?;
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
