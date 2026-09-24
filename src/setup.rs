//! `conductor init` and `conductor doctor`: getting a repository ready, and checking it is.

use anyhow::{Result, bail};
use conductor_model::Workflow;
use conductor_model::workflow::Gate;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, ExitCode};

const WORKFLOW: &str = r#"# One agent writes failing tests for the work; another makes them pass without being able
# to touch them. Every check is run by conductor, not by the agents.
#
# Conductor reads this file from the commit a run starts at, never from your working tree:
# commit changes to it before they take effect.
id: build
version: 1
kind: build

defaults:
  # A failed check is sent back to the agent; the second retry starts from a clean tree.
  retries: { max: 2, ladder: [in_context, fresh] }

budget:
  max_stage_wall_clock_sec: 900
  max_mutation_wall_clock_sec: 600

# Turn a passed run into a draft pull request with its receipt (`conductor deliver`).
# deliver: { base: main }

stages:
  - id: tests
    agent:
      kind: claude
      allowed_tools: ["Bash(cargo test:*)", "Bash(cargo build:*)"]
    prompt_file: .conductor/prompts/tests.md
    # Tests, plus `todo!()` stubs in src/ so tests of new code compile.
    scope: { write: ["tests/**", "src/**"] }
    gates:
      - { type: scope }
      # The new tests must compile, and every one of them must fail for the right reason
      # (an assertion or a `todo!()`, not a trivially false test). A stub that quietly
      # implements the work would make some pass, and fail this check.
      - type: command_assert
        command: ["cargo", "test", "--no-fail-fast", "--message-format", "json"]
        parser: cargo_json
        assert:
          - "compiled == true"
          - "tests_new > 0"
          - "tests_failed == tests_new"
          - "failures.kind all != 'trivial'"

  - id: implement
    depends_on: [tests]
    agent:
      kind: claude
      allowed_tools: ["Bash(cargo test:*)", "Bash(cargo build:*)"]
    prompt_file: .conductor/prompts/implement.md
    scope:
      write: ["src/**"]
      frozen: ["tests/**"]      # the tests from the stage above are locked
    gates:
      - { type: scope }
      - type: command_assert
        command: ["cargo", "test", "--message-format", "json"]
        parser: cargo_json
        reruns: 1               # passing twice rules out a lucky run
        # tests_new == 0: the implementer adds no tests of its own, even inside src/.
        assert: ["tests_failed == 0", "tests_run > 0", "tests_new == 0"]
      # Bugs injected into the changed code must be caught by the tests.
      - { type: mutation, tool: cargo_mutants, in_diff: true, min_score: 0.6 }
"#;

const TESTS_PROMPT: &str = "\
You write tests only. Write Rust integration tests under tests/ for the work described
below. Cover normal cases, boundaries and malformed input.

If the tests call something that does not exist yet, add it under src/ with its real
signature and a body of `todo!()`, and export it, so the tests compile. Nothing more: do
not implement anything. Every test you add must fail. Run `cargo test` to confirm they
compile and fail.
";

const IMPLEMENT_PROMPT: &str = "\
You implement. Make the tests under tests/ pass by changing src/ only. The tests are
locked: do not edit them. Run `cargo test` to check your work.
";

const POLICY: &str = "\
# Repository policy, read from the commit a run starts at.
#
# Paths no agent may change, in any workflow. conductor always protects its own config
# (.conductor/*.yaml, .conductor/workflows/**, .conductor/prompts/**) and .github/**.
protected: []

# Files build tools rewrite, which any stage may change. They are listed on the receipt as
# not reviewed. Leave unset for the usual lockfiles (Cargo.lock, package-lock.json, …);
# `generated: []` allows none.
# generated: [Cargo.lock]
";

const TASK: &str = "\
# What to build

Describe the change here: what it should do, with examples of inputs and outputs, and
anything it must not do. Both agents get this text. Then run:

    conductor run .conductor/workflows/build.yaml --spec .conductor/task.md
";

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub fn init(repo: &Path) -> Result<ExitCode> {
    if !repo.join("Cargo.toml").is_file() {
        bail!(
            "this repository has no Cargo.toml. conductor 0.1 checks work with `cargo test`; \
             other languages need the JUnit XML parser, which arrives in 0.2"
        );
    }
    let files = [
        (".conductor/workflows/build.yaml", WORKFLOW),
        (".conductor/prompts/tests.md", TESTS_PROMPT),
        (".conductor/prompts/implement.md", IMPLEMENT_PROMPT),
        (".conductor/policy.yaml", POLICY),
        (".conductor/task.md", TASK),
    ];
    for (rel, text) in files {
        let path = repo.join(rel);
        if path.exists() {
            println!("  kept     {rel} (already there)");
            continue;
        }
        std::fs::create_dir_all(path.parent().unwrap_or(repo))?;
        std::fs::write(&path, text)?;
        println!("  wrote    {rel}");
    }
    let ignore = repo.join(".gitignore");
    let current = std::fs::read_to_string(&ignore).unwrap_or_default();
    if current
        .lines()
        .any(|l| l.trim().trim_matches('/') == ".conductor/runs")
    {
        println!("  kept     .gitignore (already ignores run records)");
    } else {
        let sep = if current.is_empty() || current.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::write(&ignore, format!("{current}{sep}/.conductor/runs/\n"))?;
        println!("  updated  .gitignore: run records stay out of git");
    }
    println!(
        "\nNext:\n  1. describe the work in .conductor/task.md\n  2. commit .conductor/ and .gitignore (runs read the workflow from a commit)\n  3. conductor doctor\n  4. conductor run .conductor/workflows/build.yaml --spec .conductor/task.md"
    );
    Ok(ExitCode::SUCCESS)
}

struct Report {
    failed: bool,
}

impl Report {
    fn ok(&self, what: &str, detail: &str) {
        println!("  ✓  {what:<22} {detail}");
    }
    fn warn(&self, what: &str, detail: &str) {
        println!("  !  {what:<22} {detail}");
    }
    fn fail(&mut self, what: &str, detail: &str) {
        self.failed = true;
        println!("  ✗  {what:<22} {detail}");
    }
}

fn version(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_owned()
    })
}

pub fn doctor(repo: &Path, herdr: conductor_herdr::Herdr) -> Result<ExitCode> {
    let mut r = Report { failed: false };
    println!("conductor doctor · {}\n", repo.display());

    let Some(head) = git(repo, &["rev-parse", "--short", "HEAD"]) else {
        r.fail("git", "no commit yet: runs start from a commit");
        return Ok(ExitCode::FAILURE);
    };
    r.ok("git", &format!("HEAD at {head}"));

    // Workflows as a run will see them: at HEAD.
    let listed = git(
        repo,
        &["ls-tree", "--name-only", "HEAD", ".conductor/workflows/"],
    )
    .unwrap_or_default();
    let paths: Vec<&str> = listed
        .lines()
        .filter(|p| p.ends_with(".yaml") || p.ends_with(".yml"))
        .collect();
    let mut workflows = Vec::new();
    if paths.is_empty() {
        if repo.join(".conductor/workflows").is_dir() {
            r.fail(
                "workflows",
                "none committed; runs read workflows from a commit, so commit .conductor/",
            );
        } else {
            r.fail("workflows", "none yet; `conductor init` writes a starter");
        }
    }
    for p in &paths {
        let Some(text) = git(repo, &["show", &format!("HEAD:{p}")]) else {
            continue;
        };
        match Workflow::parse(&text) {
            Ok(wf) => {
                let v = wf.validate();
                if v.is_ok() {
                    r.ok(
                        "workflow",
                        &format!("{p}: `{}`, {} stages", wf.id, wf.stages.len()),
                    );
                } else {
                    r.fail(
                        "workflow",
                        &format!("{p}: {} (`conductor validate {p}`)", v.errors[0].message),
                    );
                }
                workflows.push(wf);
            }
            Err(e) => r.fail("workflow", &format!("{p}: {e}")),
        }
    }
    if git(
        repo,
        &[
            "status",
            "--porcelain",
            "--",
            ".conductor/workflows",
            ".conductor/prompts",
            ".conductor/policy.yaml",
        ],
    )
    .is_some_and(|s| !s.is_empty())
    {
        r.warn(
            "uncommitted config",
            "changes under .conductor/ are not used until committed",
        );
    }

    // What the workflows need on this machine.
    let kinds: BTreeSet<&str> = workflows
        .iter()
        .flat_map(|w| w.stages.iter().map(|s| s.agent.kind.as_str()))
        .collect();
    for k in &kinds {
        match *k {
            "claude" => match version("claude", &["--version"]) {
                Some(v) => r.ok("agent claude", &v),
                None => r.fail(
                    "agent claude",
                    "`claude` is not on PATH; install Claude Code",
                ),
            },
            "script" => {}
            other => r.fail(
                &format!("agent {other}"),
                "only `claude` and `script` agents run in 0.1",
            ),
        }
    }
    let gates: Vec<&Gate> = workflows
        .iter()
        .flat_map(|w| w.stages.iter().flat_map(|s| s.all_gates()))
        .collect();
    let programs: BTreeSet<&str> = gates
        .iter()
        .filter_map(|g| match g {
            Gate::CommandAssert { command, .. } => command.first().map(String::as_str),
            _ => None,
        })
        .collect();
    for p in programs {
        match version(p, &["--version"]) {
            Some(v) => r.ok(&format!("check tool {p}"), &v),
            None => r.fail(
                &format!("check tool {p}"),
                "not on PATH; checks that run it would fail",
            ),
        }
    }
    if gates.iter().any(|g| matches!(g, Gate::Mutation { .. })) {
        match version("cargo", &["mutants", "--version"]) {
            Some(v) => r.ok("mutation testing", &v),
            None => r.fail(
                "mutation testing",
                "cargo-mutants is missing: `cargo install --locked cargo-mutants`",
            ),
        }
    }

    // Where runs will happen.
    let in_herdr = std::env::var("HERDR_ENV").as_deref() == Ok("1");
    match herdr.check_protocol() {
        Ok(p) if in_herdr => r.ok("herdr", &format!("protocol {p}; runs open a tab here")),
        Ok(p) => r.ok(
            "herdr",
            &format!("protocol {p}; use `--executor herdr` to run in it"),
        ),
        Err(conductor_herdr::HerdrError::Missing(_)) => {
            r.warn("herdr", "not installed; runs are headless (fine for CI)")
        }
        Err(e) if in_herdr => r.fail("herdr", &e.to_string()),
        Err(_) => r.warn("herdr", "no server reachable from here; runs are headless"),
    }
    match conductor_engine::otlp::Config::from_env() {
        Some(c) => r.ok("telemetry", &format!("runs are exported to {}", c.endpoint)),
        None => r.warn(
            "telemetry",
            "off; set CONDUCTOR_OTLP_ENDPOINT to export traces (`conductor trace` works without)",
        ),
    }
    let ignored = git(repo, &["check-ignore", "-q", ".conductor/runs/x"]).is_some();
    if ignored {
        r.ok("run records", "ignored by git");
    } else {
        r.warn("run records", "add /.conductor/runs/ to .gitignore");
    }

    println!();
    if r.failed {
        println!("Fix the ✗ lines before running.");
        Ok(ExitCode::FAILURE)
    } else {
        println!("Ready. Start a run with `conductor run <workflow> --spec <file>`.");
        Ok(ExitCode::SUCCESS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_starter_workflow_is_valid() {
        let wf = Workflow::parse(WORKFLOW).unwrap();
        let v = wf.validate();
        assert!(v.is_ok(), "{:?}", v.errors);
        assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    }

    #[test]
    fn the_starter_policy_parses() {
        conductor_model::workflow::Policy::parse(POLICY).unwrap();
    }
}
