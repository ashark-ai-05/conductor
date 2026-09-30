# First native-capability slice

Krunal's current problem: acting on a returned result requires another model message
or a predefined workflow, even when the next operation is a routine read or refresh.
The request to keep user actions open-ended and Conductor deterministic is the scope
for this implementation. Broader demand and time savings remain unmeasured.

This slice lets Krunal register a local report, find it in Actions, read its data,
refresh it, and inspect captured evidence inside the existing Herdr task surface.
Worked means that a second named report becomes usable without code or renderer
changes, and refresh/reopening/inspection start no agents. Stop and revisit the
contract if another report needs another top-level screen.

## Implemented boundary

- Adapters register under versioned names. Descriptors bind an operation name, label,
  adapter and explicit input data. A fingerprint covers the complete descriptor.
- The registry is reconstructed from native adapters and explicit project report
  registrations. Unknown adapters stay unavailable. Registration does not install
  executables, expose PATH as a tool catalog, or create executable UI.
- Dispatch validates the saved fingerprint against the current descriptor. Rebinding
  a name cannot retarget a saved task. Payload changes at the same selected source
  are expected during a refresh and become another retained attempt.
- A separate versioned task record contains ordered start/capture/failure events.
  Projection checks transition order and output hashes. It performs no effects.
  Existing question records and hash-chained workflow evidence are unchanged.
- The OS holds one writer lock per native task. A crash releases the lock; replay
  shows an interrupted read. An explicit retry records the interruption before
  starting another attempt. A model is never involved in these transitions.
- Shapes select curated components: JSON objects to facts, record arrays to tables,
  Markdown files to document widgets, and other text to a literal viewer. Saved `.md`
  and `.markdown` reports are recognised on reopen, including older plain-text captures.
  `v` shows Markdown source. Imported claims never become native acceptance checks.
- The UI retains an expanded evidence snapshot while newer results arrive. Actions
  explicitly reloads its inventory; queued bindings are revalidated on dispatch.

This is not yet an adaptive planner or a generated-script runner. It establishes the
adapter, binding, capture and presentation boundary using two real read adapters.
No claims of live Jira/CI integration, sandboxed code execution, signed audit records
or full event-level control over delegated agents are made.

## Limits and next step

File sources are explicitly selected UTF-8 regular files inside the project, up to
64 KiB. Canonical paths are checked before accepting a read; this local adapter is
not a sandbox against a hostile process racing filesystem changes. Git status uses
fixed argv, disabled filesystem-monitor hooks, a deadline and the existing runner.
Records retain at most 32 attempts. The registry uses a small filesystem inventory;
large catalogs will require indexing and paging.

Next add one live provider adapter through the same interface, including its real
availability and authentication failures. Only then add plans that compose operations,
followed by bounded generated diagnostics. Those steps need their own working tests;
the earlier Jira walkthrough remains illustrative.
