# conductor

Agentic software work with receipts. Conductor runs coding agents from YAML workflows,
in [herdr](https://github.com/ogulcancelik/herdr) panes when you want to watch, or headless
in CI and overnight. Every run ends in a receipt that says what was proven, by which check,
and what was not checked.

**Status: v0.1 in progress.** This build has the terminal UI (with sample data) and
workflow validation. Running workflows comes next.

## Try it

```bash
cargo run -- ui --demo            # dark theme; add --light for the light one
cargo run -- validate examples/build.yaml
```

In the UI: `1`–`4` switch screens, `↑↓` pick a run, `Enter` opens it, `r` opens the receipt
from anywhere, `n` starts a new run, `q` quits.

## How it fits together

- **Conductor runs as a herdr pane.** Each run gets its own herdr tab, with one pane per
  active stage. Conductor and the agents can open, close and drive panes; every action goes
  through conductor and is recorded (docs/SPEC.md §11.2).
- **Evidence never comes from guessing.** Turn boundaries and tool calls come from agent
  hooks, token usage from the agent's session log, and checks run in conductor's own
  process. herdr's view of agent state is used only for scheduling and is labelled
  `inferred`.
- **A receipt may only say "passed" about a check that could have failed.**

## Layout

| Path | What |
|---|---|
| `crates/conductor-model` | Workflows and their validation, the hash-chained evidence log, receipts, UI view models |
| `crates/conductor-tui` | The Ratatui interface |
| `src/main.rs` | The `conductor` command |
| `examples/build.yaml` | A build workflow: spec → independent tests → implementation |
| `docs/` | Product definition, spec, assessment, and the clickable design mockup |

## Checks

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
