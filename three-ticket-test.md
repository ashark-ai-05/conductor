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
|      | BUG-101 |                               |                                 |                         |
|      | BUG-102 |                               |                                 |                         |
|      | BUG-103 |                               |                                 |                         |

Result: 6 of 6 yes means worked; anything else says what to build next, in the last
column. Decide from this table, not from a feeling.
