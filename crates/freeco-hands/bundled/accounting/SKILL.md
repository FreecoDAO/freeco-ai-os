# Akaunting

Auditable books on the user's own machine: income and expense, invoices, and the
reports grant funders ask for.

## What it is

Two containers: the Akaunting application and a MariaDB database. That is why
this hand declares `compose` rather than `image`. The database holds the user's
financial records and lives in a named volume that survives the hand being
paused.

- Application: <http://localhost:8080>

## The one manual step

After activation the user must open <http://localhost:8080> and finish the
first-run wizard — company plus admin user. Those credentials are what the
accounting tools authenticate with afterwards.

Report this rather than reporting the hand as finished. A running service with
no company set up cannot answer a single question about the books.

## Checking it

`hand_status accounting` reports `serving` from the runtime, not the registry.
First activation is slow: MariaDB initialises its data directory before
Akaunting will start, so the compose file gates the app on the database's own
healthcheck and the timeout is 600s.

```
podman compose -p freeco-accounting-<instance> logs
```

## These are financial records

- Never state a figure you have not read from the service. If it is unreachable,
  say so — a plausible wrong number in someone's books is worse than no number,
  because it will be believed.
- Never run `compose down -v`. That destroys the ledger. Pausing the hand runs
  `down` without `-v` deliberately.
- Deleting or editing records is the user's decision, not yours. Reconciliation
  that silently "fixes" a discrepancy destroys the audit trail that makes these
  books auditable in the first place.
