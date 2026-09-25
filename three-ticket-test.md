# The three-ticket test

The scope card's measure (CLAUDE.md): on three mock tickets, "I could approve from the
ticket alone" and "a PO at work would accept this" are both yes. Stop means: approving
without reading.

One run per ticket, with the real Claude as the `fix` agent. Review each as the PO, from
the ticket file on the run's branch and nothing else, then fill in the row. Be honest; a
"no" with its reason is the useful kind of row.

```
cp -r examples/sdlc-mock /tmp/accounts && cd /tmp/accounts
git init -q -b main && git add . && git commit -qm "accounts with three bugs"
conductor run .conductor/workflows/bugfix.yaml --spec tickets/BUG-101.md   # then 102, 103
```

| date | ticket | approve from the ticket alone? | a PO at work would accept this? | if no, what was missing |
|------|--------|--------------------------------|---------------------------------|-------------------------|
| 2026-09-25 | BUG-101 | yes | yes | "looks good". Noise: Maven warnings and the full startup log; each check's line shown twice |
| 2026-09-25 | BUG-102 | yes | yes | same as 101; no "before" (the ACs failing before the fix); the diff is not in the ticket |
| 2026-09-25 | BUG-103 | yes | yes | same; the agent wrote the ticket's root cause and scope allowed it; approvals with an empty note were accepted |

Runs 0MUGHYFUEJQ, 0MUGI17JOTA, 0MUGI3JDKEY in /tmp/accounts, the real Claude as the fix
agent, one try each, about $1 each. The approvals were clicked with empty notes to see what
happens, and the review was done from the tickets afterwards. Result: 6 of 6, with the last
column as the list of what to change, in order: cut the noise, say each check once, show the
before, show the change, keep the agent out of the ticket, require a note on approve.

## Second run, 2026-09-25, after the six changes

| date | ticket | approve from the ticket alone? | a PO at work would accept this? | if no, what was missing |
|------|--------|--------------------------------|---------------------------------|-------------------------|
| 2026-09-25 | BUG-101 | (rejected on purpose, to see the reject path) | | the rejection and its reason land in the ticket; nothing loops back to the agent |
| 2026-09-25 | BUG-102 | yes | yes | "satisfactory evidence provided" |
| 2026-09-25 | BUG-103 | yes | yes | note was "approved": a required note cannot make a good note |

Runs 0MUGNS5BX8U, 0MUGNUOG5TR, 0MUGNXY7TNJ. On 102 and 103 the agent wrote the ticket's
root cause on try 1, was stopped, and passed on try 2 without touching it: the guard works
against a real agent, at the cost of one retry (about $1). Reviewed from the run page.

What the reviewer said while reviewing, verbatim: "i dont know where to look. where is the
ui?" The page and the TUI both say the evidence is in the ticket and neither shows it; the
reviewer had to open the ticket file with git. That is the first real incident for the
review panel item, from the person on the scope card, at the moment on the card.

Also seen: the agent inherits the reviewer's global Claude plugins (a memory plugin wrote
notes under the repository root, outside the worktree, through Bash, which the mock
allows). On the later list: start the headless agent without the user's own settings.

Result: 6 of 6 yes means worked; anything else says what to build next, in the last
column. Decide from this table, not from a feeling.
