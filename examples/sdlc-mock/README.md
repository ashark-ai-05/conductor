# The SDLC mock

A stand-in for the team's real loop, so the mechanism can be tried without Jira, Bamboo,
Bitbucket or Amp: a Spring Boot service with one real bug, a ticket in `tickets/` in the
shape a Jira ticket has, a deploy that starts the service locally, and a log it writes.

```
cp -r examples/sdlc-mock /tmp/accounts && cd /tmp/accounts
git init -q -b main && git add . && git commit -qm "accounts with BUG-101"
conductor run .conductor/workflows/bugfix.yaml --spec tickets/BUG-101.md
```

The run fixes the bug, deploys, checks the acceptance criteria, writes the evidence into
`tickets/BUG-101.md`, and waits. Open `conductor serve` to review and approve, or:

```
conductor approve <run-id> --by PO -m "AC1-3 shown against the deployed build"
```

What this mock cannot prove: that a reviewer at work accepts the evidence, that Amp or
Copilot can be driven as the `fix` stage, that Jira and Bamboo are reachable from a work
machine. Those are proven at work, on a real ticket.
