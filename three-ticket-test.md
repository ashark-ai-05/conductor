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

Result: 6 of 6 yes means worked; anything else says what to build next, in the last
column. Decide from this table, not from a feeling.
