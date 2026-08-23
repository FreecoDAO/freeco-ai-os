# FreEco.ai Roadmap & verified status

_Last verified: **2026-08-15** against this repository's `v0.9.7` tag, the
running daemon on this machine, and the live OpenRouter model list._

Every claim below was checked against something that can disagree with it — a
running endpoint, a database row, a test, or a published API — rather than
against the commit that was supposed to implement it. Several items previously
described as done turned out to be schema-only or unreachable, and are now
recorded as such.

## Status legend

- ✅ **Shipped** — present in a tagged release.
- 🟨 **Unreleased** — merged on the current branch, not yet tagged.
- ⚠️ **Partial** — some layers exist, but the feature cannot be used.
- 📋 **Planned** — intended work that is not implemented.

## Shipped in v0.7.7

- ✅ Local-first Agent OS with desktop and CLI clients.
- ✅ Multi-agent workflows, approvals, budgets, audit records, and channel
  adapters.
- ✅ Native FreEco CEO, secretary, and shopping agents plus Dev Pod templates.
- ✅ Local-AI setup and model-provider configuration.
- ✅ Cloud-data disclosure confirmation and plain-language approval
  consequences.
- ✅ Shell-execution policy gate verification.
- ✅ Operational API authentication hardening, including the audit-log SSE
  stream.
- ✅ Token-authenticated, loopback-only WhatsApp Web gateway with a trusted
  local browser origin.
- ✅ Reproducible TruffleHog CI installation with a pinned checksum.
- ✅ Model tuning controls for the latest FreEco.ai workflows.
- ✅ Privacy-aware routing between local and cloud model paths.
- ✅ Encrypted recovery flow fixes for protected reasoning/persistence data.
- ✅ Security scanning coverage for bundled skills and release paths.
- ✅ Operator and contributor documentation in `wiki/`.

## Shipped in v0.9.2

- ✅ Chat history is no longer deleted during compaction. Two separate call
  sites were discarding messages; both are removed (`542f980`, and the earlier
  fix it followed).
- ✅ Historical tool results are truncated rather than re-sent in full on every
  turn (`e2ff16b`). This was the source of a ~31:1 token amplification.
- ✅ wasmtime 47.0.3, clearing RUSTSEC-2026-0222.
- ✅ Approval requests state what will run, why, and at what cost.
- ✅ Project, company, and team records with an `/api/org/*` API and a
  dashboard tab — verified by creating a project against the running daemon
  and reading it back.

## Shipped in v0.9.5

- ✅ Agent ids are derived from agent names instead of being random, so a
  reinstall reattaches an agent to its own history. Previously the registry
  came back with new ids and every conversation was orphaned in place: intact
  in the database, invisible in the product, indistinguishable from deletion.
- ✅ Recovery for history orphaned before that fix — `GET
  /api/sessions/orphaned`, `POST /api/sessions/orphaned/adopt`, and a button on
  the sessions page. Changes ownership only; no message is rewritten.
- ✅ OpenRouter model ids are sent in the form OpenRouter accepts. The
  catalog's `openrouter/` prefix is stripped only when a vendor/model pair
  remains, so `openrouter/free` and `openrouter/auto` — real models under the
  `openrouter` vendor — are passed through unchanged.
- ✅ Signup page, one-line instruction, and cost class for all 42 providers,
  served from the backend beside the provider registry. A test fails if a
  provider is added without them. Previously 13 of 42 were explained, in a
  hand-written table in the dashboard JavaScript.
- ✅ The agent model field is a list built from the live catalogue rather than
  a blank box that only worked if you already knew the exact id.
- ✅ Dashboard agent configuration presents and updates all persisted model
  settings: provider, model, output-token limit, temperature, API-key
  environment variable, and provider base URL.

## Shipped in v0.9.6

- ✅ Label-driven release preparation that validates private-distribution
  prerequisites, creates release metadata, tags the commit, and triggers the
  artifact build without manual version or tag changes.
- ✅ FreEco.ai sandbox and distribution naming with safe legacy configuration
  compatibility and per-workspace reusable-container isolation.

## Shipped in v0.9.7

- ✅ Release builds produce artifacts for internal releases. v0.9.6 was tagged
  and built nothing, because `prepare-release.yml` skipped the
  commercial-distribution checks while `release.yml` still enforced them —
  only half the gate had been made conditional. Both halves now read
  `COMMERCIAL_DISTRIBUTION`, so a tag without it builds and publishes
  normally.
- ✅ CI selects test platforms per event rather than always running three. The
  full matrix on every pull request exhausted the account's Actions allowance
  in a day (macOS bills at ten times Linux), after which every job failed
  instantly with no steps — indistinguishable from a broken workflow file.
  Pushes to `main` and release-labelled pull requests still run all three.

## Unreleased

### Done — sandbox tiers and a working container runtime

- ✅ **Podman runs containers on this machine.** Verified end to end:
  `podman machine init` from a locally fetched image, `machine start`,
  `podman run` pulling and executing a container, and `podman version`
  answering in seconds. This is the first container runtime that has ever
  worked here — Docker Desktop's daemon never completed an image pull.
- ✅ **The image had to be fetched with resume.** `podman machine init` pulls
  in one shot with no resume, so on a connection that drops mid-transfer it
  restarts from zero and never finishes; four attempts failed that way. The
  WSL rootfs on GitHub releases is 238 MB rather than the ~1 GB OCI image, and
  `curl -C -` in a retry loop completed it. Verified by zstd magic number, not
  by file size, so a truncated file could not be mistaken for a finished one.
- ✅ **Runtime is selectable** (`docker.runtime`). Zero hardcoded `docker`
  invocations remain; every call goes through the configured binary, and the
  runtime is carried per container because a container must be exec'd into and
  removed by the runtime that created it.
- ✅ **Sandbox tiers.** `exec` / `workspace` / `service`, with memory, timeout,
  PID limit, network and reapability following from the tier. The old defaults
  — 512 MB, 60 seconds, 100 PIDs — were correct for running a command and made
  building anything impossible: `pnpm install` was killed mid-flight and
  Postgres would not start. Explicit limits in an existing config still win.
- ✅ **A build-capable image.** `deploy/sandbox/Dockerfile` gains Node 22,
  pnpm, yarn, `gh` and `socat`. The default was `python:3.12-slim`, which has
  no Node at all, so an agent asked to build a web app could not install a
  single dependency.
- ✅ **Hibernate instead of destroy.** Idle cleanup now stops the container
  rather than removing it, so dependencies installed into it survive. Files
  under the workspace were always safe — that is a host bind mount — but
  everything in the image layers was thrown away.
- ✅ **Approval before a service starts.** A service keeps running after the
  call returns, publishes a port, and is exempt from cleanup, so approving
  "run a command" must not silently approve that.
- ✅ **The Docker probe is bounded at five seconds.** It had no timeout, so a
  wedged CLI — routine on Windows — hung every call that checked for it, with
  nothing reported.
- ✅ **Backups include agent workspaces.** They were excluded entirely, so a
  restore returned the database and the agents and none of their work.
  Regenerable directories are skipped and files above 64 MB are reported
  rather than silently dropped.
- ✅ **Local AI asks every time on unsuitable hardware.** `428` unless the
  caller confirms, with the reason and the estimated minutes per turn. A setup
  approved once used to keep starting forever.
- ✅ **Approvals explain themselves in the dashboard.** The kernel filled in
  `aim` and `consequences` and the API dropped them, so the UI could only show
  the raw command — the exact thing the explanation exists to replace.

### Verified, correcting earlier claims

- The WSL machine reports 2 GiB in `podman machine list`, but a container is
  granted 8 GB and sees 14.3 GB. That figure is stale metadata, not a ceiling;
  no resize is needed and `.wslconfig` was left as the user had it.
- The sandbox code is runtime-agnostic. A grep for "podman" finds one comment,
  because the code never names the binary — it invokes `config.runtime`.
  Concluding "no Podman support" from that grep inverts the logic.

### Not done — next

- ✅ **Step 1 done — `users` and `tenants` tables, `tenant_id` on 16 owned
  rows.** Both keyed by a generated id, never by display name: the previous
  auth work keyed users by `name` and looked them up with
  `find(|u| u.name == ...)`, which is the same shape as the bug that left 126
  messages unreachable this week. Roles are ordered so a check is a
  comparison, and an unknown role parses to the least privilege. Verified
  against a copy of the real database - 1298 rows preserved, none left
  without a tenant - and live on `GET /api/tenancy` and
  `POST /api/tenancy/users`.
- [ ] **Wire the data gateway into the LLM path.** Still called by nothing, so
  prompts are not scanned for keys. Must land before social login.
- [ ] **`users` and `tenants` tables.** No row can say who owns it.
- [ ] Build the T1 image with `podman build` and make it the default.
- [ ] Rebuild the desktop app from this repo. The installed one is from
      2026-08-12, still named `openfang-desktop`, and seizes port 50051 with
      code that predates every fix in this session.
- [ ] Prove a service tier end to end by hosting something real in it.


## Planned — containers, users, local and cloud

The sequence matters more than the items. Phase 1 is a gate: every later phase
assumes a row can say who owns it, and today no row can.

### Phase 1 — Foundation

- [ ] **`users` and `tenants` tables, `tenant_id` on every owned row.** This
  does not exist. `/api/auth/login` and `/api/users` are on `main`, but the
  database has no user or tenant concept, so nothing can be scoped to anyone.
  Multi-tenancy is not half-built here; it is designed-for and not started.
- [ ] **Merge `copilot/introduce-multiuser-support`** for local accounts. It
  stores users in `config.toml` (`name`, `role`, `password_hash`,
  `api_key_hash`, `enabled`), which is right for a handful of accounts on one
  machine and wrong as a tenancy model — a config entry cannot scope a database
  row. Take its session hardening; treat config users as a seed for the local
  case, not a second user system.
- [ ] **Wire the data gateway into the LLM call path.** `freeco-datagateway`
  is built and tested and called by nothing, so prompts are not scanned for
  keys today. This must land before social login, which routes real Google,
  GitHub and Microsoft tokens through the same path.

### Phase 2 — Containers that can actually build

Podman rather than Docker: daemonless, rootless by default, and OCI-compatible,
so the same images and the same commands work. Docker Desktop's daemon wedges
on Windows and takes every call that probes it down with it.

- [ ] **Runtime selection** (`docker.runtime = "podman" | "docker"`). Done and
  merged; Podman needs proving on a machine that can pull an image.
- [ ] **Raise the limits.** 512 MB and a 60-second timeout cannot build
  anything — `pnpm install` or `cargo build` is killed mid-flight, and Postgres
  will not start. T1 needs ~4 GB and minutes; T2 needs ~8 GB and no timeout.
- [ ] **A T1 image with Node in it.** `deploy/sandbox/Dockerfile` exists in this
  repository with rust, cargo, clippy, git, python3, jq and ripgrep, and is not
  the default — the default is `python:3.12-slim`, which has no Node, so a
  JavaScript project cannot be built at all. Add Node 22, `gh` and `socat`, and
  build it with `podman build`. The Dockerfile is OCI, so no rewrite is needed
  for the runtime change.
- [ ] **Hibernate rather than destroy.** Idle cleanup currently removes the
  container. Stopping it keeps the filesystem and resumes instantly, which is
  what makes a workspace worth returning to.
- [ ] **A `tier` field** replacing the scattered `scope`/`service`/
  `persistent_workspace` flags: T0 exec, T1 workspace, T2 service.

### Phase 3 — Tenant isolation

- [ ] **One SQLite database per tenant.** The strongest isolation model — no
  query can cross a tenant boundary because there is no shared table to cross.
  It is also why this project is unusually well placed for the sync
  requirement: the tenant boundary and the sync unit are the same file, in the
  same format a local install already uses.
- [ ] **T3 tenant envelope** — a per-user container holding that user's T0–T2,
  hibernated when they log out.
- [ ] **Per-tenant quotas** on CPU, memory, disk and spend.

### Phase 4 — Cloud

- [ ] Ingress and TLS, routing `user.freeco.ai` to that tenant's envelope.
- [ ] **Social login mapped to a tenant.** Identity, not a security boundary.
  Depends on Phase 1's gateway work.
- [ ] Per-tenant backup and restore.

### Phase 5 — Sync

- [ ] Local ↔ cloud reconciliation over the same schema.
- [ ] Conflict resolution.
- [ ] "Cloud only", "local only" or "synced" as a user choice.

- 🟨 Agents no longer lose their conversations on reinstall: ids derive from
  names, so rebuilding the registry reattaches history instead of orphaning it.
- 🟨 Model ids where the provider name is part of the id are no longer
  corrupted on save. `nvidia/nemotron-3-ultra-550b-a55b` was stored as
  `nemotron-3-ultra-550b-a55b`, which NVIDIA does not serve, so the request
  404'd and the fallback chain reported an error about a different model.
- 🟨 NVIDIA NIM free tier as the default: Nemotron 3 Ultra 550B, no daily token
  cap, recorded at zero cost because that is what the provider charges.
- 🟨 Every agent's system prompt carries working and secret-handling
  guidelines, subagents included.
- 🟨 Agent config asks which provider's key to use rather than the name of an
  environment variable, and takes the key itself in the same screen.
- 🟨 Tool filters are a checkbox list with select-all, not 65 names typed one
  at a time.
- 🟨 Models are chosen by intent (best free / free only / all); models without
  tool support are hidden because they fail on the first turn.
- 🟨 `scripts/refresh_model_catalog.py` checks the catalog against what
  providers actually serve. It found 15 delisted ids.
- 🟨 Agent workspaces repointed from `~/.openfang` to `~/.freeco-ai`. The data
  had migrated; the paths inside each `agent.toml` had not, and one agent had
  already dropped out of the registry because of it.

- 🟨 The Assistant can perform setup, not just describe it: provider_key_set,
  provider_list and agent_set_model. Keys are never echoed back.
- 🟨 OmniRoute runs in a container and FreEco routes through it: 115 models
  including the auto/best-* routes, plus 130 Arena leaderboard ratings.
- 🟨 The sandbox works with Podman as well as Docker. It was Docker-only, so on
  a Podman machine an agent was told to use a container it could not start.
- 🟨 Agents are instructed never to install software on the host, to work in
  their sandbox, and to ask first if something truly must touch the machine.
- 🟨 `stream` is always sent. Omitting it made gateways that default to
  streaming return SSE where the driver expected JSON.
## Partial — do not describe these as done

- ⚠️ **Unified inbox and contacts (CRM).** The tables exist
  (`inbox_messages`, `contacts`, `contact_handles`, migration v12) and are
  empty. There is no API — `/api/inbox` and `/api/contacts` both return 404 —
  and no dashboard tab. Nothing about this is reachable by a user. Building the
  schema first was reasonable; reporting it as a delivered feature was not.

## Known limits that shape the product

- **OpenRouter's free tier is capped at roughly 50 requests per day** without
  credits, returning `429 free-models-per-day`. An agent run spends several
  requests, so a new user on a free key can hit this within a session and will
  read it as the product being broken — the 429 is currently swallowed and
  shown as a generic failure. Adding $10 of credit raises the cap to 1000/day
  and is not consumed by free models. Any "works for free out of the box"
  claim has to account for this.
- **The desktop app and the CLI daemon share one database** at `~/.freeco-ai`.
  Running both at once puts two kernels on the same SQLite file and the same
  agents, which produces `Agent is unresponsive` heartbeat warnings.

## Planned

- 📋 **Social login, and the keys that come with it.** Sign in with Google,
  GitHub, Meta, or OpenAI, and offer to connect the AI subscriptions and keys
  that account already carries. Most people arriving at FreEco.ai already pay
  for at least one model provider without knowing they hold an API key; asking
  them to find and paste one is the step where they give up. Keys obtained
  this way follow the existing rule — stored in the hands/connector/env layer,
  never in plaintext, never in config committed to a repo.
- 📋 **Offer two or three free providers at setup, not one.** OpenRouter's free
  tier alone runs out within a session (see Known limits). Groq, Google AI
  Studio, and GitHub Copilot have independent free allowances, so proposing
  several and configuring them together gives a new user a working day rather
  than a working ten minutes.
- 📋 **Automatic switch to a backup provider when one is exhausted.** The
  fallback chain already exists and is honoured by the agent loop; what is
  missing is populating it by default from whichever providers the user
  connected, and treating a daily-quota 429 as a reason to move down the chain
  rather than to fail. Mixing a top-tier paid model with free ones works the
  same way: the paid model leads, the free ones catch the overflow.
- 📋 **AI gateway.** Not built — the crate is not in the tree and nothing in
  the runtime or kernel references it. Until it exists, no claim can be made
  about it preventing data leaving the machine.
- 📋 Surface provider rate-limit errors (429) as themselves instead of a
  generic failure, with the remedy stated.
- 📋 Unified inbox and contacts: API and UI over the existing schema.
- 📋 Per-model tuning fields exposed for every provider, not just a few.
- 📋 Import agents and settings from other platforms.
- 📋 Emergency-freeze control on every dashboard screen.
- 📋 Agent deletion-confirmation UI.
- 📋 Automated backup and recovery workflows.
- 📋 Company chart and live multi-agent view.
- 📋 Global assistant widget and voice experience.
- 📋 Multi-user and multi-company tenancy.
- 📋 Expanded language support, CRM integrations, Deskmate, and the FreEco.ai
  OS distribution.

## Release policy

Only work included in a Git tag is described as shipped. Work on the main
branch is unreleased until a new tagged release is published.

A feature is "shipped" when a user can reach it, not when the code that would
support it exists. Schema without an API, or an API without a route
registration, is recorded as Partial.
