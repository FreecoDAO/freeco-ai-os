# Twenty CRM

A CRM running on the user's own machine — people, companies, notes, pipeline.
Activating the hand brings up the stack; nothing else is needed to get it
running.

## What it is

Two containers, not one: the Twenty application and a Postgres database. That is
why this hand declares `compose` rather than `image`. The database holds the
user's contacts, so it lives in a named volume and survives the hand being
paused.

- Application: <http://localhost:3200>
- Port 3200 deliberately, **not** 3000 — the voice hand publishes 3000, and two
  hands fighting over one port means whichever is activated second fails to
  bind.

## The one manual step

Twenty has no API until a workspace exists. After activation the user must:

1. Open <http://localhost:3200>
2. Create their workspace
3. Settings → API & Webhooks → create an API key

Say this plainly. Until that key exists the service is up and every tool that
reads from it returns nothing, which reads as a broken install and is not.

## Checking it

`hand_status crm` reports `serving`, checked against the runtime rather than the
registry. A stack can have its application up while the database is still
initialising, so first activation legitimately takes minutes — the health
timeout is 600s for that reason, and the compose file waits for Postgres to pass
`pg_isready` before starting the app.

If it does not come up:

```
podman compose -p freeco-crm-<instance> logs
```

## Do not

- Do not start it with shell commands. A stack started outside the hand has no
  instance record, so `hand_status` and everything downstream is blind to it.
- Do not run `compose down -v`. The `-v` destroys the named volumes, which is
  the user's contact database. Pausing the hand runs `down` without it on
  purpose.
