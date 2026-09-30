//! User-facing commands for discovering and retaining native operations.
use anyhow::{Context, Result, bail};
use conductor_engine::capabilities as native;
use std::{path::Path, process::ExitCode};

pub fn dispatch(repo: &Path, args: &[String]) -> Result<ExitCode> {
    let usage = "conductor capability list | add <id> <file> [--label <title>] | remove <id> | run <id> | refresh <task-id> | show <task-id>";
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => {
            for c in native::Registry::load(repo)
                .map_err(anyhow::Error::msg)?
                .available(repo)
            {
                println!(
                    "{}  {}\n  {}",
                    c.binding.id,
                    c.label,
                    c.unavailable.unwrap_or(c.detail)
                );
            }
        }
        Some("add") if args.len() == 3 || (args.len() == 5 && args[3] == "--label") => {
            let label = args.get(4).unwrap_or(&args[1]);
            let d = native::add_report(repo, &args[1], label, Path::new(&args[2]))
                .map_err(anyhow::Error::msg)?;
            println!(
                "Registered {}. Open Actions with Ctrl+K in Conductor, or run `conductor capability run {}`.",
                d.id, d.id
            );
        }
        Some("remove") if args.len() == 2 => {
            native::remove(repo, &args[1]).map_err(anyhow::Error::msg)?;
            println!("Removed {}. Existing results are retained.", args[1]);
        }
        Some("run" | "refresh") if args.len() == 2 => {
            let parent = (args[0] == "refresh").then_some(args[1].as_str());
            let binding = if let Some(id) = parent {
                native::read(repo, id).map_err(anyhow::Error::msg)?.binding
            } else {
                native::Registry::load(repo)
                    .map_err(anyhow::Error::msg)?
                    .available(repo)
                    .into_iter()
                    .find(|c| c.binding.id == args[1])
                    .context("Capability not found. Use conductor capability list.")?
                    .binding
            };
            let id = native::invoke(repo, &binding, parent).map_err(anyhow::Error::msg)?;
            println!("{id}\nOpen with: conductor ui --run {id}");
            let p = native::read(repo, &id)
                .map_err(anyhow::Error::msg)?
                .project()
                .map_err(anyhow::Error::msg)?;
            if let Some(e) = p.error {
                eprintln!("Read failed: {e}. The attempt and previous output are retained.");
                return Ok(ExitCode::FAILURE);
            }
        }
        Some("show") if args.len() == 2 => {
            let record = native::read(repo, &args[1]).map_err(anyhow::Error::msg)?;
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        _ => bail!("{usage}"),
    }
    Ok(ExitCode::SUCCESS)
}
