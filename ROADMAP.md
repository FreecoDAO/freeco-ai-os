# FreEco.ai Roadmap & verified status

_Last verified: **2026-10-28** against this repository's `v0.9.8` tag, the
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

## Shipped in v0.9.8

The current state of every feature, with how to check it, is in
[`docs/feature-register.md`](docs/feature-register.md) (2026-09-30).

### Found 2026-09-30 — open

- ⬜ **Idle agents burn tokens.** Continuous-mode hands tick hourly with no work
  and resend their whole history: ~15 M of 27.8 M input tokens since
  2026-09-15. Next build: the tick loop honours pause, and projects, tasks,
  schedules and CRM tasks wake agents instead of a timer; the CEO governs who
  sleeps.
- ✅ **`pause_hand` is dead wiring** — fixed: `pause_hand` stops the hand's
  background agent and persists the paused state, and boot restores it paused.
- ⬜ **The SQLite log grows unbounded while the daemon runs.** An open read keeps
  the WAL from being checkpointed (106 MB beside a 24 MB database). Backups
  now checkpoint first; the holder is not found.
- ⬜ **Reactive agents with an `[autonomous]` block are flagged unresponsive**
  (`freeco-shopping`).
- ⬜ **Streamed text can show a gateway placeholder** until the reply completes.

### Chat, attachments and runtime recovery — verified 2026-09-23

Checked against the running daemon on this machine, not against the commits.

- ✅ Attachments of any supported type reach the agent. Verified: a `.docx`
  uploaded (HTTP 201, a type the old allow-list refused), staged to
  `workspaces/Developer/uploads/`, and read by the agent with `docx2txt` inside
  its sandbox — no host shell involved.
- ✅ Sandbox image `freeco-sandbox:t3` reads documents: `pdftotext`, `tesseract`
  (en/fr/de/ru), `docx2txt`, `pypdf`, `python-docx`, `openpyxl`. Verified by
  extracting text from a real PDF in a container with `--network none`.
- ✅ Model entitlement is probed at boot and persisted to `served_models.json`.
  Verified: NVIDIA shows 70 of 95 models available rather than all 95; Novita
  went from 3 visible models to 54.
- ✅ Conversation history pages instead of silently stopping at 50. Verified on
  an agent holding 284 messages: `total` and `has_more` returned, earlier
  messages loadable. The prompt sent to the model is unchanged.
- ✅ Renaming an agent moves its folder. Verified by renaming twice: one folder
  at each step, no orphan left. This is what had produced a second assistant.
- ✅ The container runtime recovers unattended. Verified by stopping the Podman
  VM: all 12 containers were back **50 seconds** later with no human action.
- ✅ Fonts are served from the binary; the dashboard makes no request to Google,
  which is also removed from its Content-Security-Policy.

Not yet verified, and recorded as such:

- ⚠️ Recovery from a real sleep/wake cycle, and the `wsl --shutdown` escalation
  it depends on. The code is in place; the state that triggers it has not been
  reproduced deliberately.
- ⚠️ A live fallback failover. Each part is verified separately — the repaired
  model id answers 200, the chain-error reporting has tests — but no real
  failover has been observed end to end.
- ⚠️ `ReloadBrowserConfig` is queued by the config watcher and handled nowhere,
  so browser settings apply only after a restart.


### Org structure — principles taken from the Yanovsky material

Seven structural principles, each with the mechanism it maps to here. Kept
separate from the rhythm work below because these decide the *shape* of the
organisation; the rhythm decides how it runs.

**1. A manager is defined by the quality of its choices, not its output.**
"Специалист ждёт постановки задачи. Руководитель сам решает, какие направления
дадут максимальный результат." Every agent here is currently the same shape: it
waits for a message and answers it. There is no agent whose job is *selection* —
deciding which two of ten things actually change the result. That is a distinct
role and needs a distinct manifest, not a bigger tool list.

**2. Authority travels with accountability, or the role is fiction.**
"Кто принимает решения — тот и руководитель." An agent named owner of an area
that must escalate every decision is a relay. Maps to `team_add_member`'s role
plus the approval gates: a team owner's manifest should state what it may decide
alone, and that boundary should be visible to the agents around it.

**3. Delegation is progressive, never one step.** Specialist work first, then
team management, then daily control. For agents this is a maturity ladder: a new
agent starts narrow, and authority widens as its record justifies it. Today
every agent is hired at full authority immediately, which is why a broad tool
list feels dangerous rather than useful.

**4. Every unit answers five questions.** From the first article: what it is
accountable for, how its work is measured, what counts as good, who decides,
what it must do for growth. A team record holding a name and nothing else — as
`Team` does now — cannot answer any of them. These are the fields a team is
missing.

**5. Structure changes when the metrics say so.** "Иногда приходится менять
структуру отдела, перераспределять ответственность." The org chart is an
instrument, not a monument. Teams and ownership must be cheap to change — which
is an argument for keeping the org model small and editable rather than modelling
a company in full.

**6. Aiming and firing are different jobs.** Eight elements are the aim, the
action is the shot. In agent terms: the agent that analyses need not be the
agent that acts, and separating them is what stops "we discussed it and nothing
changed" — the failure the source names directly.

**7. Culture is set by the questions a manager asks.** The sharpest point for
this product: "Если руководитель принимает формальные отчёты без вопросов,
сотрудники быстро понимают, что можно ограничиться красивыми словами." Agents
are *exactly* the thing that produces confident-sounding reports — this session
alone produced backups "verified" that could not restore, hands reporting Active
while serving nothing, and an agent stating another agent's model as its own. A
manager agent that accepts a subordinate's report without asking for evidence
will get pretty words every time, and it will be fast and wrong. **A reviewing
agent must ask for the check, not the claim.**

- ⬜ **Manager and specialist as distinct manifests**, differing in
  responsibility and decision rights rather than tool count (1, 2).
- ⬜ **Team gains the five fields** — accountability, measure, target, decision
  rights, growth objective (4).
- ⬜ **A review rule in the prompt**: when receiving a colleague's report, ask
  what was verified and how, before passing it on. Report the check, not the
  claim (7).
- ⬜ **An authority ladder** — new agents start narrow; widening is a decision
  the user makes (3).

### Next — the management rhythm (Yanovsky), as a mechanism

The owner supplied the actual source material. Unlike the marketing pages, it is
structural, and it maps onto an agent system unusually well — because its whole
argument is that management should be a *repeating mechanism* rather than a
person's judgement, and a mechanism is the thing software can hold.

Four ideas worth building, in order of value:

**1. The ten-element cycle.** The core of it, and explicitly "cannot be
implemented partially — it works only as a whole system":

```
1 improvement category   2 owner        3 target        4 current result
5 what was done          6 analysis     7 decisions     8 agreements
9 actions               10 follow-up
```

Every element supports the next: drop follow-up and agreements are forgotten;
drop analysis and decisions become arbitrary; drop the owner and nobody is
accountable. This is a data structure, not a philosophy — one record per
improvement area, carried forward daily.

**2. Daily, not weekly.** "За неделю можно практически ничего не изменить" —
a week is long enough to write a convincing report about a week in which nothing
changed. The question the system should force is "what got better today?", which
cannot be answered in generalities. Agents run continuously and never tire of a
daily cycle, which is the one place software beats the human version outright.

**3. Every action names the metric it moves.** The prescribed management
question is not "what will you do?" but "which indicator changes after this?".
Directly implementable: an action recorded without a target metric is
incomplete, and the tool should say so.

**4. Whoever decides is the manager.** An agent given a responsibility but no
authority is a relay, not an owner — it just escalates. Maps exactly onto our
`team_add_member` role and the approval gates: a team owner should be able to
act within its area without asking, and the boundary should be explicit.

**Deliberately not adopted:** the framing that people are secondary to numbers.
For a system that already reports confidently and wrongly, "manage the metrics"
without "verify the metrics are real" would industrialise that failure. The
existing rule stands: an accurate account beats a good-looking one.

- ⬜ **`rhythm` store and tools** — record and read the ten elements per area;
  `rhythm_record`, `rhythm_review`, `rhythm_open` (what was agreed and not yet
  done). Backed by a table, like `OrgStore`.
- ⬜ **A daily cycle the CEO agent runs** — pull results, compare to target,
  analyse deviations, decide, record agreements, and *check yesterday's
  agreements first*. Follow-up is the element most often dropped and the one
  that makes the rest real.
- ⬜ **Actions carry their metric.** Reject or flag an action recorded without
  one.
**Gradual adoption does not apply here.** The source prescribes it, and it is
right — for people. It solves human adoption friction: resistance to new rules,
forgetting to fill fields, confusion when everything changes at once. None of
that is true of agents. They fill ten fields as readily as one, never tire of a
daily cycle, and do not need a habit embedded before the next arrives. Shipping
the structure whole is not only possible here, it is *required*: a half-built
management system gives agents somewhere to record a number and nowhere to say
what it should have been, which is worse than none.

**The hands are the instruments.** An indicator a human types in is the
*«кажется, продажи выросли»* problem with extra steps — it looks like
measurement and carries the same guesswork. The service hands are what make the
numbers real, and each maps to a part of the scorecard:

| hand | indicators it can supply |
| --- | --- |
| CRM (Twenty) | contacts added, pipeline by stage, donors by status, follow-ups overdue |
| Accounting (Akaunting) | revenue, cost, margin, invoices outstanding, runway |
| Voice (Dograh) | calls placed and connected, talk time, outcomes per campaign |
| Lead generation | leads by source, cost per lead, conversion to contact |
| OmniRoute | tokens and spend per agent, provider mix, failure rate |

Two consequences worth stating. First, the hands stop being optional extras and
become a dependency of the management system — an agent asked "what got better
today?" needs somewhere to read the answer from. Second, **lead generation has no
hand at all**; it is the one instrument in that table with nothing behind it, and
without it the funnel starts mid-way.

Reading is enough to begin. A hand that can answer "how many contacts were added
this week" already supplies an indicator; writing back into these systems is a
later and much more careful question.

**The six parts, built together, because each is useless without the others:**

1. **Mission and vision**, on the company, injected into every agent's prompt.
   Alignment is not a slogan — it is the tie-breaker an agent uses when two
   actions are both defensible. Without it every agent optimises locally.
2. **Structure** — teams carrying the five fields (accountability, measure,
   target, decision rights, growth objective), so a team can answer what it is
   for.
3. **Benchmark** — what good looks like for each indicator. A number with no
   target cannot be managed, only reported.
4. **Indicators** — named, owned, with a unit and a current value. Owned by a
   team or agent, never unassigned.
5. **Heartbeat** — the daily cycle carrying the ten elements. Note the name
   collision: `heartbeat` here already means agent liveness. The management one
   needs a distinct name (`rhythm`) or the two will be confused in code.
6. **Scorecard** — current against target across a team's indicators. This is
   the artefact a manager agent actually reads before deciding.

- ⬜ Build 1–6 as one model, then the tools, then the prompt that uses them.
  The order matters only in that nothing should ship until an agent can read a
  scorecard and record an action against an indicator — that round trip is the
  smallest thing that is genuinely a management system rather than a schema.

### Next — agents that get unstuck instead of reporting stuck

Measured on 2026-08-31, across the whole system prompt:

```
                        restrictive   investigative
TOOL_CALL_BEHAVIOR            4            0
GLOBAL_GUIDELINES            20            3
OPERATIONAL_GUIDELINES        4            1
SAFETY_SECTION                2            0
                             30            4
```

Thirty prohibitions to four invitations. And of the five behaviours that decide
whether an agent is useful when something goes wrong, four are absent
altogether: what to do when a tool fails, diagnose before reporting, propose
alternatives when blocked, escalate only after trying. Nothing in the prompt
tells an agent to do anything except stop and say so — so that is what it does.

This is not a model-quality problem. On 2026-08-31 every gpt-5.6 agent had its
tools stripped by a driver retry and answered "I have no tools", which was
**correct**. The passivity that remains after that bug is a design property of
the prompt, not of the model.

- ⬜ **A "when you are blocked" section, carrying real weight.** Try the
  obvious alternative first; diagnose with the tools you have before concluding
  anything; say what you tried and what happened; propose the next step rather
  than ending on the obstacle. "I can't" is only an acceptable last line when it
  is followed by "here is what I tried and what I suggest instead".
- ⬜ **Rebalance the ratio.** Every prohibition is there for a reason and none
  should go, but a prompt that is only prohibitions produces an agent that only
  refuses. Pair each restriction with the permitted path: not "never install on
  the host", but "install in the sandbox — here is how".
- ⬜ **Self-diagnosis before escalation.** An agent that cannot do something
  should check *why* with the tools it has -- read its own config, check a
  service's health, read the log -- and report the cause, not the symptom. This
  is exactly what separated a useful diagnosis from "I have no tools" today.
- ⬜ **Learn from what worked** (the Hermes idea worth taking). After a task
  that took several attempts, record what finally worked so the next attempt
  starts there. Needs the memory-extraction work below; without it every agent
  solves the same problem from scratch forever.

### Corrected — the agents were right, the driver was wrong

An earlier version of this section claimed agents "refuse tools they hold and
invent reasons", called it the session's most important finding, and listed
three examples. **It was wrong, and it was written from a single test.**

Asking the same agent to call `hand_list` — a tool watched working an hour
earlier — produced an identical refusal. That is not an agent confabulating;
that is tool calling being broken and the agent reporting it accurately. The
phrase it used, "not available in the callable tool interface", was exactly
right and was dismissed three times as a rationalisation.

The cause: OpenAI rejects function tools combined with `reasoning_effort` on
`/v1/chat/completions`, and nothing here ever set that field — the model applies
its own default. On the resulting 400 the driver retried **without tools**,
which is the wrong half to drop: a reasoning model with no tools cannot act, and
the agent then truthfully says it has none. Every gpt-5.6 agent, every message.
Fixed by setting `reasoning_effort: "none"` and keeping the tools, checked
before the retry that strips them.

**The lesson worth keeping:** a known-good case must be tested before concluding
the witness is unreliable. "The agent is wrong" and "the product is broken" look
identical from outside, and the cheap way to tell them apart is to ask the agent
to do something already proven to work. A day went into the wrong one.

- ⬜ **Make that distinction cheap.** Log a claim of missing capability against
  the tools actually sent, so this is one query rather than a day.

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
- ✅ **Wire the data gateway into the LLM path.** Done in two steps: first the
  non-streaming loop only (background ticks), then, on 2026-09-30, the
  streaming loop the chat and popup use, plus system prompts, tool results and
  tool arguments. Until then everything typed in the chat reached the provider
  unscanned. See `docs/feature-register.md` §2.
- [ ] **`users` and `tenants` tables.** No row can say who owns it.
- ✅ **Build the sandbox image and make it the default — built, not yet the
  default.** `localhost/freeco-sandbox:t1` (2026-08-22) predated the browser
  commit, so Brave, Xvfb, x11vnc and `freeco-screen` were never in it, and
  `deploy/sandbox/Dockerfile` could not build: `USER agent` came before the
  root-only `apt-get` and `chmod` steps. Fixed, with the Windows linker
  (`gcc-mingw-w64-x86-64`) and the `x86_64-pc-windows-gnu` Rust target added.
  `:t2` is built and its contents are verified as `agent`. Still open: build a
  `.exe` inside `:t2` as `agent` and run it on Windows (the throwaway proof on
  `:t1` printed "windows"), then point `[docker] image` at the `:t2` digest.
  iOS is deliberately not in the image: Apple's SDK is licensed for Apple
  hardware and signing needs Apple's tooling, so it belongs on the macOS CI
  runners.
- [ ] Rebuild the desktop app from this repo. The installed one is from
      2026-08-12, still named `openfang-desktop`, and seizes port 50051 with
      code that predates every fix in this session.
- [ ] Prove a service tier end to end by hosting something real in it.


- ✅ Container runtime detection and the five-second probe timeout are one
  mechanism now. Two sessions built these separately; detection probes each
  candidate through the guarded check, so a wedged Docker costs five seconds
  once instead of hanging, and Podman is found when Docker does not answer.
- ✅ `docs/services.md` records the three tiers, why containers rather than
  WASM for persistent services, and what exists today versus what does not.

## Unreleased

### Spend limits and secrets.env -- from the 2026-09-14 audit

Checked by tests (`crates/freeco-kernel/tests/spend_limit_test.rs`, the
secrets tests in `freeco-types` and `freeco-api`), not yet on a running daemon.

- 🟨 Streaming calls -- the chat, the popup, the OpenAI-compatible API -- are
  refused once the agent's cost quota or the global `[budget]` cap is reached.
  Before, only the non-streaming path checked either.
- 🟨 Every `secrets.env` writer refuses line breaks and NUL in values and
  anything but letters, digits and `_` in names; the API answers 400.
- 🟨 Release builds add each target to the pinned toolchain, so the Windows
  ARM64 and macOS Intel installers and CLIs build again.
- 🟨 Release images go to the private `ghcr.io/freecodao/freeco-ai-os`; the
  public `freeco-ai` package is never pushed to.
- 🟨 MCP over HTTP caps response bodies (rmcp 3.9), and the bundled SQLite is
  3.54.1 (rusqlite 0.41), for two advisories published in November.
- ⬜ Still open: `PUT /api/budget` is not enforced until restart (enforcement
  reads the boot-time config); handlers still call `std::env::set_var`;
  `freeco-migrate` keeps its own secrets writer, which does not validate.

- 🟨 **Conversations run in parallel.** Runs take turns per conversation, not
  per agent, so different chats -- including several of one agent -- run at the
  same time, and two runs in one conversation can no longer save over each
  other. Verified by `crates/freeco-kernel/tests/parallel_conversations_test.rs`
  against a scripted model: it fails with the old unlocked streaming path.
- 🟨 **`agent_delegate`** runs several tasks on several agents at once, each in
  a fresh conversation filed under the lead's project. Still one canonical
  history per agent: a delegated task is appended to it like any other turn.
- 🟨 **Projects page** edits the brief every agent in the project reads, starts
  conversations inside a project and shows which are working.
- 🟨 **Teams run like a lead with teammates.** A goal given to a team goes to
  its lead in a new conversation; the lead hands parts to members with
  `agent_delegate`, they run at once, and every member's prompt carries the
  roster. The delegated conversations are the team's task list on the Projects
  page.
- 🟨 **Team task boards and the Teams page.** `task_post` puts a task on a
  team's board, where only members can claim it, and holds it until the tasks
  it is `blocked_by` are completed; only the holder or the poster completes it.
  The Teams page shows each team as a room: a seat is lit only while a run holds
  one of that agent's conversations, and hand-offs (`agent_delegate`,
  `agent_send`) are drawn between seats from the kernel's record. Verified by
  `team_board_respects_blockers_membership_and_holder` and the hand-off tests in
  `parallel_conversations_test.rs`. The hand-off record is in memory and starts
  empty after a restart.
- 🟨 **A message from another agent is marked as not from the user**, so a
  colleague's request cannot pass as the owner's approval.
- 🟨 **`web_fetch` checks every redirect** against the same SSRF rules as the
  first URL; the fallback fetch now has them too.
- 🟨 **The WhatsApp gateway answers only allowed chats** (groups off unless
  listed, optional number allowlist) and gives each contact a conversation of
  its own. The kernel still keys a conversation's history by agent, so a guest
  is kept apart by the gateway choosing the conversation, not by the kernel.
- 🟨 **Long chats keep their start in view.** What the history window drops is
  carried in the system prompt: the user's words verbatim (instructions kept
  longest when the budget is short), the agent's replies in short. Free and
  deterministic; no summarising call.
- 🟨 **A failed or stopped reply keeps the user's message**, saved before the
  model is called.
- ⚠️ Stopping a run still stops the agent's most recent one; with parallel
  conversations that may not be the one on screen.
- ⚠️ A chat of several hundred messages carries its earliest ordinary messages
  only as a count once the note's budget is full; instructions are kept
  longest. A paid summary for that case is not built.

### OmniRoute as the one gateway — 2026-11-14

Live now (runtime config, no build needed):

- ✅ OmniRoute bound to 127.0.0.1 only (`OMNIROUTE_HOSTNAME`; `HOSTNAME` is
  overwritten by the image's launcher). Sandboxes still reach it via pasta `-T`.
- ✅ Connections: Claude Code subscription (`cc/`, setup token), Novita (live
  catalog, 101 models), NVIDIA (live catalog, free-only filter off), OpenAI.
- ✅ Claude Code (`freeco-claude`, its own container, pasta forwarding only
  20128) runs through OmniRoute: opus/sonnet/haiku → `freeco-cc-heavy` /
  `-main` / `-easy`, Claude first, then Novita/NVIDIA. Failover verified with
  the Claude connection switched off.
- ✅ FreEco agents run through OmniRoute: CEO → `freeco-agent-ceo`, worker 1 →
  `freeco-agent-worker`, hands → `freeco-agent-hands` (each keeps its old model
  as step 1). The `omniroute` agent stays direct so it works when the gateway
  is down.
- ✅ Compression: session-dedup + lite, standard above 64k; system prompt never
  compressed. OmniRoute advertises only connected providers (allowlist,
  blocked keyless providers, auto combos hidden).
- ✅ OmniRoute MCP on (streamable HTTP, loopback); scoped key for the hand;
  registered in config.toml.

In the repo (2026-11-18):

- ✅ The bundled `omniroute` hand uses OmniRoute's MCP tools
  (`mcp_servers = ["omniroute"]`, no shell) with the generic caretaker prompt;
  the owner's combos and connections stay in the live override.
- ✅ `${VAR}` in `[[mcp_servers]] headers` is resolved from the environment or
  the vault, so the OmniRoute MCP key no longer has to sit in config.toml in
  plain text. Example in `freeco-ai.toml.example` and `docs/configuration.md`.
- ✅ `containers` is a bundled agent template (`agents/containers`):
  podman-only `[exec_policy]`, approval on every `shell_exec`, no host-specific
  container list.

Open:

- ⬜ **Daemon rebuild needed** to ship: fallback picker rendering nothing
  (duplicate x-for keys across providers), `/api/models?available=true`
  offering models that are not ready (offline keyless servers, gateway routes
  with no provider), and to activate the `omniroute` MCP server — then attach
  it to the `omniroute` agent (`PUT /api/agents/{id}/mcp_servers`). Fold in the
  unmerged `claude/project-thread-*` branches first.
- ⬜ **Security approvals have no UI.** `POST /api/security/approve/{hash}` is
  never called by the dashboard, so a manifest with `shell_exec` can never be
  approved by clicking. (The `containers` agent was approved through the API
  on the owner's chat go-ahead, hash `5e6652e8…`.)
- ⬜ **Hands cannot safely manage their own containers.** A hand that lists
  `shell_exec` in HAND.toml gets `exec_policy` mode **Full** (any command;
  `activate_hand`: "Hands are curated packages"). Needs a per-hand
  `[exec_policy]` in HAND.toml (e.g. podman-only) and a container-name scope
  checked by the kernel. Until then the `containers` agent does container
  work. Also: hand agents are rebuilt from HAND.toml on every boot, so their
  `agent.toml` edits are silently discarded (overrides go in
  `~/.freeco-ai/hands/<id>/HAND.toml`; the `omniroute` hand now has one with
  its OmniRoute MCP tools).
- ⬜ **MCP servers are connected once at boot, with no retry.** The daemon
  started 2 s before OmniRoute and never connected; `POST
  /api/integrations/reload` fixed it by hand.
- ⬜ **`capabilities.shell` is dead wiring.** It becomes `Capability::ShellExec`
  and nothing checks it; only the agent's `[exec_policy]` limits commands.
- 🟨 **Containers hand** (`containers`, live 2026-11-14): kernel-enforced —
  approval on every `shell_exec` (verified live), no shell metacharacters,
  `[exec_policy]` allowlist = `podman` only, five tools, no agent spawning.
  Prompt-only (not enforced) — FreecoDAO images only, `freeco-*` containers
  only, backup/rollback, 127.0.0.1 binding, never touching `freeco-daemon`.
  `podman` with any arguments is still powerful (`run --privileged`, host
  mounts); next: read-only podman without a click, mutating podman with one,
  and an image/argument allowlist checked by the kernel.
- ⬜ **OmniRoute from the FreecoDAO fork.** `FreecoDAO/OmniRoute` is on
  `release/v3.8.50` (upstream v3.8.54) and its docker-publish run failed
  (2026-08-27); the container still runs `docker.io/diegosouzapw/omniroute`.
- ⬜ CEO worker 2 (`claude-opus-5-5@medium`) reaches Claude via an OmniRoute
  alias with no fallback: `@` is not allowed in combo names.
- ⬜ OmniRoute's dashboard has no login password; loopback binding is the only
  thing closing it.

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

- ✅ **Four crates nothing linked, removed (Nov 2026).** `freeco-learning` (a
  replay / scoring / promotion self-improvement loop), `freeco-runtime` (a
  re-export wrapper), `freeco-budget-engine` (tiers from an old plan: Free,
  Concierge, Business at CHF 22) and `freeco-input-parser` (listed by the
  Shopping agent, never imported) had no callers. They are in git history if
  wanted. Plan budgets belong in `freeco-kernel/src/metering.rs`, which is the
  budget the daemon enforces.
- ⚠️ **A complete OAuth PKCE flow, wired to nothing.**
  `freeco-extensions/src/oauth.rs:128` — `run_pkce_flow()` with zeroizing token
  accessors beside it. The only OAuth actually reachable is Copilot's device
  flow. Social login is listed below as unbuilt; the hard part of it is already
  written. 35 other public functions have no caller anywhere either.

## Found 2026-09-14/15 — scheduled, not done

_Update 2026-09-30: the cross-site request fix (`csrf.rs`, verified live) and
the `sandbox_exec` rename are done; the other items stand._

- **Cross-site requests reach the API.** With no `api_key` and dashboard auth
  off, `middleware::auth` (`middleware.rs:141-143`) admits every loopback
  request, and nothing in `freeco-api` checks `Origin`. A page open in the
  user's browser is loopback, so a cross-site `<form method=post>` reaches the
  35 POST handlers that take no JSON body, with no preflight. Among them are
  `/api/shutdown`, `/api/auth/disable`, `/api/system/unfreeze`,
  `/api/services/{id}/install`, `/api/hands/{id}/install-deps`,
  `/api/sandbox/pull` and agent stop/restart/session-reset. Fix: one middleware
  that rejects state-changing requests whose `Origin` is present and foreign,
  plus a JSON content-type requirement on body-less POSTs. The new
  `/api/services/vm-fix/{id}` is built with that guard from the start. Until
  then, setting an `api_key` or enabling dashboard login closes it.
- **No privileged self-repair without the user.** Decided 2026-09-15 (option B):
  the daemon detects a disabled `podman-restart.service` or a missing VM
  `resolv.conf`, reports it in Settings, and runs the one fixed repair only when
  the user clicks. Previously `ensure_vm_resolver` ran `sudo` in the VM
  silently, and the restart-service fix was written to do the same.
- **Agents cannot coordinate shared work.** `agent_send`, `task_post` /
  `task_claim` / `task_complete` and `event_publish` exist, but there are no
  claims on shared resources (the build directory, the Podman VM, a set of
  files), no notification when a claimed task finishes, and nothing stops one
  agent treating another's message as approval. `agent_send` also timed out
  after 600 s on a real call. Planned: the CEO Assistant owns the task board
  and resource claims, and escalates approvals to the user instead of granting
  them.
- **`docker_exec` is named for the wrong runtime.** It runs whatever
  `[docker] runtime` says, which on this machine is Podman. To be renamed
  `sandbox_exec`, with `docker_exec` kept as an alias. `capabilities.tools` is
  matched exactly (`kernel.rs:6639`) and existing manifests list the old name.
- **`cargo fmt --check` was already failing at `d30e66d`.** CI runs it. The
  formatting goes in its own commit, separate from functional changes, so the
  real diff stays reviewable.

## Debt found by the 2026-09-12 scan — scheduled, not done

Full ledger: eighteen findings over 172,694 lines. Fixed in this cycle:
findings 1 (global spend cap unenforced) and 12 (stale CLAUDE.md figures).
The rest, in priority order, with nothing here described as done:

- **02 — `env::set_var` in live HTTP handlers.** `routes.rs:3154` and `:3238`,
  both `unsafe`, both justified with "SAFETY: single-threaded config operation"
  on axum handlers running on a multi-threaded Tokio runtime. `setenv` is not
  thread-safe; a concurrent `getenv` anywhere — including libc's own during DNS
  or timezone lookup — can read freed memory. Fix is an `AppState` secret
  overlay consulted before `std::env::var`, then both `unsafe` blocks go. Two
  days, and deliberately not bundled with unrelated work: a half-migrated
  overlay is worse than the current state, because readers would disagree about
  where the truth is.
- **04 — 216 API handlers, test ratio 0.02.** `server.rs` has 214 route
  registrations and zero tests. The eight-step manual checklist in CLAUDE.md
  exists because it is genuinely needed, and it runs only when someone
  remembers. An `axum_test` harness over the 30 handlers the dashboard calls on
  load turns it into a CI check.
- **03 — MSRV 1.75 declared, never verified.** Nothing pins or tests it, and
  `unsafe { env::set_var }` needs 1.82, so the published floor appears wrong.
- **05 — pull requests build Linux only.** Windows is the primary development
  platform here and regressions in it are found after merge.
- **06 — two reqwest and two tungstenite stacks linked.**
  `freeco-kernel-runtime` pins `tokio-tungstenite = "0.24"` literally instead of
  `workspace = true`, so it inherits default features in a tree that is
  deliberately rustls-only.
- **07 — vendored JS with no version or integrity record.** Five minified
  libraries ship in the product and execute in the dashboard; nothing records
  which versions they are and `cargo audit` does not cover them.
- **15 — 24,159 frontend lines with no lint, test or build step.**
- **14, 13, 16 — structural.** 48 channel adapters repeating one lifecycle by
  hand; a 4,986-line `config.rs` in the crate 14 others depend on; `routes.rs`
  and `kernel.rs` absorbing 276 commits a year between them. All are carve jobs
  to attempt only after finding 04's harness exists.
- **17 — `freeco-cli` untested (18,109 lines).** Deferred on purpose while the
  interactive CLI is being rewritten.

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
- 🟨 **AI gateway.** `freeco-datagateway` exists and is on every provider call
  (see above). It masks known key formats, JWTs, announced secrets, cards,
  IBANs, emails and IPv4 addresses. It is a filter on text, not a guarantee:
  it cannot stop an agent composing a secret itself, and does not read binary
  attachments.
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
