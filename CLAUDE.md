# conductor: read this first

Work here follows `APPROACH.md`: problem, person and evidence before any building; short;
ask instead of assume. If it is not in this file, ask before building it.

## The problem, as found by interview

At a regulated firm, no step of a change may proceed until it is proven correct,
reviewed by someone who did not do it, and recorded so it can be audited later. Today
the evidence (screenshots, log lines) is gathered after the fact into Jira, and the loop
(develop, deploy, test, review) is slow. The person: the developer, first; then QA and
the PO who reviews.

A second, separate problem on personal projects: a half-formed idea handed to an agent
comes back confidently written up at length, its assumptions unread, and the agent
builds things that were never wanted. Under test until 2026-10-09 (`drift-log.md`).

## The loop (from the interview; the mock copies it)

bug reported (BA/support writes acceptance criteria) → investigate → develop (Amp/Copilot)
→ deploy to test env (Bamboo pipeline) → test (dev, then QA; evidence into Jira; PO
reviews) → PR (Bitbucket; agents and people review) → merge → deploy to prod → verify.

## What is built

- Stages, checks the agent can't touch, receipts hash-chained and anchored in git,
  PR delivery, `check-pr`, metrics, terminal UI, `conductor serve` (two lanes: what the
  agent did, what conductor witnessed; replay; live).
- Any language via JUnit XML; `init` per stack; `setup:`.
- `agent: { kind: human, who: PO }` stages; `evidence: { to: "{{spec}}" }` into the
  ticket as it happens; `conductor approve`/`reject`, the page's buttons, `y`/`n` in the TUI.
- `examples/sdlc-mock`: Spring Boot service, BUG-101, deploy-and-test, the `bugfix`
  workflow. `tests/sdlc_mock.rs` runs it end to end.
- Claude agents' tool calls and refusals are recorded as they happen.

## Current scope card (fix → review)

Does: an agent fixes a mock bug; conductor deploys and checks the acceptance criteria;
evidence lands in the ticket as it happens; the run pauses; the PO approves or rejects.
Worked means: on three mock tickets, "I could approve from the ticket alone" and "a PO
at work would accept this" are both yes. Stop means: approving without reading.

## Next, in order

1. Answer a refused tool from the run page (Claude's `--permission-prompt-tool`).
2. Show the ticket's evidence on the review panel (today it says "in the ticket" but
   does not show it).
3. Two more mock tickets for the three-ticket test.
4. Then, only after the test: investigate stage, rejection looping back to fix, Amp or
   Copilot as an agent, Jira/Bamboo/Bitbucket over MCP.

## Later list (not now)

Cost caps, survivor loop, mutation for JS/Python, TypeScript mock, PR creation stage, prod
deploy, verify in prod, Confluence, WBS, analysis, screenshots as evidence, fleet view.

## Working rules

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` before every push. `CONDUCTOR_LANG_TESTS=1` makes the
  language and mock tests fail instead of skip when a toolchain is missing.
- Merge with merge commits (the receipt anchors depend on history).
- Plain words in code, docs and messages. State facts as facts, guesses as guesses.
