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

### Next — agents that refuse tools they hold

Verified on 2026-08-31: `session_rename` and `session_fork` are in the
assistant's request payload (`tool_count=70`, both named in `tool_names`), the
agent lists them when asked what it has, and it then answers "I can't call
`session_rename` because it isn't available in the callable tool interface".
The plumbing is fine end to end; the agent is declining and composing a reason.

This is the third instance of one behaviour, not three bugs:

- `shell_exec` — quoted an error string this codebase never emits, after a
  loop-guard block it had misread as a permissions failure.
- its own model — said it could not know, then named a different agent's model.
- `session_rename` — as above.

The shape each time: something blocks or looks unfamiliar once, the agent
converts that into a **standing claim about its own capabilities**, and repeats
it from its own conversation history without retrying. Its history is the
evidence it trusts most, and nothing contradicts it.

- ⬜ **Tell agents what they hold, plainly.** The prompt lists tool names but
  never says the list is authoritative and current. A line stating that the
  tools in this request are the tools that exist, and that a past failure is not
  evidence about the present, costs nothing.
- ⬜ **Make refusals checkable.** An agent claiming it lacks a tool it was given
  should be visible: log the claim against the payload, so "the agent is wrong"
  and "the product is broken" stop looking identical from the outside. Hours
  went into the second before finding it was the first.
- ⬜ **Stop failures becoming beliefs.** A blocked call sits in history as a
  bare failure. Recording *why* alongside it -- transient, guard, genuinely
  absent -- gives the agent something to reason from other than "that did not
  work, so I cannot".

### Next — agents that accumulate understanding

Context lives in three tiers, and only two of them exist here. Recent turns are
replayed verbatim, older ones are compacted into a summary, and durable facts
are supposed to live in memory — but nothing ever writes that third tier, so
everything past the replay window degrades into a flattened summary and no
conclusion is ever promoted out of a conversation. That is why an agent reads as
a different person each time: it has a transcript, not knowledge.

Done: `memory_store` was described as a coordination channel ("shared memory
accessible by all agents"), which is not a description any model reads as "this
is how you remember things". It now says what it is for, and the prompt tells
agents to record durable facts when they learn them.

- ⬜ **Extract on compaction.** The moment history is about to be summarised is
  exactly when durable facts should be lifted into memory. Compacting without
  extracting is pure loss, and it happens silently.
- ⬜ **Summarise with a model, not string concatenation.** `compacted_summary`
  is currently roles and text joined together, which degrades badly and gets
  worse the longer a conversation runs — the case where the summary matters
  most.
- ⬜ **One canonical session per agent, not per task.** Storage is keyed by
  `agent_id`, so every unrelated conversation with one agent shares a single
  message list and competes for the same 50-message window. Separate tasks
  should not evict each other.

**Note for anything touching this:** context is keyed by agent, not by model, so
it already survives a model change. When history appeared to vanish on a model
switch the storage was intact — the UI had cleared its view and had no endpoint
to reload from. Check the surface before suspecting the store.

### Next — settings that stay set

- ✅ **`agent.toml` does not override the database** — this was recorded here as
  a bug and the code says otherwise: the boot scan skips any directory whose
  agent name is already in the registry, so a restored agent is never rebuilt
  from disk. The disk file is a seed for agents the database does not know, and
  `persist_manifest_to_disk` keeps it in step. Left as a corrected entry rather
  than deleted, because the wrong version was acted on.
- ⬜ **A model change was still observed reverting**, with the dashboard showing
  `gpt-5.6-terra-pro` while the registry and `agent.toml` both held
  `gpt-5.4-2026-03-05`. Since disk does not override the database, the
  divergence is elsewhere — a write that never reached the registry, or a stale
  view. Needs reproducing with the before/after state captured, not guessed at.
- ⬜ **Two assistant directories exist:** `agents/assistant` (nvidia, last
  touched 2026-08-23) and `agents/Freeco.AI assistant` (openai, live). The first
  is a pre-rename leftover and is dormant, but boot auto-spawns agents from
  `agents/*/agent.toml`, so a stale directory is a duplicate agent waiting to
  happen. Left in place deliberately: nothing is deleted until the owner says.
- ⬜ **A reply should name the model that actually served it.** When the primary
  fails the chain silently substitutes another, while the UI keeps showing the
  configured one -- so "the model shown is not the model answering" is true and
  currently invisible. This was suspected from a dead `omniroute/auto` fallback
  and could not be confirmed from logs, because the log had been truncated;
  reporting the serving model would have made it self-evident.

### Next — container runtime watchdog

- ⬜ **Notice when the runtime dies mid-run.** `runtime()` caches the runtime
  name after a successful probe, so if the VM stops while the daemon is running
  every container operation fails with podman's own error until something calls
  `runtime_status()`. Recovery on the *cold* path is done (a failed probe now
  triggers `podman machine start`), and the gap is the warm one.
- ⬜ **Design, so this is added deliberately rather than bolted on:** a task on
  a 60s tick calling `runtime_status()`, holding the result in shared state,
  logging only on *transitions* (a warning every minute for a runtime the user
  has deliberately stopped is noise that trains people to ignore the log).
  Auto-start attempts must be rate-limited — the agent auto-recovery loop
  performed 94 restarts in one run precisely because nothing limited it.
- ⬜ **Re-activate service hands whose containers vanished.** A stopped VM
  leaves every hand instance recorded as Active with nothing running, which is
  exactly the "reports Active while serving nothing" failure the container
  wiring was written to end. The watchdog should reconcile: for each active
  service instance, if its container is gone, start it or mark the instance
  degraded — never leave the record claiming something that is not true.

**Why this matters:** the VM was down for four days here and the only symptom
was LLM errors, because OmniRoute stopped answering. A container problem
surfaced as a model problem, which is the most expensive kind of wrong signal.

### Next — session tools for agents

The assistant confirmed from inside a live session that this is a tool-exposure
gap, not a design question: it searched its callable tool surface and found no
session-label or append-canonical operation. The backing functions exist in
`freeco-memory` (`set_session_label`, `create_session_with_label`,
`append_canonical`) and stop at the crate boundary — nothing in
`builtin_tool_definitions()` or `KernelHandle` reaches them. This is the same
dead-wiring shape as `freeco_hands::container`: complete implementation, no
caller.

Blocked work, reported by the assistant as not executed:

- ⬜ **`session_rename`** — rename the current thread (the pending case: rename a
  thread to "3+5"). Backing: `set_session_label`. Needs a tool definition, a
  `KernelHandle` method and a kernel implementation.
- ⬜ **`session_fork`** — copy a conversation up to a chosen message into a new
  named session, leaving the original intact. Backing:
  `create_session_with_label` + `append_canonical`.
- ⬜ **`session_split`** — the pending case: split one mixed thread into separate
  sessions named "1", "2" and "4". Same operation as fork with the tail moved
  rather than copied, so it needs message-level removal from the source and must
  therefore wait on the soft-delete work below.
- ⬜ **Message-level soft delete, approval-gated.** No message delete exists in
  `freeco-memory` at all. This one must not be built casually: chat history has
  already been lost here once, and the standing rule is that nothing is removed
  until the user removes it. It needs a recoverable soft delete plus the
  approval gate, so a deletion is reversible and never happens without a human
  answering for it. Until that exists, `session_split` copies rather than moves
  and the source thread is left untouched.

### Done — the chat talks to the right agent, with the right model

- ✅ **Approvals explain themselves.** `request_approval` had no parameter for
  the agent's reasoning, while the per-tool fallback text was documented as
  covering requests that arrive without it — so the fallback was the only
  reachable path and every request read "Agent \<uuid\> requests to execute
  shell_exec". Agents now supply a `purpose`, required by the `shell_exec`
  schema, and the prompt makes the sandbox the default and the host a last
  resort that must justify itself.
- ✅ **The assistant is resolved by role, not by name.** The chat matched the
  first agent whose name contained "freeco", which is the product name that
  every bundled agent carries, so `freeco-shopping` answered messages meant for
  the assistant. `/api/assistant` resolves by role, adopts the agent as CEO and
  persists it, so the guess happens once.
- ✅ **Agents stop being resurrected.** The crash-recover guard only protected
  agents that had never been used, so any agent that had been chatted with
  entered the loop: 94 recoveries in one daemon run, including agents the user
  had switched off.
- ✅ **A model picker that only offers models that work** — provider
  configured and tools supported, labelled with provider and price. The list was
  already fetched and never rendered, and the switch built its URL from an
  object.
- ✅ **A new task gets its own named session**, with relatedness judged by the
  model and failing open to continuing the current one.
- ✅ **Permission requests appear in both chat surfaces**, from one shared
  implementation.

### Done — service hands actually start, and the fallback chain works

- ✅ **Service hands start their container.** `freeco_hands::container` had
  **zero callers** outside its own module: the schema, the health check, the
  `HAND.toml`, the tests and the SKILL doc all existed, and nothing ever called
  `start`. Activating OmniRoute returned `status: Active` and started nothing.
  Activation now starts the container and waits for its health check, and rolls
  the instance back if it does not come up — an instance that maps to no
  container would otherwise block the next activation as "already active" while
  still serving nothing. Wired on both paths: the HTTP route and the
  `hand_activate` tool agents call.
- ✅ **Deactivation stops the container.** The hand id is read *before* the
  instance record is removed; afterwards nothing remains to say which container
  the instance owned, and a leftover container holds the port so the next
  activation fails complaining about the port instead of the leftover.
- ✅ **`hand_status` asks the runtime.** It reports `is_service` and `serving`,
  checked against the container rather than inferred from the registry. The
  registry records what was asked for; only the container knows what runs, and
  they disagree exactly when it matters.
- ✅ **Streaming requests fall back like non-streaming ones.** The two paths had
  drifted: non-streaming fell back on ModelNotFound, Timeout, Overloaded and
  ContextOverflow, streaming only on ModelNotFound. The dashboard streams, so
  the narrower path was the one users hit. Both now share
  `should_try_fallback`. Auth and Billing still never fall back — spending on
  another provider to work around the user's own key is worse than failing.
- ✅ **The free-model picker no longer returns models it must not.**
  `best_free_tool_model()` filtered on price and on the `tools` parameter, and
  both of the families it needed to exclude pass those checks: `openrouter/*`
  routers advertise `tools` then answer "No endpoints found that support tool
  use" — the exact failure the function exists to avoid — and `stealth/*`
  cloaked models log prompts to an undisclosed provider. The cloaked model had
  the widest free context on OpenRouter, so ranking picked it first.

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


- 🟨 Container runtime detection and the five-second probe timeout are one
  mechanism now. Two sessions built these separately; detection probes each
  candidate through the guarded check, so a wedged Docker costs five seconds
  once instead of hanging, and Podman is found when Docker does not answer.
- 🟨 `docs/services.md` records the three tiers, why containers rather than
  WASM for persistent services, and what exists today versus what does not.
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
