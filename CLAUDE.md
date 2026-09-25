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
- `examples/sdlc-mock`: Spring Boot service, BUG-101/102/103, a deploy script that checks
  the run's ticket's ACs (`{{spec}}` in a check's command is the ticket), the `bugfix`
  workflow. `tests/sdlc_mock.rs` runs all three end to end. `three-ticket-test.md` is the
  sheet for the scope card's measure.
- Claude agents' tool calls and refusals are recorded as they happen.
- Evidence shows the checks before the fix, the change, and only conductor's words (an
  agent editing the ticket fails the attempt). A decision needs a note.
- A tool the agent asks for is answered by a person: `conductor ask` is Claude's
  `--permission-prompt-tool` (an MCP server over the run's `asks/` files); the run page,
  the TUI and `conductor allow`/`deny` answer it; the stage clock stops while it waits.

## Current scope card (fix → review)

Does: an agent fixes a mock bug; conductor deploys and checks the acceptance criteria;
evidence lands in the ticket as it happens; the run pauses; the PO approves or rejects.
Worked means: on three mock tickets, "I could approve from the ticket alone" and "a PO
at work would accept this" are both yes. Stop means: approving without reading.

## Second scope card (decide from one screen), 2026-09-26

Problem: Krunal, reviewing a run as the PO in herdr, had the intent in the ticket, the
change on a branch, the evidence in a file opened with git, the decision on another page
and the cost on a third; he did not know where to look ("i dont know where to look. where
is the ui?", 2026-09-25 19:23, run 0MUGNS5BX8U) and the review stalled.
Does: when a run pauses for a person, the TUI's live screen shows the ticket's title and
acceptance criteria, what changed, each check before the fix and after with its result
lines, what it cost (tries, tokens, dollars, time waited on people), and the decision box.
The engine writes it as `review.json`; the page can show the same file.
Does not: plan or break work down (no evidence yet); show the diff text or the log on that
screen (a key away); change the page (second surface, later).
Worked means: on the next three runs Krunal decides from that screen without opening git
or the browser, and none of the three comes back wrong.
Stop means: he opens git anyway.
Open: a PO at work has not seen it (Gate 3.3).

## Next, in order

1. The three-ticket test ran on 2026-09-25 (`three-ticket-test.md`): 6 of 6 by Krunal's
   reading, and its last column was built the same day. Not yet seen by a PO. Next: show a
   PO at work one ticket and write their words, verbatim, into the sheet. No feature
   before that.
2. Re-run the three tickets with the real Claude on the new evidence shape, and watch
   what happens when the agent edits the ticket and is stopped (BUG-103 did, once).
3. Then, from the PO's words: investigate stage, rejection looping back to fix, Amp or
   Copilot as an agent, Jira/Bamboo/Bitbucket over MCP, the review panel showing the
   ticket's evidence, or nothing.

## Later list (not now)

Cost caps, survivor loop, mutation for JS/Python, TypeScript mock, PR creation stage, prod
deploy, verify in prod, Confluence, WBS, analysis, screenshots as evidence, fleet view.

## Working rules

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` before every push. `CONDUCTOR_LANG_TESTS=1` makes the
  language and mock tests fail instead of skip when a toolchain is missing.
- Merge with merge commits (the receipt anchors depend on history).
- Plain words in code, docs and messages. State facts as facts, guesses as guesses.
