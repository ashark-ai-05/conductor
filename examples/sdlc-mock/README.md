# The SDLC mock

A stand-in for the team's real loop, so the mechanism can be tried without Jira, Bamboo,
Bitbucket or Amp: a Spring Boot service with three real bugs, a ticket for each in
`tickets/` in the shape a Jira ticket has, a deploy that starts the service locally and
checks the ticket's acceptance criteria against it, and a log it writes.

| ticket | the bug | for the three-ticket test |
|---|---|---|
| BUG-101 | an account with no postings gives HTTP 500 | one line |
| BUG-102 | a lower-case account id gives HTTP 404 | where to normalise is the agent's call |
| BUG-103 | a half-cent posting rounds the wrong way | a rounding rule, one token |

Each ticket's acceptance criteria are lines the deploy script can check, so the ticket is
the check's specification, the way the BA's criteria are at work.

```
cp -r examples/sdlc-mock /tmp/accounts && cd /tmp/accounts
git init -q -b main && git add . && git commit -qm "accounts with BUG-101"
conductor run .conductor/workflows/bugfix.yaml --spec tickets/BUG-101.md
```

The run fixes the bug, deploys, checks the ticket's acceptance criteria, writes the
evidence into `tickets/BUG-101.md`, and waits. The same for `BUG-102.md` and `BUG-103.md`. Open `conductor serve` to review and approve, or:

```
conductor approve <run-id> --by PO -m "AC1-3 shown against the deployed build"
```

What this mock cannot prove: that a reviewer at work accepts the evidence, that Amp or
Copilot can be driven as the `fix` stage, that Jira and Bamboo are reachable from a work
machine. Those are proven at work, on a real ticket.
