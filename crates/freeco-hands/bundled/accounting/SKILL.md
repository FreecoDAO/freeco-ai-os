# Akaunting

Auditable books on the user's own machine: income and expense, invoices, and the
reports grant funders ask for.

## What it is

Two containers: the Akaunting application (`docker.io/akaunting/akaunting`,
upstream's image) and a MariaDB database, declared as native services rather
than a compose file. The database holds the user's
financial records and lives in a named volume that survives the hand being
paused.

- Application: <http://localhost:8080>

## The manual steps

After activation the user must open <http://localhost:8080> and finish the
first-run wizard — company plus admin user. The database step is prefilled
from the hand's environment. Then they enter that login under
**Services → Akaunting → Connect**: it is checked against Akaunting, stored in
the vault as `ACCOUNTING_API_EMAIL` / `ACCOUNTING_API_PASSWORD`, and from then on
the `accounting_api` tool can read the books. The company defaults to id 1 (the
wizard's); set `ACCOUNTING_COMPANY_ID` in the vault for another.

## Reading the books

`accounting_api` sends `GET` only, to `http://localhost:8080/api/...`, with the
stored login. Readable: `transactions`, `documents` (invoices with
`?search=type:invoice`, bills with `type:bill`), `accounts`, `contacts`,
`categories`, `taxes`, `transfers`, `currencies`, `items`, `reports`,
`companies`, `dashboards`, `ping`. It refuses every write, and the GET routes
Akaunting uses as actions (enable, disable, mark received/sent/cancelled,
duplicate, e-mail).

Report this rather than reporting the hand as finished. A running service with
no company set up cannot answer a single question about the books.

## Checking it

`hand_status accounting` reports `serving` from the runtime, not the registry.
First activation is slow: MariaDB initialises its data directory before
Akaunting will start, so the app waits for the database's own healthcheck and
the timeout is 600s.

```
podman logs freeco-accounting-app   # freeco-accounting-<tenant>-app for a tenant
```

## These are financial records

- Never state a figure you have not read from the service. If it is unreachable,
  say so — a plausible wrong number in someone's books is worse than no number,
  because it will be believed.
- Never remove the hand's volumes (`db-data`, `app-data`). That destroys the
  ledger. Pausing the hand stops the containers and keeps the volumes.
- Nothing here moves money, contacts a bank or acts on a real account.
  Akaunting records; it does not pay.
- Country-specific tax and bookkeeping rules are not configured; that is the
  owner's choice. Do not apply a country's rules from memory.
- Deleting or editing records is the user's decision, not yours. Reconciliation
  that silently "fixes" a discrepancy destroys the audit trail that makes these
  books auditable in the first place.
