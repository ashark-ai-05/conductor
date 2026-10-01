# Conductor

A terminal workspace for questions, local reports, and coding workflows. Use it
standalone or inside [Herdr](https://github.com/herdrdev/herdr), with results,
sources, activity, and review decisions in one place.

**Status: v0.1.** Claude CLI is the supported agent provider. Workflow stages also
support scripts and human review. Other agent providers and live Jira/CI
connections are not implemented.

## What it does

- **Questions:** web answers and follow-ups with saved history and sources. Results
  render as Markdown, facts, or tables. Structured clarification requests offer
  selectable choices, a custom answer, and optional context.
- **Local reports:** inspect Git status or register JSON, Markdown, and text files.
  Capture, refresh, and display them without a model call.
- **Coding workflows:** run YAML stages in a separate Git worktree, with retries,
  independent checks, saved diffs, test reports, and human decisions. Each run
  produces a receipt recording check results and gaps.

Conductor selects and renders built-in widgets from validated data. Agents supply
content; they do not generate executable UI.

## Install and try

Requires Git and a current stable Rust toolchain. Agent tasks also require an
authenticated `claude` CLI on `PATH`; the demo and native operations do not.

```sh
git clone https://github.com/ashark-ai-05/conductor.git
cd conductor
cargo install --locked --path .
conductor ui --demo
```

The demo uses illustrative data and makes no model calls. Press `Tab` to browse
sample questions, clarification inputs, and a bug-fix review.

To ask a real question, run this from your project directory:

```sh
conductor ui --ask
```

Type a question and press Enter. Questions use web tools with no project files
supplied; no workflow setup is needed. Follow-ups stay in the same task.

From a result, `n` starts a question, `w` opens configured workflows, `Tab` opens
tasks, and `Ctrl+K` opens native actions. `a` shows activity; `v` shows full text
or Markdown source. For questions, `s` opens sources and `f` follows up. For
workflows, `e` opens evidence.

## Inside Herdr

Requires Herdr 0.9.1 or later. From the Conductor checkout, in a Herdr session:

```sh
cargo build --release --locked
herdr plugin link "$PWD"
```

The plugin adds **Ask Conductor**, **Open project tasks**, and **Return to
Conductor task**. Workflow runs open in Herdr tabs; `g` visits the active agent.
Rebuild after updating the checkout: the plugin uses `target/release/conductor`.

## Local reports without an agent

In a Git repository, `Ctrl+K` → **Repository changes** captures Git status.
Register an existing project file to add another action, for example:

```sh
conductor capability add project.readme README.md --label "Project README"
conductor capability run project.readme
```

The command prints a task ID and the command to open it. JSON objects render as
labelled values, record arrays as tables, and Markdown as documents. Other text
uses a literal viewer. `r` captures a fresh result and retains previous attempts.
Files must be UTF-8, inside the project, and at most 64 KiB.

## Run a coding workflow

From the target Git repository:

```sh
conductor init
$EDITOR .conductor/task.md
git add .conductor .gitignore
git commit -m "Configure Conductor"
conductor doctor
conductor run .conductor/workflows/build.yaml --spec .conductor/task.md
```

Review the generated workflow and install any prerequisites reported by `init`
and `doctor`. Workflows and prompts are read from the starting commit, so commit
changes before running.

The starter has one agent write failing tests and another implement the change,
with checks rejecting edits to frozen tests. `init` detects Rust, JavaScript
(Jest/Vitest), Python, Go, Maven, and Gradle. Checks read Cargo output or JUnit XML;
the Rust starter also requires `cargo-mutants` for mutation testing.

CLI runs default to Herdr panes inside Herdr and headless execution elsewhere
or in CI. Use `--executor herdr` or `--executor headless` to select explicitly.

| Command | Purpose |
|---|---|
| `conductor receipt <run>` | Read the check results and recorded limitations |
| `conductor verify <run>` | Verify record integrity without model calls |
| `conductor approve <run> -m "What I checked"` | Accept a pending human review; `reject` declines it |
| `conductor deliver <run>` | Push a passed run and open a draft PR when GitHub credentials are available |
| `conductor check-pr` | Verify that the reviewed branch ends at its delivered receipt |
| `conductor serve --open` | Browse workflow timelines, receipts, and statistics |

See `conductor --help` for all commands.

## Evidence and limits

Conductor runs workflow checks using definitions from the starting commit.
Receipts record what passed, failed, or was not checked; their evidence is
hash-chained and anchored in Git. A passing receipt covers the configured checks.

Question citations are supplied by the agent. Matching captured web fetches are
shown when available; cited claims are not independently verified. Question
history and native captures are stored locally under `.conductor/`, separately
from workflow receipts.

## Guides and development

- [Workspace controls, result views, and clarification inputs](docs/task-workspace.md)
- [Native capabilities and capture limits](docs/native-capabilities.md)
- [Example workflow](examples/build.yaml) and [bug-fix example](examples/sdlc-mock/README.md)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Set `CONDUCTOR_HERDR_BIN` to include tests against a real Herdr installation.
