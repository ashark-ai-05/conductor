# Task workspace: first implementation

Approved by Krunal in the UI discussion on 2026-09-26, after reviewing the question,
answer, Sources and Activity layouts. This replaces the launch/runs/live/receipt
navigation in the default terminal UI.

Problem: Krunal could not find the request, result and evidence together while
reviewing Conductor inside Herdr. The weather question also went through a build
workflow, failed a deployment check and returned no useful answer in the main view.

The task stays selected while it works, finishes, or waits for a person. The main area
shows its output. Sources/Evidence and Activity open beside it when there is room,
or fill the task area with an Escape path back. Build review outputs remain accessible
after the human stage ends. Startup failures remain in the task list.

## Scope

- Direct questions use the existing authenticated Claude CLI, with WebSearch and
  WebFetch available. They run in an empty temporary directory with safe mode enabled,
  no project files supplied, no workflow, worktree, Herdr tab or human acceptance gate.
- Each follow-up and refresh adds a turn to the same task. Previous answers, citations
  and activity stay associated. A request for clarification is `Needs you`; an answer
  is `Answered`, never a passing check. Timeouts and process errors are `Stopped`.
- Questions persist under `.conductor/questions/`. This is conversation history, not
  the hash-chained, Git-anchored receipt used for build checks.
- Citations and their support statements are supplied by the answering agent. An exact
  URL match to a successful recorded WebFetch attaches its captured tool response and
  retrieval timestamp. Other citations explicitly say no matching fetch was captured.
  This does not independently validate the cited claim or the page's observation time.
- Activity records tool calls, returned tool output, failures and lifecycle events.
  Tool output is capped at 24,000 characters per event with visible truncation. No
  generated reasoning or made-up progress is displayed. Missing usage stays unavailable.
- Existing build workflows, independent checks, tool permissions, and human review
  decisions retain their execution behavior. They use the same workspace with Evidence.

## Use

Run `cargo run -- ui` in the repository, or `conductor ui` after reinstalling.
`cargo run -- ui --demo` uses explicitly illustrative data and makes no agent calls.

- Type a question and Enter. `n` starts another question; `w` starts a configured workflow.
- `Tab` switches task-list focus; arrows select, Enter opens a task. On a workflow
  composer, Tab switches between the workflow selector and input; arrows choose a workflow.
- `s` opens Sources; `1`–`9` opens a cited source directly. Enter expands an item, `o`
  opens a selected source in the browser. `a` opens Activity. `e` opens build Evidence.
- Escape returns to the result without resetting its scroll. Page Up/Down scroll long
  records. On narrow panes the inspector fills the available task area.
- `f` follows up, `r` refreshes/retries the displayed question, `[`/`]` reads earlier/later
  answers, `x` cancels the current question, `c` copies the answer (macOS).
- `y` accepts and `d` rejects an existing human review, with a required note. For a tool
  permission these keys allow or deny. Rejection retains the engine's existing stop
  behavior; an automatic revision loop is not implemented.
- Input supports cursor movement, Unicode, bracketed paste, and Alt+Enter for a newline.
  Quitting the UI leaves already-started work running.

## Validation and next observation

Automated checks use a fake Claude process, not a paid model call: direct answers,
follow-ups with history, captured source provenance, cancellation, duplicate submission,
agent failure, launch failure, keyboard behavior, and wide/narrow rendering. Existing
workspace tests cover build execution. The later Herdr integration section records
the real-agent acceptance check; fixture tests alone do not establish provider availability
or answer quality.

Next observation: Krunal asks three questions, including one he can check against its
source, without leaving this view to find the answer. Stop and reconsider if getting a
simple answer still requires workflow knowledge or reconstructing context from logs.

Later: dedicated QA execution, BA requirements editing, operations action plans,
file attachments, richer artifact viewers, and revision loops. These are not implied
to be implemented by having a shared task layout.

## Document rendering

Answers without structured data render in a bounded Markdown document. Headings,
strong/emphasized text, lists, block quotes, code, links and tables render as terminal
cells. Narrow tables reflow into labelled fields. `v` switches a prose answer between
rendered Markdown and its original source; structured answers retain the full-text
view. Existing source shortcuts still open the cited evidence. Unknown links retain
their destinations in a Links section. Raw captures and logs remain literal text.

Facts use a compact panel with labels above values in two columns when there is room,
and a single column in narrow panes. Long context has an explicit overflow hint and
remains available in full text. Layout and formatting require no model call; reopening
a saved answer uses the same renderer.

## Structured result views

Approved in the follow-up discussion after reviewing Shapeshift. The problem is the
same review moment: Krunal needs the useful values, comparisons or failed tests without
reconstructing them from prose, while retaining access to the supporting record.

- Questions may return one optional presentation: key facts or a table. Conductor
  validates its shape, size and citation references. Unknown or invalid presentations
  fall back to the complete Markdown answer with an explanation. Old records work.
- Each fact/row names its own one-based source references. Enter shows only those
  sources, including captured fetch output when available. An uncited row says so.
  `s` returns to all answer sources. A citation is still an agent claim about support.
- Existing build workflows show a test report when their gates contain parsed cargo
  or JUnit cases. Cases are grouped by stage/attempt and failed cases appear first.
  Enter shows the parsed failure message, command exits, retained stderr, output hash
  and report file hashes where recorded. Full command stdout is not retained in
  `gates.json`; the inspector says so. Expected/actual values are never invented.
- Cases come from the latest parsed report of the latest completed attempt per stage.
  Baseline cases are excluded. Gate verdicts include all reruns and stay visible,
  including flaky/unwitnessed results. Case counts are not the run verdict. Build/load
  failures with no cases say "No test cases recorded", never "all passed".
- Gate records are published after each completed attempt, including before a retry or a later human review.
  A running command does not yet stream individual cases into this report. Workflows
  without parsed test cases retain their existing output and check evidence.
- `v` toggles full text and the result view. Row selection survives opening evidence,
  switching to text and resizing. Narrow panes show one selected row with labelled
  fields; Enter opens its complete details. The task list, Activity and review actions
  stay in their established places. A later question can use a different presentation.

Scope excludes generated code/widgets, new providers, intent classification while
typing, automatic actions and BA/ops-specific editors. The agent supplies display data;
it cannot create test verdicts or action buttons through that data.

The native components take inspiration from [Shapeshift](https://github.com/anishfn/shapeshift)
and the focused interactions in [shipwithjev](https://www.shipwithjev.com/). No code or
runtime dependency from either project is used.

Validation uses weather → city comparison → forecast fixtures, failure → captured
test evidence, malformed result data, uncited rows, old records, Unicode and narrow
panes. The interactive demo exposes all three layouts. Next observation remains Krunal
using the result with a real question and checking a cited claim; fixtures do not prove
real-provider availability or answer quality.

## Herdr integration

Approved in the follow-up request to fit this UI to Herdr. The existing person and
problem remain the same; this pass removes repeated navigation and makes the route
from a result to an agent and back explicit.

- `ui --run <id>` and `ui --ask` use the full pane. The project task list is a `Tab`
  drawer, even on wide terminals. Escape returns without resetting the result.
  At 80×24 the result, status and action remain visible. Sources and Activity occupy
  the task area when a side inspector would crowd it.
- Workflow tabs use the request's title. `g Open agent` visits an available stage
  pane. The plugin action **Return to Conductor task** returns to the originating
  view, including its selection and open inspector. Closed panes and other sessions
  produce an explanation instead of navigating to an unrelated pane.
- `.conductor/runs/<id>/herdr.json` stores full opaque IDs and their server socket.
  It is disposable navigation metadata, not evidence. Older records remain readable
  but do not acquire invented agent links.
- The Conductor view reports `working`, `blocked` for human input, and `idle` to
  Herdr. It releases its report on normal exit and never reports on the agent's
  behalf. These are lifecycle states; a failed run can be idle, and its failed
  checks remain visible in Conductor.
- **Ask Conductor** opens a focused overlay. **Open project tasks** opens or reuses
  the project's home tab in the current workspace. Actions resolve the invoking
  pane's project, not the plugin's working directory. Escape closes an unused Ask
  composer; `q` closes a result view. Started work continues after closing a view.
- Standalone UI launch no longer chooses the Herdr executor just because a server
  is reachable. The caller must actually be in a Herdr pane.

Build from this repository, then link from inside your Herdr session:

```sh
cargo build --release --locked
herdr plugin link /absolute/path/to/conductor
```

Use the actions from Herdr's plugin menu, or run `target/release/conductor herdr ask`
and `target/release/conductor herdr tasks` from the checkout in a Herdr pane. **Return to Conductor task** is also available as
`conductor herdr return` when the binary is on PATH. Existing keybindings are not
changed. Linking does not build; rebuild after updating Conductor.

Validated on Herdr 0.9.1 / protocol 22 in isolated sessions: plugin launch; a real LA
weather question and a sourced San Francisco follow-up in the same task; project
context and home reuse; script execution through checks and a human approval; blocked
→ idle attention; agent visit and return; narrow/wide rendering; client detach and
reattach. The weather turns retained their own sources, including explicit absence
of a matching captured fetch for one follow-up citation. These calls establish that
this provider path works here, not that every generated claim is independently true.

The real Herdr suite also exposed a stale completion marker when stages reused a
pane. Script attempts now use a unique marker so old terminal output cannot finish
another stage. The multi-stage Herdr test covers that path.

This pass adds no generated UI, new role-specific executor, new agent provider or
background monitoring. Next observation: Krunal uses this in his regular Herdr layout
and can finish a question and one review without reconstructing context from logs.


## Question and bug-fix layout

The approved two-workflow design uses a single task surface. The task list is a Tab
drawer in both focused and project views; Herdr owns the surrounding navigation.
Short answers keep their actions and follow-up prompt close to the result. Facts use
labelled values with source references; comparisons retain a table and a narrow-pane
field view. A failed or running retry may show the earlier answer to exactly the same
question, explicitly labelled as previous, with that answer's original sources. A
different follow-up never borrows an earlier answer.

Workflow attempts publish their gate snapshot after checks, before the next retry.
The task reads saved patches only for stage/attempt pairs in that snapshot. Changes
and parsed cases for the same recorded attempt appear beside one another on wide
panes. `p` switches changes/checks in narrow panes or selects patch scrolling in wide
panes; left/right selects a saved file, and Enter expands its saved patch preview.
Large previews disclose their byte or file limit and identify where the full saved
changes are retained.
These records describe a saved attempt, not unrecorded edits in a live worktree.
Other-stage cases remain in the full record (`v`), rather than being presented as
verification of the selected attempt. Check details already open stay fixed while a
new attempt arrives.

Activity uses readable event labels and actor names in the list. Expanding an event
retains timestamps and captured tool details. Human review, permissions, question
follow-ups/retry/cancel, and opening the agent retain their existing behavior. There
is no new workflow pause/resume or mid-attempt steering protocol; the UI exposes only
actions the runtime supports. A completed question is Answered, not a passing test.

`conductor ui --demo` includes an illustrative BUG-102 review in the task drawer.
The layout renderer example also accepts `bug` and `patch` for wide/narrow inspection.

## Clarification inputs (2026-09-27)

Krunal's FDE study-resource question returned a list of interpretations as prose, so
answering meant typing text that was already on screen. This slice lets him select an
interpretation or write his own answer and continue the same direct-question task.
Worked means the choice reaches the next answer with the original task history, and
reopening or submitting an older card cannot start duplicate or misdirected work.

- A question answer may carry `input_request` with `kind: single_choice`, a question,
  explanation and 2–8 uniquely identified options. Conductor validates the data and
  supplies the custom-answer path. Controls, layout and validation require no model call.
- Use arrows to choose, Space to select the focused option, and Enter to continue.
  `f` writes a custom answer; `c` adds context. Enter saves context; Escape retains the
  draft. `a` opens Activity, `v` shows the original message, and `[` / `]` browse turns.
- Submissions are bound to the task, one-based request turn and fingerprint of the
  complete validated request. The engine checks the latest state under an OS submission
  lock before reserving work. Concurrent or stale replies are rejected. The next turn
  stores the structured response and readable label/text; the prior turn is unchanged.
- A valid request puts the task in Needs input. The provider is prompted to reuse meanings
  already established in this task and ask only for information that changes the answer.
  Context is not implicitly borrowed from unrelated tasks.
- Completed cards become read-only history. The original message view includes the full
  recorded reply. Worker launch failures retain the submitted reply for explicit retry.
- Old prose-only requests and malformed/unknown schemas retain the ordinary text reply
  path. They are not reverse-engineered from bullets and are not migrated on reopen.
- Input requests collect information; they cannot grant tool permissions or invoke a
  capability. Existing workflow review and permission gates keep their own controls.

The scope is direct-question single choice plus free text. Multi-select, generated forms,
provider switching and clarification during a running build stage remain separate work.
The demo task “Best resources to study FDE” exercises rendering without invoking an agent.
