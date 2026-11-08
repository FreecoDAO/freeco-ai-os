# Changelog

All notable changes to FreEco.ai will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Agents work in parallel. Each conversation has its own queue, so one agent
  can answer a WhatsApp guest, a dashboard chat and a delegated task at the
  same time, and different agents never wait for each other.
- `agent_delegate`: a lead agent hands several tasks to several agents at once
  (or to fresh conversations of itself) and gets every result back together.
  Each task runs in a conversation of its own, filed under the lead's project,
  where the user can open and follow it. Any agent allowed `agent_send` may use
  it.
- Projects you can work in. A project's brief is read by every agent working
  in one of its conversations; the Projects page edits the brief, starts a
  conversation with any agent inside the project, shows which conversations are
  working right now, and opens each one.

### Fixed

- Messages no longer vanish when two things talk to one agent at once. The
  streaming path (dashboard chat and popup) took no lock, so a reply running
  while a channel message or a second window arrived saved over the other's
  messages. The agent's shared history had the same race.
- Switching chats no longer stalls or drops messages. The dashboard had one
  "sending" flag and one queue for every chat, so a reply running in one chat
  queued every message typed into any other behind it, and leaving a chat
  mid-reply could strand its queue for good. Each chat now keeps its own; what
  was queued is still delivered after switching away, and a chat opened while
  its agent is still working says so and shows the reply when it lands.
- A reply can no longer be drawn into the wrong chat. Frames from the socket of
  the chat just left were still rendered into the next one.
- A message lands in the conversation it was typed in, even after another
  window switched the agent to a different conversation.
- A message waiting behind a reply goes to the new conversation when the old
  one is reset meanwhile, instead of bringing the reset one back. A streamed
  message in that spot is refused with a note to send it again.
- An agent asked a question by an agent it is itself waiting on is told so,
  instead of both waiting for each other forever.
- `project_create` stored the description as the company id.
- Opening an agent's chat from a link (a project's Open button, the assistant's
  "open the chat" links) while another chat was open landed on the agent list
  instead of the chat.

## [0.9.8] - 2026-10-28

### Fixed

- A reached limit hands over to the fallback chain. Rate limits and used-up
  allowances (a Claude subscription's 5-hour limit, a provider's quota) never
  fell back, so the agent stopped until the limit reset even with GLM and
  Nemotron configured behind it. A wrong or revoked key still does not.
- The Claude Code provider passes the conversation on standard input. It was
  a command-line argument, which Windows caps near 32,000 characters, so any
  real conversation failed to start.
- The data gateway now protects the chat. It masked secrets only on the
  non-streaming path, which background ticks use; the dashboard chat and the
  popup stream, so everything typed there reached the model provider unscanned
  while the log showed the gateway busy. It now also covers the system prompt,
  tool results and tool arguments (only plain-text messages were scanned), and
  restores real values in tool-call arguments so a masked address or key still
  works when a tool uses it.
- A number that passes the card checksum no longer bricks a conversation.
  Cards and private keys were refused outright; the text stays in history, so
  every later request was refused too. A phone number in a tool call did that
  to the CEO's chat. Both are now masked (restored in replies and tool
  arguments), and digits running into letters -- message ids, hashes -- are
  not read as cards at all.
- Dictation works with a local speech server. FreEco.ai always asked for
  OpenAI's `whisper-1`, which speaches maps to a model not installed here, so
  every dictation failed with a 404. `[media] audio_model` names the model to
  request.
- Read-aloud in the main chat no longer says "asterisk asterisk". Markdown is
  stripped before speaking, as the popup already did, and the main chat uses
  the voice chosen in the assistant (or the best local "Natural" voice for the
  language) instead of the browser's default robotic one.
- The gateway recognises JSON Web Tokens -- how self-hosted services such as
  Twenty issue API keys, with no vendor prefix -- and values announced as
  secrets ("CRM key: …", `CRM_API_KEY=…`, "password is …"). Ordinary prose
  around the same words is left alone.
- Agents can reach the services FreEco.ai installs from their sandbox. The
  services listen on the VM's loopback, which a sandbox on podman's default
  network cannot see: tested, every one refused, and agents asked for a host
  shell instead. A networked podman sandbox now gets exactly the service ports
  forwarded (`[docker] service_ports`, default CRM 3200, voice 3010/8000,
  speech 8010, accounting 8080) and no other host port. The voice, speech,
  accounting and CRM agents get `sandbox_exec`, and are told the addresses.
- Backups: the database's write-ahead log is checkpointed first (uncheckpointed
  it had reached 106 MB beside a 24 MB database); an archive a restore would
  refuse is no longer written -- it was reported as a success, and pruning then
  deleted the oldest good archive to make room; the restore limit is 512 MB.

- Conversations show up in the chat again. The chat loaded the agent's compact
  cross-channel memory instead of the conversation it is in, and built those
  messages in a shape the page does not draw -- so whole conversations looked
  empty (the CEO's chat showed none of its 116 messages) and "load earlier" fetched more of
  the wrong thing. It now opens the agent's current conversation, tool steps
  included, draws the latest 200 and shows the rest instantly on request; the
  switcher names the conversation you are in.
- Paid models were recorded as free. GLM 5.3, 5.3 Flash and 5.2 on Novita had
  a price of 0 in the catalog, so 26 M+ tokens cost "$0"; they now carry
  Novita's published prices.
- The Budget page shows where the money went: spend, calls and tokens by
  agent, team, project, company and model, for today, 7 days, 30 days or all
  time, from the stored calls. Its agent list only included agents with a
  spending limit and was empty; per-agent usage read an in-memory counter that
  starts at zero on every restart. Calls recorded at $0 before a price was
  known are estimated at today's price and marked.
- The assistant button no longer trembles. Its logo bobbed and rotated every
  3.4 seconds, and wiggled continuously under the mouse.
- Pausing a hand stops it. Pause only set a flag nothing read, so a paused
  agent kept waking every hour and spending tokens; a paused hand was also
  dropped from the saved state and gone after a restart. Its agent's loop now
  stops, resumes on resume, and stays paused across restarts. The service
  container keeps running: the agent sleeps, the service does not.
- A reactive agent with an `[autonomous]` block (freeco-shopping) is no longer
  flagged unresponsive every minute. The block carries limits, not a schedule.
- Agents that list `docker_exec` see the sandbox tool again. The rename to
  `sandbox_exec` was honoured when running a tool but not when choosing which
  tools to show the model, so those agents silently lost their sandbox.
- Deleting or restoring a conversation never destroys it without a copy. A
  failed archive copy was ignored and the conversation cleared anyway.
- The service-data backup no longer deletes real backups. A day with the
  container runtime down left an empty run directory that pruning counted as a
  backup; one container that could not be inspected aborted the whole run.
- The chat's file panel shows workspace-relative paths on Windows. Comparing a
  `\\?\`-prefixed root with plain paths never matched.
- Saving keys from a chat message no longer overwrites one with another: two
  keys in one message get distinct names, the dialog says when a name already
  holds a key, and system variables such as `PATH` are refused as names.
- The cross-site request guard works behind a reverse proxy. It required a
  loopback `Host` on every write, which only defends a daemon listening on
  loopback; anywhere else it refused all writes.

### Security

- With dashboard login on and no API key set, an empty token was accepted:
  the empty key and `Authorization: Bearer ` with nothing after it compared
  equal. An empty key now matches nothing.
- The old `POST /mcp` is removed. It ran any built-in tool -- `shell_exec`
  included -- with no token and no permission check. It is replaced by the
  MCP endpoint below, which needs a token and acts with one agent's own tools.
  (Registered twice, the two would also have stopped the daemon starting.)

- The container watchdog no longer takes down a healthy Podman VM. It treated a
  failed `podman version` probe as proof the runtime was dead, and escalated to
  `podman machine stop` and then `wsl --shutdown` -- but that probe also fails
  while the VM is perfectly well, and the daemon had once reported no runtime
  with twelve containers running. With the daemon stopped the VM stayed up; with
  it running, the VM went down within minutes, repeatedly. Nothing destructive
  now happens unless the VM's SSH port is confirmed to refuse connections from
  Windows, which is what actually distinguishes the post-sleep failure. The
  watchdog also writes `logs/watchdog.json` on every tick -- what it saw and what
  it did -- because a daemon launched without its output captured left no way
  to tell whether it had tried at all.

- Scheduled agents are no longer declared crashed between their own runs. The
  heartbeat judged every agent by its heartbeat interval alone, so a service hand
  told to run once an hour was marked unresponsive after sixty idle seconds,
  marked Crashed, and "recovered" -- about 860 times across five agents in three
  days. An agent is now late only once its scheduled run is overdue.

- Pressing Enter twice in the assistant no longer sends the message twice. The
  guard against a second send only closed after an awaited call to the session
  router; for as long as that took, the message was not yet on screen and a
  second Enter ran the whole path again. The message now appears at once and
  the guard closes before anything is awaited. A message sent but not yet
  answered also survives a reload or a model switch.

- Dictation works in Brave. The browser defines speech recognition but removed
  the service behind it, so it failed with `language-not-supported` every time,
  and the record-and-upload fallback -- which only ran when recognition was
  absent -- never did. A failure that will not change now switches to recording.

- File tools cannot reach outside a workspace. With no workspace set, the only
  check rejected `..`, so any absolute path was readable and writable,
  `~/.freeco-ai/secrets.env` included. They now refuse and say why.

- A local speech service is no longer sent the OpenAI key. Pointing
  `audio_base_url` or `tts_openai_base_url` at a local server still built the
  Authorization header from `OPENAI_API_KEY`: the wrong credential for that
  server, and one that should only ever go to OpenAI. `local_audio_api_key_env`
  names the key to use instead.

- Approving one of several waiting requests no longer announces the rest again.
  The alert compared the whole set, so any change -- a removal included --
  re-fired it for requests already on screen.

- Browser settings apply without a restart. `[browser]` changes were detected and
  queued as a hot-reload action that nothing handled.

- Attachments reach the agent whatever their type. Only images were passed on:
  every other upload - a PDF, a CV, a spreadsheet - was dropped without a word
  by the code that resolved attachments. The upload succeeded and the file
  appeared in the chat, so the agent was asked about a document it had never
  received, and went looking for it across the user's disk with a host shell.
  Every attachment is now copied into the agent's own workspace under
  `uploads/`, and the message carries both paths: the workspace one and
  `/workspace/uploads/<name>` inside its sandbox. Word, Excel, PowerPoint and
  OpenDocument files are accepted alongside images, text, audio and PDF, and the
  sandbox image ships `pdftotext`, `tesseract` (en/fr/de/ru), `docx2txt`,
  `pypdf`, `python-docx` and `openpyxl` so an agent can read them without asking
  for anything on the host.

- Renaming an agent moves its folder instead of leaving a copy behind. The
  manifest is mirrored to `agents/<name>/`, and boot spawns every folder there
  whose name no agent holds - so a rename left a folder that the next restart
  turned into a second agent sharing the first one's workspace. That is how the
  assistant came to exist twice. If a folder already stands under the new name,
  the old one is archived rather than deleted.

- The Podman VM comes back after the machine sleeps. Waking is detected from the
  jump in wall-clock time between watchdog ticks, which clears the ten-minute
  backoff that could otherwise leave every service down long after the user
  returned. When starting the VM fails the way it does after sleep - the VM
  running and sshd listening inside it, but WSL's port forwarding gone, so
  `machine start` reports "not listening on ssh port" - WSL itself is restarted,
  at most once every ten minutes.

- Runtime commands no longer outlive their timeout. Every `podman` call is
  spawned with `kill_on_drop`, so one that hangs is killed rather than left
  holding Podman's lock while every later call queues behind it - which is how
  the daemon came to report no container runtime while twelve containers ran.

- A failed fallback chain reports every link, in the order tried. Only the last
  error was kept, so a primary out of credit followed by a backup whose model id
  does not exist surfaced as "404 page not found" - naming a model the user
  never chose, and saying nothing about the balance that actually stopped the
  turn. Retryable errors keep their variant so the agent loop still backs off,
  and a chain of one reports its own error unchanged.

- The dashboard no longer asks Google for its fonts. Inter and Geist Mono were
  fetched from fonts.googleapis.com on every page view, sending the viewer's
  address and browser to a third party. Both are SIL Open Font License 1.1 and
  now ship with the binary, and Google is gone from the page's
  Content-Security-Policy so it cannot return unnoticed.

- An agent's configured key can no longer be blanked by opening its settings.
  The provider list loaded only when the key field was focused, so the first
  render offered a single option - "None needed (local model)" - and the browser
  wrote that blank over the real value.

- An agent keeps its sandbox across a daemon restart. The container name is
  fixed per agent and workspace, but the pool that remembers containers lives
  in memory, so after any restart, rebuild, or idle hibernation the next
  `docker_exec` ran `run --name <same name>` and failed with "name is already
  in use". The agent had lost its sandbox until someone deleted the container
  by hand. Sandboxes now carry a fingerprint label. A container with a matching
  fingerprint is picked up again (and started if it was stopped). One with a
  different or missing fingerprint is replaced, which is safe because
  `/workspace` is a bind mount.

- `npm install` works inside the sandbox. The root filesystem and `$HOME` are
  read-only, so npm exited 1 trying to write `~/.npm`, and pip ran with its
  cache silently disabled. The caches now point at the writable workspace, or
  at `/tmp` for a read-only sandbox.

- Service containers come back by themselves after the Podman VM restarts.
  Their `unless-stopped` policy is only honoured at boot by
  `podman-restart.service`, which ships disabled, so every reboot left CRM,
  voice and OmniRoute exited until someone started them by hand. The daemon now
  detects when the unit is off and reports it on Settings > Services, with the
  exact command and a Fix button. It runs sudo inside the VM only when you
  press Fix, and a missing /etc/resolv.conf is handled the same way instead of
  being repaired silently. Verified: after a VM restart, all ten service
  containers returned on their own within about ninety seconds.

- A Podman VM that reports "already running" while its API refuses every
  command is now recovered. The VM stayed up but its SSH port forward had died,
  and another process had taken the port. The runtime watchdog treated "already
  running" as success and retried every ten minutes, forever. This is how the
  container runtime stayed dead for two weeks. It now checks the API, and when
  the API is dead it cycles the VM, which re-establishes the forward.

- The global spend cap actually stops spending. `check_global_budget` existed,
  read the configured hourly and daily ceilings, and returned `QuotaExceeded` --
  and had no caller anywhere in the repository, so the dashboard displayed a
  limit that nothing could enforce. It is now checked on every agent run
  alongside the per-agent quota, which cannot substitute for it because the bill
  is the sum over all agents. A single conversation had spent $12 against a
  provider while the recorded month-to-date total read $0.33.

- A transient provider fault no longer takes an agent's tools away. Any `500`,
  or the words "internal error" anywhere in a response body, used to clear every
  tool from the request and retry without them; the agent then had no way to act
  and narrated instead -- "I am checking that now" -- returning having done
  nothing, which reads as a model that will not work. Server errors are now
  retried unchanged, and tools are dropped only when the provider states
  explicitly that it does not support them. The reasoning-conflict message
  ("Function tools with reasoning_effort are not supported for ...") contains
  both "tool" and "not supported" and was matching that rule as well, so a
  gpt-5.6 agent lost its tools on the retry after the one that switched
  reasoning off.

- Switching model in the chat keeps the provider. The picker sent only the model
  id, so the server resolved it across the whole catalogue and preferred
  Custom-tier entries -- `gpt-5.6-sol` could land on the OpenRouter duplicate and
  then look for `OPENROUTER_API_KEY`, reported to the user as a local model that
  needs no key. The provider-aware path already existed and was being bypassed.

- Services keep their containers and their data across restarts. Container and
  volume names were keyed by the activation instance, and a new instance id was
  minted on every boot, so each restart built a fresh container on an empty
  volume and the CRM set up the day before came back blank. Names are now keyed
  by hand and tenant, which survives a restart, and containers left behind by a
  superseded instance are removed instead of accumulating.

- Tool calling works on reasoning models. On `/v1/chat/completions` the OpenAI
  reasoning default is rejected alongside function tools, and the driver
  responded by stripping every tool from the request -- so agents held tools
  they could not call and explained, plausibly, that they had none. Reasoning is
  now switched off for that endpoint instead, keeping the tools. This is a
  workaround, not a cure: the models reason less well as a result, and the real
  fix is to route them through the Responses API.

- The gateway keeps its configuration across a restart, and replies say which
  model actually answered rather than which model was requested -- the two
  differ whenever a fallback fires, which made an outage look like a quality
  regression.

- Approval requests say what the agent is trying to achieve. Every one of them
  read "Agent \<uuid\> requests to execute shell_exec", because the per-tool
  fallback text was documented as covering "requests that arrive without
  agent-supplied reasoning" and `request_approval` had no parameter to carry
  such reasoning — so the fallback was the only path that could run. Agents now
  pass a `purpose`, required by the `shell_exec` schema, describing the goal and
  why the sandbox cannot do it. Their system prompt makes the sandbox the
  default and the host a last resort.
- The chat picked the wrong agent. It chose the first whose name contained
  "freeco" — the product name, which every bundled agent carries — so in a
  registry where `freeco-shopping` sorted first, messages typed to the assistant
  were answered by the shopping agent. `GET /api/assistant` now answers by role,
  adopts the agent it resolves as CEO, and persists that, so the guess happens
  once rather than on every page load.
- Idle agents are no longer treated as crashed. The guard against the
  crash-recover loop compared `last_active` to `created_at`, so it only ever
  protected agents that had never been used; the moment one was chatted with,
  idleness read as a crash. One daemon run performed 94 recoveries across six
  agents, including every agent the user had switched off. Reactive agents are
  idle by design and are no longer liveness-checked; genuine crashes still
  recover.
- Newer OpenAI models work. They accept only their default temperature, so an
  agent temperature of 0.3 failed with 400 — gpt-5.4 accepted it and 5.5 and 5.6
  did not, which read as a broken model family. The driver now retries without
  the temperature, and does so before the retry that clears tools, which was
  "handling" the rejection by silently stripping the agent's capabilities while
  failing for the same reason.
- Model ids keep their provider prefix in fallback chains. Only the primary
  model was normalised, so a stored NVIDIA fallback read back as
  `nemotron-3-ultra-550b-a55b` and NVIDIA answered with a plain-text `404 page
  not found` — an error naming a provider and a model the user never chose. The
  catalog is now asked whether the prefixed form is the real id, which repairs
  existing rows on read.

### Changed

- The sandbox tool is `sandbox_exec`. It was named `docker_exec` when Docker was
  the only runtime, and the name sent agents looking for Docker. The old name
  still works, and a manifest listing it keeps the capability: tool permissions
  are now compared by canonical name on both sides.

- dograh runs from FreecoDAO/dograh, the audited fork, built by that fork's CI
  and pinned to the commit. It ran upstream's `latest` images, so the code
  running was neither the code that was read nor a version anyone could name.

- The assistant everyone talks to is first in every agent list, found by its
  role rather than guessed from its name.

- A refused host `shell_exec` says where the command can run instead:
  `sandbox_exec` accepts pipes and substitution because nothing it runs can
  reach this machine. A timed-out `agent_send` says the other agent's model is
  the usual cause, instead of inviting another ten-minute wait.

- Model availability is learned at boot rather than assumed. Each provider is
  asked which models its key may actually call and the answer is persisted, so
  the picker stops offering models that return "does not have access". The probe
  existed already, but only a button in Settings ever called it.

- Models are chosen from a list everywhere they are configured - agent config,
  the fallback chain and the spawn form - instead of typed. A fallback entered as
  "provider/model" was split on the slash, discarding the "nvidia/" that is part
  of NVIDIA's real model ids, so the backup answered 404 at exactly the moment it
  was needed.

- The chat shows what the agent remembers. History was capped at the model's
  50-message replay window, so a conversation of 284 messages displayed the last
  50 with no sign the rest existed. It now pages, reports the total and offers the
  earlier messages. What the model receives is unchanged, so this costs no tokens.
  A message sent but not yet answered survives a reload instead of vanishing.

- One loader owns the conversation view. Two ran against different stores and
  raced, so the thread came back whole, truncated or empty depending on timing.

- The assistant widget shows the model the assistant is actually using rather
  than the model new agents are created with, so one assistant stops looking like
  two.

- The composer says whether replies are streaming or arriving over plain HTTP,
  which until now degraded silently when the socket gave up reconnecting.

- Every agent follows a "Truth And Evidence" standard: name what a claim
  rests on, never invent a citation, line number or statistic, separate what
  was verified from what was inferred, treat "I cannot verify this" as a
  complete answer, show the working, and check before finishing. It replaces
  three overlapping lines rather than repeating them, so the shared
  instructions grow by about 200 tokens per request rather than 360.
- The stated minimum Rust version is 1.87, not 1.75. The code already needed
  1.87 (`u32::is_multiple_of`), so packagers were told something false. The
  exact floor is not yet measured. The toolchain is pinned to 1.96.1 instead
  of floating `stable`, so a Rust release cannot break the build without a
  commit here.
- The vendored dashboard libraries (Alpine.js 3.15.8, Chart.js 4.4.7,
  highlight.js 11.11.1, marked 15.0.12) are recorded with their SHA-256 in
  `static/vendor/VERSIONS.md` and `SHA256SUMS`, and CI checks them; they ship
  in the binary and no audit tool covered them. They are kept byte-identical on
  every platform (`.gitattributes`), since line-ending conversion changed them.
- CI lints test code too (`clippy --all-targets`), as the project rules require.
- `tokio-tungstenite` comes from the workspace, so it cannot pull a native TLS
  stack into a tree that is deliberately rustls-only.
- The global spend cap has a regression test; it is enforced and nothing
  checked that it stays so. `docs/ui-coverage-audit.md` is marked superseded:
  it lists 157 routes, and there are 237.

### Added

- "New task" in the popup's routing card puts the task on the task board,
  assigned to the agent, and starts its own conversation. Routing stays in the
  popup; the main chat sends to the conversation on screen.
- `/?widget=1` shows only the assistant, filling its window: what the
  "FreEco Assistant" desktop shortcut opens.
- Claude Code and Codex can join the FreEco team. `POST /mcp` is an MCP
  endpoint with ten tools -- task board, agent conversations, asking an agent,
  the agent's sandbox, the CRM -- and `integrations/claude-code-plugin`
  installs it in Claude Code (Codex setup documented). The client acts as a
  named agent with that agent's own permissions, completes only tasks that
  agent holds, and the endpoint refuses everything until `FREECO_MCP_TOKEN` is
  set.

- Claude can be an agent's model and use FreEco's tools. The `claude-code`
  provider gave the model no tools and dropped earlier tool calls and results,
  so a Claude-backed agent could talk but not act. It now receives the agent's
  tool list, its calls come back as real tool calls that FreEco runs with the
  usual checks, and Claude Code's own built-in tools are switched off.
- The Claude CLI runs in a container. `base_url = "podman:freeco-claude"`
  runs it through `podman exec` in a container built from
  `deploy/claude/Containerfile`, not on the host; the owner's token
  (`CLAUDE_CODE_OAUTH_TOKEN`, from `claude setup-token`) is passed per call and
  never stored in the container.
- The CRM is usable by agents. `crm_api` reads and changes Twenty's tasks (with
  due dates: the owner's schedule), people, companies, notes and opportunities.
  It reaches one fixed local address, only the CRM's API paths, with the key
  read from the vault by the tool, so the key never passes through a model.
  Services -> Twenty CRM has a Connect field that checks the key against the
  CRM before storing it. Previously there was no way to connect it: the crm
  agent had only `web_fetch`, which refuses local addresses.
- Chat and popup stop a message that contains a key. They offer to save it in
  the vault under a suggested name (`CRM_API_KEY` when the message mentions the
  CRM) and send the message with the key replaced by that name.

- The assistant routes work to where it already lives. Each message is checked
  against existing conversations by theme (`GET /api/sessions/search`) and
  against the current thread in parallel; when there is a real alternative it
  offers it -- an existing conversation, a new task, a new project, a new
  company, or continuing here -- and delivers only when one is chosen.

- Actions that travel. The assistant's Ask, Translate and Shell chips can be
  dragged into another app or page, or copied: a drag carries a link, an HTML
  anchor and a ready `curl` command at once. `/?ask=...&agent=...` opens the
  dashboard with the text prefilled, never sent.

- Dictation language: browser default, English (US and UK), Ukrainian, French,
  German and Italian. Spoken replies use the same language.

- Spoken replies in the main chat, sharing the assistant's setting.

- Approval alerts with sound and a desktop notification when the dashboard is
  not the window in front, with a switch in Settings.

- Cross-site request forgery protection on every state-changing request. With
  dashboard auth off every loopback request is trusted, and a page on any site
  reaches 127.0.0.1 like any other client; only one endpoint was guarded.

- `docker.sandbox_env`: named environment variables passed into the sandbox,
  empty by default. `sandbox_env = ["GH_TOKEN"]` lets agents use `gh` inside
  their container.

- Service data backups. Databases are dumped with their own tools
  (`pg_dumpall`, `mariadb-dump`) and other service volumes exported, daily and
  on demand at `POST /api/backups/services`. The `.ofbak` archive never covered
  container volumes, which is how the CRM's data was lost with no copy.

- Speaches, local speech-to-text and text-to-speech, as a service hand. Built
  from the audited FreecoDAO/speaches fork, with authentication on, its demo UI
  off and telemetry off, on port 8010.

- `company_create`, so an agent can create the organisation it belongs to. The
  store method and the HTTP route both existed and the tool did not, so an agent
  made CEO had no way to create a company -- and `project_create` takes a
  `company_id` that had to name an existing row, which meant any id an agent
  supplied failed on the foreign key with nothing it could do about it.

- Memory scopes. `memory_store` and `memory_recall` take an optional `scope`:
  `global` (the default and the previous behaviour), `company:<id>`,
  `project:<id>`, `team:<id>`, or `agent` for the caller's own. Shared memory was
  one flat namespace, so two teams both storing the obvious key overwrote each
  other and nothing could be kept to the group it concerned. `global` composes to
  the bare key, so everything already stored is still readable where it was and
  nothing needs migrating.

- Session tools, so an agent can name, fork and reset its own conversations.
  The operations existed in `freeco-memory` and stopped at the crate boundary,
  so an agent asked to tidy its threads could only explain that it had no way
  to.

- Organisation tools, so agents can see and build the structure they work in:
  `org_overview`, `team_create`, `team_add_member` and `project_create`. Note
  that `create_company` remains reachable only over HTTP -- there is no
  company-creating tool, so an agent cannot create the company it belongs to.

- A model picker in the assistant chat, listing only models whose provider is
  configured and which support tools, labelled with provider and price, free
  first. The list was already being fetched and never rendered, and the code
  that would have switched the model built its URL from an object, so it could
  never have worked.
- A new task opens its own named session. Everything used to append to one
  endless thread, so unrelated work shared a context and nothing could be found
  later. `POST /api/sessions/route` asks the model whether a message continues
  the conversation or starts a task, and fails open to continuing.
- Permission requests appear in the Chat page as well as the floating
  assistant, from one shared implementation so the two cannot drift apart.

- Service hands start their container. `freeco_hands::container` had no callers
  anywhere: activating OmniRoute recorded an instance, answered `status:
  Active`, and started nothing. Activation now starts the container, waits for
  its health check, and rolls the instance back if it never serves — on both
  the HTTP route and the `hand_activate` tool agents call. Deactivation stops
  it, reading the hand id before the record is deleted, since a container that
  outlives its instance holds the port and makes the next activation fail
  complaining about the port instead of the leftover.
- A host-network service hand could never pass its health check. The port was
  read only from published mappings, and a host-network container publishes
  none — the URL came out as `http://127.0.0.1:/v1/models`, so the first
  activation of the only service hand would have burned its whole 180-second
  budget and reported a timeout naming nothing. The port now falls back to
  `PORT` in the container env, and a health path with no derivable port fails
  at once saying which field to add. `/api/hands` reported `port: null` for the
  same reason.
- `hand_status` reports whether a service is actually serving, checked against
  the container runtime. The registry records what was asked for; only the
  container knows what runs, and they disagree exactly when it matters.
- Streaming requests fall back between models like non-streaming ones. The two
  paths had drifted — non-streaming retried on model-not-found, timeout,
  overload and context overflow, streaming only on model-not-found — and the
  dashboard streams, so the narrower path was the one users hit. Auth and
  billing failures still never fall back: spending on another provider to work
  around the user's own key would be worse than failing.
- The free-model picker no longer returns models that cannot serve agents.
  It filtered on price and on the `tools` parameter, and both families it
  needed to exclude pass those checks: OpenRouter's own routers advertise
  `tools` and then answer "No endpoints found that support tool use", and
  cloaked `stealth/*` models log prompts to an undisclosed provider — one of
  which had the widest free context on offer, so ranking picked it first.

- Model ids where the provider name is part of the id are no longer corrupted
  when saved. `nvidia/nemotron-3-ultra-550b-a55b` was stored as
  `nemotron-3-ultra-550b-a55b`, which NVIDIA does not serve: the request 404'd,
  the fallback chain fired, and the user was shown an error about a different
  model on a provider they never chose, while the dashboard still displayed the
  model they picked.
- Agent workspaces pointed at `~/.openfang` after the data had moved to
  `~/.freeco-ai`. Both paths resolving hid it; one agent had already dropped
  out of the registry, and archiving the old directory would have stopped the
  rest from starting.
- Agent config no longer asks for the name of an environment variable. It asks
  which provider's key to use, and accepts the key itself in the same screen.

### Added

- NVIDIA NIM as the default provider: Nemotron 3 Ultra 550B, free with no daily
  token cap, recorded at zero cost because that is what the provider charges.
- Working and secret-handling guidelines in every agent's system prompt,
  subagents included.
- Tool filters as a checkbox list with select-all.
- Model selection by intent -- best free, free only, or all -- with models that
  cannot call tools hidden, since they fail on the first turn.
- `scripts/refresh_model_catalog.py`, which checks the catalog against what
  providers actually serve. It found 15 delisted ids.

## [0.9.7] - 2026-08-15

### Fixed

- The release build now honours the same distribution-intent flag as the
  release preparation. v0.9.6 was tagged and then built nothing, because only
  half the gate had been made conditional.

## [0.9.6] - 2026-08-15

### Changed

- Releases are prepared only when a merged PR has the `release` label; the
  metadata commit and tag then trigger the release build automatically.
- FreEco.ai is now the primary identity for sandbox containers, workspace state,
  portable launchers, WhatsApp gateway integration, and environment variables.

### Fixed
- Release pipeline runs internally again: distribution prerequisites now apply
  only when COMMERCIAL_DISTRIBUTION is set, so builds and security patches are
  not blocked by a commercial-distribution review.
- Blocked bind mounts could be bypassed with a symlink. `validate_bind_mount`
  resolved the candidate path but compared it against the unresolved blocked
  path, so on any system where a blocked directory is reached through a symlink
  the mount was accepted. Both sides are now resolved before comparison.

- Reusable Docker sandboxes now preserve their maximum lifetime, avoid
  cross-workspace container-name collisions, safely retire incompatible
  containers, and reject concurrent execution in one agent workspace.
- Release preparation rejects empty changelog notes and preserves every roadmap
  section while moving Unreleased items into the tagged release.

## [0.9.5] - 2026-08-12

### Fixed
- Conversation history no longer becomes unreachable after a reinstall. History is
  stored per agent id and agents were getting random ids, so rebuilding the agent
  registry left every conversation in the database with nothing pointing at it:
  intact, invisible, and indistinguishable from deletion. Built-in and on-disk
  agents now derive stable ids from their names.
- Recovery for history already orphaned this way. `GET /api/sessions/orphaned`
  reports what is recoverable and the sessions page offers it as a button. It
  changes ownership only, so no message is rewritten and nothing is deleted.
- Auto-configure no longer starts a local model or downloads gigabytes of model
  files without being asked. Detection is read-only; only the explicit setup
  endpoint, reached by pressing a button that says so, may launch or download.
- OpenRouter model ids are sent in the form OpenRouter recognises, and
  `openrouter/free` is no longer rewritten to a pinned model lacking tool support.
- The FreEco.ai Assistant cannot be deleted. It is the entry point and holds the
  accumulated context, so deletion is refused at the API with a pointer to pause.

### Added
- Data gateway that inspects outbound prompts for API keys, card numbers, IBANs,
  emails, IP addresses and private keys, masks them with reversible tokens and
  restores them in replies. Card numbers are Luhn-checked and keys are
  prefix-anchored, so ordinary text is not mangled.
- Setup guidance for every shipped provider: where the key comes from and whether
  it is free, free-tier, paid or local. A test fails if a provider is added
  without it.
- Naming compatibility: `FREECO_*` variables are preferred, `FREECO_AI_*` still
  work, and an existing `~/.freeco-ai` install is never relocated.

- NVIDIA NIM as a free frontier option. Nemotron 3 Ultra 550B, Super 120B and
  Nano 30B on NVIDIA's own endpoint, where the free tier has no daily token
  cap. Cost is recorded as zero because that is what this provider charges;
  the same models resold through OpenRouter are priced, and copying those
  figures would bill users for spend they never incurred. Saving an NVIDIA key
  configures Ultra rather than whichever model sits first in the list.
- Claude Opus 5, Sonnet 5 and Fable 5, at their real 1M context. Carrying the
  4-series 200k figure over would have silently capped requests at a fifth of
  what the model accepts.

### Fixed

- Local models are no longer recommended to machines that cannot run them. The
  recommendation endpoint ignored the hardware gate autoconfigure applies, so a
  laptop with integrated graphics was offered a 7.2 GB download while
  autoconfigure refused the same machine. The catalog was the source: five of
  eight entries claimed to need no GPU at all.
- Local model setup no longer reports a server as ready while it is still
  loading. The readiness check accepted any HTTP reply, and llama.cpp answers
  503 for as long as the weights are loading, so the first message hit a server
  that had not finished starting.
- Agent config no longer asks for the provider twice. Choosing a model already
  sets it, so the provider row says so and the manual field is an override for
  custom endpoints rather than a second thing to fill in.
- Choosing a model from a provider with no API key now says so before saving,
  and links to where the key comes from, instead of failing later with a 401
  that nothing on the screen explains.

## [0.9.4] - 2026-08-08

### Fixed

- Agents no longer lose their conversations when FreEco.ai is reinstalled.
  History is stored keyed by agent id, and agents were assigned random ids,
  so rebuilding the agent registry -- which is what a reinstall does --
  brought them back under new ids and left every conversation in the
  database with nothing pointing at it. The data was intact and completely
  invisible, which is indistinguishable from deletion for anyone using the
  app. Built-in and on-disk agents now derive their ids from their names, so
  an agent comes back as itself. Existing agents are untouched.

- Conversations already orphaned this way can be recovered, since deriving
  ids does nothing for history that lost its owner before this change:
  `GET /api/sessions/orphaned` reports what is recoverable and the sessions
  page offers it as a button. It changes ownership only -- no message is
  rewritten and nothing is deleted.

- The in-app updater never ran. `withGlobalTauri` was unset, so
  `window.__TAURI__` did not exist in the dashboard at all, and every update
  fell through to "open the download page" even inside the desktop app. The
  dashboard also looked for the updater plugin's JavaScript bindings, which
  are not served with it, while the app's own `check_for_updates` and
  `install_update` commands went uncalled. Updates now install in place.

- Tagging a release no longer produces a tag with no build. A tag pushed by
  a workflow only starts another workflow when the token permits it; when it
  does not, the tag lands, nothing builds, and no job fails. v0.9.3 was
  tagged this way and never released. Auto Tag now starts the release build
  explicitly and fails loudly if it cannot.

- OpenRouter model ids are sent in the form OpenRouter accepts. The catalog
  prefixes them with `openrouter/` to tell them apart from the same model
  sold directly by its vendor, and that prefix was being sent upstream,
  where it is not a known model.

### Added

- The setup wizard says where an API key comes from and what it costs, for
  every provider it lists. It offered 42 providers and explained 13, because
  the signup links lived in a hand-written table in the dashboard that
  nobody updated when a provider was added. A test now fails if a provider
  ships without one.

- The model field is a list built from the live catalogue rather than a
  blank box that only worked if you already knew the exact id.

- The Assistant cannot be deleted, only paused. It holds the accumulated
  context the rest of the system reads from, and no confirmation dialog
  conveys the difference between removing an agent and discarding that.

## [0.9.3] - 2026-08-08

### Fixed

- Compaction no longer deletes messages. It called `split_off` on the
  message list and discarded older history permanently, keeping only a
  summary that cut each message to 200 characters and then clipped the
  result to its trailing 4000. Users who watched older chats vanish were
  seeing exactly that. The deletion was never necessary: the prompt builder
  already takes only the trailing window, so the model never saw those
  messages either way. Keeping the model's context small and throwing away
  the user's conversation are different things, and the second must not be
  a side effect of the first.
- `GET /api/sessions/{id}` exists. The history sidebar had been calling it
  and getting 405, so a finished feature sat starved of data.
- The Copy, Edit and Retry buttons do something. They were rendered and
  bound to `copyMessage`, `editMessage` and `retryFrom`, none of which had
  been written, so pressing them did nothing at all.
- Secrets Scan stopped failing at random. TruffleHog's Lob detector matched
  Rust test function names and reported them as *verified* secrets;
  verification calls the provider's API, whose answer is not deterministic,
  so the identical tree passed once and failed the four runs before it. A
  gate that blocks merges at random teaches people to wave it through.
- Agents no longer lose their conversations on reinstall. History is keyed by
  agent id, and agents were getting random UUIDs, so rebuilding the registry
  brought them back under new ids and orphaned every conversation in place —
  intact in the database, invisible in the product, indistinguishable from
  deletion. Ids are now derived from agent names. Existing agents are
  unaffected, being restored with their stored ids.
- OpenRouter model ids are sent in the form OpenRouter accepts. The catalog's
  `openrouter/` prefix is stripped only when a vendor/model pair remains, so
  `openrouter/free` and `openrouter/auto` — real models published under the
  `openrouter` vendor — are passed through unchanged.
- An exhausted daily allowance is reported as itself instead of a generic
  failure. A 429 was always treated as "too fast": retried, then returned
  bare. Providers use the same status for "no requests left today", which does
  not clear for hours, so the retries were wasted and the user saw no cause
  and no remedy.

### Added

- Recovery for conversations orphaned before the id fix: `GET
  /api/sessions/orphaned`, `POST /api/sessions/orphaned/adopt`, and a button
  on the sessions page. Changes ownership only — no message is rewritten and
  nothing is deleted, so it is safe to run twice.
- Signup page, one-line instruction, and cost class for all 42 providers,
  served from the backend beside the provider registry. A test fails if a
  provider is added without them. Previously 13 of 42 were explained, in a
  hand-written table in the dashboard JavaScript.
- The agent model field is a list built from the live catalogue, rather than a
  blank box usable only if you already knew the exact id.
- Assistant conversation controls: copy, retry, edit, search, and export.

### Changed

- `ROADMAP.md` re-verified against the running daemon rather than against the
  commits that were meant to implement each item. The unified inbox is
  recorded as Partial — tables exist, but `/api/inbox` and `/api/contacts`
  return 404 and there is no UI. OpenRouter's ~50-requests-per-day free cap is
  recorded as a known limit, since it shapes what "free by default" can mean.

## [0.9.2] - 2026-08-05

### Fixed

- **Chat history is no longer deleted.** Compaction had two deletion sites,
  not one. The first was fixed before 0.9.0; `store_llm_summary` and the
  kernel's compaction path both still replaced the message list with the
  post-compaction remnant and saved it, so an agent could go from 48 messages
  to 10 in a single pass. Neither was necessary: the prompt window was already
  bounded, so the model never saw the older messages either way.
- **Session lists were missing half their rows.** The two queries returned
  different field names -- `session_id`/`created_at` against
  `agent_id`/`updated_at` -- while the UI reads `id` and sorts on
  `updated_at`. Every row lacked one field or the other, so rows dropped out
  of the sort and the survivors could not be opened.
- **Tool results are no longer re-sent in full on every turn.** Measured at
  23.0M input tokens against 734k output across 114 calls, a 31:1 ratio and
  about EUR 25 spent almost entirely on re-transmitting payloads the model had
  already read. Historical results are now cut to 4000 characters, head and
  tail kept; the two most recent keep full fidelity. The complete output stays
  in the session and is readable in the interface.
- **Migrations v9 and v10 were being skipped.** They were gated on
  `< 8` while `SCHEMA_VERSION` stayed at 8, so any database already at 8
  skipped them permanently and silently. That is why `session_archive` did not
  exist, leaving the deletion above with no safety net.
- **`wasmtime` upgraded to 47.0.3**, clearing RUSTSEC-2026-0222. Note that
  47.0.0-47.0.2 do not clear it.
- Clippy `question_mark` lint that had been failing Linux CI.
- Approval timeout test asserted 60 seconds against a deliberate 4 hours.

### Added

- **Companies, projects and teams**, with conversations scoped to them, a
  Projects page, and archive and trash as distinct non-destructive states.
- **Unified inbox schema** (migration v12): `inbox_messages`, `contacts` and
  `contact_handles`, so the 44 channel integrations finally have somewhere to
  keep what they carry.
- **Free model discovery** via OpenRouter, fetched live rather than hardcoded.
- **Security policy** for secret handling and the rule against routing around
  security controls.
- **Approval requests must state aim, consequences and reversibility**, and
  boilerplate such as "n/a" is rejected.
- **Isolated sandbox image** with a real toolchain: rust, cargo, clippy, git,
  python3, jq, ripgrep.
- Brave browser preferred over Chrome, and found where it actually installs.
- Bootable Kubuntu from the FreEco USB without erasing it.

### Changed

- **Brand colours** applied throughout: `#68F000`, `#70E810`, `#18B810`.
- **Cloud voices blocked by default**, and the claim that Chrome transcribes
  on-device corrected -- the Web Speech API is server-side unless the page
  asks for local mode, which it now does.
- Credentials in URLs are refused rather than logged.


## [0.9.1] - 2026-08-02

### Added

- **Organisation data model.** Companies, projects and teams, with conversations scoped to them, archive and trash as distinct non-destructive states, and a Projects page.
- **Assistant capability gating.** The Assistant acts as a CEO across the system. An action is free if its damage is bounded and reversible (reading, creating, sandboxed work); gated if it is neither (destruction, standing grants, anything leaving the machine). `grant()` is one exhaustive match, so a new capability will not compile until someone decides which side of the line it falls on.
- **Conversation history with search**, auto-named sessions, copy/retry/edit, in-chat model picker, inline approvals, and a widget that remembers where you put it.

### Changed

- **Docker sandbox is on by default**, and no longer blocks ordinary shell syntax inside the container, which had made it unusable for real work.
- **Sandbox images are never pulled silently.**
- **Cloud text-to-speech voices are blocked by default.**
- **Local models are gated on real GPU capability**, with honest per-machine estimates instead of a config that looks fine and never answers.
- **Approval timeout raised to 24 hours** (ceiling) / 4 hours (default). The old 300-second ceiling denied any request raised while the user was away from the desk, so a long agent run failed on a permission that would have been granted an hour later.

### Fixed

- **Chat history is no longer deleted during compaction.** The trim bought nothing, since the prompt window was already bounded, and it destroyed the user's conversations.
- **Migrations v9 and v10 were being skipped** on any database already at v8, because SCHEMA_VERSION was never bumped. That is why `session_archive` did not exist and the deletion above had no safety net.
- **Credentials in URLs are now refused.** `web_fetch` rejects URLs carrying userinfo credentials (e.g. `https://token@api.github.com/...`), which wrote secrets into logs and audit records in clear text. The error names the header form to use instead.
- **Empty POST/PUT/PATCH bodies are now rejected** instead of sending an empty request that the server rejects with a confusing error.
- **Exec-policy refusal names the actual blocked command** instead of always saying "git and gh", so the caller knows what to allowlist or sandbox.
- **Brave browser is now detected and preferred** over Chrome and Edge. Brave installs per-user in LOCALAPPDATA on Windows, which was not checked, so an ordinary Brave install was reported as "no browser found." The search order now puts Brave and Chromium ahead of Chrome and Edge â same engine, no Google telemetry.

### Added
- **Voice dictation privacy fix.** The page was telling users that browser-based transcription was on-device, but the Web Speech API is server-side by default. The page now sets `processLocally` and checks `availableOnDevice()`, and warns plainly when the browser vendor's servers are used.

### Security

- **Assistant gating model** enforces that destruction, trust-changing capabilities (exec policy, user roles, installing tools or MCP hands), and anything leaving the machine require approval. `PurgeSession` and trust-changing capabilities rate Critical severity.
- **Credential leakage prevention** in `web_fetch` â tokens embedded in URLs are blocked before the request is made.

## [0.8.2] - 2026-07-26

### Added

- **Local AI without Ollama.** New llama.cpp runtime: FreEco downloads a **Gemma 4** GGUF straight from HuggingFace (which honours HTTP Range, so downloads genuinely resume) and serves it through `llama-server` as an OpenAI-compatible API on `127.0.0.1:8080`. `GET /api/local-ai/llama/catalog` lists the Gemma 4 variants with hardware fit; `POST /api/local-ai/llama/setup` downloads, starts and wires it as the default model. Ollama is no longer used or required: its blob CDN ignores Range, so an interrupted pull restarted from zero and never completed on a slow link.
- **Freeco Assistant window controls.** Drag the header to move it, drag the corner to resize, toggle full screen. A speech bar gives **pause / resume / stop** while Freeco is speaking. A **+ menu** attaches a file, a whole folder, or a photo from the camera; attachments ride along with the next message as removable chips.
- **Backups you can actually restore.** `GET /api/backups` lists archives, and the Runtime page gains a Backups card with **Verify** (dry-run, non-destructive) and **Restore**. Backup-with-no-restore was a trap.
- **Integrations tab** (Settings) for the extension subsystem: list installed and available, install, remove, reconnect.
- **Devices tab** (Settings) for mobile pairing: generate a pairing code, list paired devices, unpair.
- **USB preparation script** (`scripts/prepare-freeco-usb.ps1`) that partitions a stick into ESP + exFAT + free space for Kubuntu, so one Gemma 4 file serves both Windows and a booted Linux. exFAT because FAT32 caps a file at 4 GB.
- **Single-flight model fetcher** (`scripts/fetch-model.sh`): pid-locked so two downloaders can never fight over one file, true byte-resume, and a final size check.

### Fixed

- **dograh voice connector pointed at a dead endpoint.** dograh mounts MCP at `/api/v1/mcp`, not `/mcp`, so auto-connect registered something that looked connected but was not.
- **CRM and voice could not coexist.** dograh publishes host port 3000 and Twenty CRM bound the same port, so whichever installed second failed. CRM moved to 3200.
- **Repo-based services started without config.** Services cloned from git now get a real `.env` generated from `.env.example`, with change-me secrets replaced and an existing file never overwritten.

## [0.8.1] - 2026-07-24

### Added

- **One-click services (Settings Ã¢ÂÂ Services)**: Freeco installs and runs the services a company needs instead of assuming you already have them Ã¢ÂÂ **dograh** (real-time voice AI, MCP-native so it auto-connects to your agents), **Twenty** (CRM), and **Akaunting** (accounting). Each install verifies Docker (with a plain-language fix if it's missing), pulls the service, waits for it to come up, and Ã¢ÂÂ for MCP-native services Ã¢ÂÂ registers it so agents can use it immediately. Service cards show live install progress, download size up front, and running/connected badges. Services are downloaded on demand, not bundled in the installer.

## [0.8.0] - 2026-07-22

> Supersedes 0.7.9, whose release artifacts never published because of the
> release-pipeline failures fixed below.

### Added

- **Multiuser accounts with roles (RBAC)**: create accounts in **Settings Ã¢ÂÂ People** with a role Ã¢ÂÂ `owner`, `admin`, `user`, `kid`, or `viewer`. A new **Kid** role sits between viewer and user: a child account can chat with an agent but cannot change settings, spawn agents, or see billing. Dashboard sign-in is now role-aware (the legacy admin account is always owner; unknown or disabled accounts fall back to the least-privileged viewer). Accounts are stored in `config.toml` and apply on restart.
- **One-click local-first model autoconfig**: *Settings Ã¢ÂÂ Providers Ã¢ÂÂ Auto-configure models* points everyday work at a free, private **local Gemma** sized to your machine, and automatically registers the strongest cloud model whose key you've configured (Anthropic Ã¢ÂÂ OpenAI Ã¢ÂÂ Gemini Ã¢ÂÂ Novita Ã¢ÂÂ Groq Ã¢ÂÂ OpenRouter) as a **complex-task tier**. With no cloud key, everything simply stays local.
- **Company templates in the visual Workflow Builder**: one click lays out a whole organization as a wired sequence of role steps Ã¢ÂÂ **Lean startup**, **Nonprofit / charity**, or **Content agency** Ã¢ÂÂ each role pre-filled with a starter prompt. Assign an agent per role, then save/run.

### Fixed

- **"Skip" on the first-run password prompt now sticks.** The desktop app binds a random port each launch, and browser `localStorage` is keyed by origin (host **+ port**), so the skip flag was wiped on every restart and the prompt kept reappearing. The dismissal is now persisted server-side (`POST /api/auth/dismiss-setup`), so skipping is permanent.
- **Voice communication now works Ã¢ÂÂ and says why when it doesn't.** Speech-to-text accepts **any OpenAI-compatible transcription endpoint** via `[media].audio_base_url` (a local Whisper server, or a compatible provider), and no longer demands a Groq/OpenAI key when a custom endpoint is set. Previously a failed transcription was logged server-side and silently dropped, so holding the mic just produced an unreadable `[Voice message]`; the reason is now returned to the client and shown in both the chat and the Freeco Assistant, telling you exactly what to configure.
- **Logo rendered dark in the light/day theme** Ã¢ÂÂ a `filter: invert(1)` was flipping the green-on-white mark. The original green-on-white logo now shows in both day and night themes, and is slightly larger.

## [0.7.8] - 2026-07-21

### Added

- **Freeco Assistant Ã¢ÂÂ a global AI concierge on every page**: a floating assistant (bottom-right, present on every dashboard screen) that chats, listens (hold-to-talk voice), and helps set things up. Quick-setup shortcuts route to the right builder (company/nonprofit, agent team, workflow, tools/MCP, channels, local AI) and hand the concierge a guiding prompt; it talks to a concierge agent over the normal message API. Ships with `docs/freeco-concierge-guide.md` Ã¢ÂÂ a user guide for building a self-running company/nonprofit (org chart, tools & MCP setup, open-source connectors, per-function recipes).

### Fixed

- **Local-AI (Ollama) setup now works on any connection**: the installer download is resilient to flaky/slow links. It resumes with an HTTP Range request after a dropped connection (instead of restarting), retries up to 8 times with backoff, and verifies the downloaded file matches the server's Content-Length before use. This fixes the confusing `installer failed digital-signature verification ()` error, which was caused by a truncated download producing an unreadable signature Ã¢ÂÂ the check now also reports the real reason (empty/missing file, invalid status, or an actual signer mismatch) instead of a blank `()`.
- **Desktop app branding**: the window title bar, system-tray tooltip/menu ("Quit FreEco.ai"), and update notifications now read **FreEco.ai** instead of the internal "Freeco" name.
- **"Get update" button did nothing**: it was a bare link that silently failed in the desktop webview and gave no feedback. It now runs the desktop auto-updater (download + install + relaunch) when available, or opens the download page in the browser edition Ã¢ÂÂ with a toast/status at every step (checking, downloading, installing, error), never silent.
- **Update check now distinguishes "offline"** from a real failure, showing a calm "you're offline" message instead of a scary network error.
- **Release verify-release** relaxed from exact tag==main-HEAD to `git merge-base --is-ancestor`, so a tag that is a few commits behind main (e.g. after an auto-tag) still releases. The version/CHANGELOG/tauri-version consistency checks are retained (they catch mislabeled installers).

### Docs / tooling

- **Windows GNU-toolchain build guide** (`docs/building-on-windows.md`) + `scripts/dev-build.ps1` / `dev-build.sh` that auto-select a working toolchain (MSVC when present, else GNU with MinGW on PATH), so contributors without the MSVC Build Tools don't lose time to cryptic linker errors.


## [0.7.7] - 2026-07-14

### Added

- **Model tuning controls** so operators can adjust provider/model behavior for the new FreEco.ai workflows shipped on `main`.
- **Privacy-aware routing** for cloud versus local model usage, keeping the release aligned with the new privacy-first provider flow.
- **Security scanning coverage** for bundled skills and release surfaces that shipped with the v0.7.7 feature set.

### Fixed

- **Encrypted recovery handling** so recovery and persistence flows keep their protected payloads intact instead of downgrading them during repair paths.

## [0.7.6] - 2026-07-14

### Fixed

- **"Update check failed: Failed to fetch"**: the dashboard page's Content-Security-Policy (`connect-src`) did not include `https://api.github.com`, so the Settings Ã¢ÂÂ Software Updates check was blocked by CSP. Added the GitHub API host to the dashboard CSP (it was already allowed in the Tauri shell CSP, but the page-level CSP is the one that governs the fetch).

## [0.7.5] - 2026-07-11

### Added (Tier-0 safety, from the threat model)

- **Cloud data-leak warning**: connecting any online/cloud AI provider now shows a clear privacy confirmation before the key is saved Ã¢ÂÂ it explains that agents' messages, files, and company data will be sent to that provider, and points to the local "Free local AI" option that never leaves the device. Local providers save silently (no warning needed). Uses the existing `is_local` provider flag.
- **Plain-language approval consequences**: every pending approval now shows, in plain words a non-technical user understands, what approving will let the agent do Ã¢ÂÂ with severity color: Ã¢ÂÂ run a command on your computer / delete data / spend money (red), Ã¢ÂÂ Ã¯Â¸Â send something out / change a file / use the internet (amber), Ã¢ÂÂ¹Ã¯Â¸Â review (neutral). Derived client-side from the action; no backend change.

### Verified

- Shell execution is gated (metacharacter block + exec-policy allowlist in `tool_runner.rs`); an agent with an empty shell policy cannot run shell commands. Documented in the threat model.


## [0.7.4] - 2026-07-10

### Fixed

- Local AI model downloads now auto-resume after network drops (up to 6 total attempts with backoff; Ollama keeps finished layers) instead of erroring out and appearing to restart from zero.
- README: dummy-proof Windows install guide with a direct `-setup.exe` link, SmartScreen note, and desktop-shortcut mention Ã¢ÂÂ users were downloading the bare CLI `.zip` and finding "nothing installs". Broken `freeco.ai/install` one-liners replaced with working raw-GitHub URLs; stale v0.5.10 banner updated.
- Release CI: macOS desktop no longer fails by attempting notarization without an Apple Team ID after ad-hoc signing Ã¢ÂÂ the unsigned `.dmg` now builds.


### Added

- **FreEco.ai Editions in the dashboard** Ã¢ÂÂ the Overview page now offers one-click setup cards for the four editions: Personal Concierge, Kids (strictly guardrailed: no web, no shell, no agent messaging, parental visibility), Business Suite (Secretary + autonomous buyer team), and AI Company (CEO + Secretary + Shopping team). Five new bundled agent templates: `freeco-concierge`, `freeco-kids`, `freeco-ceo`, `freeco-secretary`, `freeco-shopping`.
- **Native freeco agents wired into the kernel** Ã¢ÂÂ the `freeco-agent-*` crates (previously compiled but unreachable) now execute via new `freeco:*` module ids (`freeco:ceo`, `freeco:secretary`, `freeco:shopping`): kernel dispatch in `execute_agent`, message conversion via `freeco-agent-core`'s bridge, deterministic CEO delegation planning with zero LLM cost, Secretary intent routing (LLM-classified when a key exists, keyword heuristic otherwise), Shopping three-tier research through the permission-gated tool gateway.
- **Self-development Dev Pod (Phase 6, stage 1)** Ã¢ÂÂ new `freeco-developer` and `freeco-tester` bundled agents: the developer implements labeled GitHub issues in a sandboxed repo clone and opens draft PRs; the tester/ethical-hacker verifies with build/test/clippy/`cargo audit` plus a security review of the diff and posts a PASS/FAIL verdict. Narrow manifests (shell limited to git/cargo/gh, network to github.com/crates.io, tester has no write access); humans merge. Dashboard gains a Dev Pod setup card; guide in `docs/self-development.md`; Phase 6 (Self-Running AI Company) added to the README roadmap.
- **One-click free local AI** Ã¢ÂÂ `POST /api/local-ai/setup` + `GET /api/local-ai/status`: detects Ollama, downloads/installs it silently per-user on Windows (official installer only, with progress), streams the model pull (default: lightweight Gemma), and points `[default_model]` at the local runtime. Settings Ã¢ÂÂ Providers gains a "Free local AI Ã¢ÂÂ no account needed" card with progress bar; macOS/Linux get a guided one-step path.

### CI

- macOS desktop bundles with ad-hoc signing when no Apple certificate is configured (previous fix skipped the import but left an empty signing identity, failing the bundle step).
- Tauri updater artifacts (`latest.json` + signatures) are now generated (`createUpdaterArtifacts: true`) Ã¢ÂÂ v0.7.3 is the first release existing installs can auto-update from.
- GHCR package-visibility step uses the user endpoint (FreecoDAO is not an org) and no longer fails the release.

### Earlier in this cycle

- Dashboard software updates: Settings Ã¢ÂÂ System now shows the installed vs latest version with a "Check for updates" button, a once-a-day automatic check (on by default, toggleable, throttled via localStorage), and a green "update available" pill in the sidebar linking to the download. Desktop-app CSP updated to allow the GitHub releases API.
- Dashboard sidebar now shows the fre.eco logo (replaced the old logo asset served at `/logo.png`).
- Portable / USB edition: OS-detecting launcher scripts (`scripts/portable/`) that run FreEco.ai from any folder or USB drive with all data kept alongside the binary via `FREECO_AI_HOME`, plus `scripts/build-portable.sh` to assemble the bundle from release binaries, and `docs/usb-portable.md`.
- Landing page (`docs/index.html`) Ã¢ÂÂ self-contained green-brand site with edition overview, download links, terminal install commands, and PayPal donation QR; deployable via GitHub Pages or any static host.
- README roadmap: "Planned: Self-Contained Isolation Layer" (bundled agent browser + prebuilt sandbox image, Manus-style all-included isolation).
- First unit tests for `freeco-learning` (`PromotionPolicy::target_file` mapping).

### Changed

- Updated user-facing dashboard branding to display `FreEco.ai` in the sidebar header.
- Updated documentation to explicitly state that FreEco.ai is built on the Freeco base with ethical and structural enhancements.

### Fixed

- Branding: browser-tab favicon and the entire desktop/installer icon set (shortcut, taskbar, Add/Remove Programs) still showed the old Freeco cobra Ã¢ÂÂ all regenerated from the fre.eco logo.
- Voice-message transcription: `media_transcribe` resolved paths against the daemon's working directory instead of the agent workspace, and dashboard audio uploads were stored only under a UUID in a temp folder Ã¢ÂÂ so agents were told a filename that existed nowhere they could reach ("Failed to read audio file"). The tool now resolves workspace-relative paths first and falls back to the uploads folder by filename, and audio uploads keep a sanitized filename-addressable copy.
- README typos ("buisiness" Ã¢ÂÂ "business", "an SUSTAINABLE" Ã¢ÂÂ "a SUSTAINABLE").
- Dashboard light theme was a duplicate of the dark palette Ã¢ÂÂ it is now a real white day theme (the Light/System/Dark switcher finally has visible effect). Previously-undefined modal variables (`--bg-card`, `--bg-input`, `--bg-secondary`) no longer fall back to dark values.
- Desktop auto-updater pointed at the upstream repository (RightNow-AI/freeco) with the upstream signing key; it now points at FreecoDAO/freeco-ai signed with the project's own key.
- Release CI no longer fails the macOS desktop build when Apple signing certificates are absent Ã¢ÂÂ it produces an unsigned .dmg instead.
- Installer branding: product renamed Freeco Ã¢ÂÂ FreEco.ai (shortcuts, Add/Remove Programs, Start Menu folder), installer version no longer stuck at 0.6.9, Windows installs per-user without admin prompts.

## [0.5.10] - 2026-04-17

### Fixed

- Non-loopback requests with no `api_key` configured now return 401 by default. Opt out with `FREECO_AI_ALLOW_NO_AUTH=1`. Fixes the B1/B2 authentication bypass from #1034.
- Agent `context.md` is re-read on every turn so external updates take effect mid-session. Opt out per agent with `cache_context = true` on the manifest. Fixes #843.
- `freeco-ai config get default_model.base_url` now prints the configured URL instead of an empty string. Missing keys return a clear "not found" error. Fixes #905.
- `schedule_create`, `schedule_list`, and `schedule_delete` tools plus the `/api/schedules` routes now use the kernel cron scheduler, so scheduled jobs actually fire. One-shot idempotent migration imports legacy shared-memory entries at startup. Fixes #1069.
- Multimodal user messages now combine text and image blocks into a single message so the LLM sees both. Fixes #1043.

### Added

- `freeco-ai hand config <id>` subcommand: get, set, unset, and list settings on an active hand instance. Fixes #809.
- Optional per-channel `prefix_agent_name` setting (`off` / `bracket` / `bold_bracket`). Wraps outbound agent responses so users in multi-agent channels can see which agent replied. Default is off, byte-identical to prior behavior. Fixes #980.

### Closed as invalid

- #818 and #819. Both reference a knowledge-domain API that does not exist on `main`. Filed against an unmerged feature branch (`plan/013-audit-remediation`). Close with a note to build the proposed validation and stale-timestamp surfacing into that feature when it lands.

## [0.5.9] - 2026-04-10

### Changed

- **BREAKING:** Dashboard password hashing switched from SHA256 to Argon2id. Existing `password_hash` values in `config.toml` must be regenerated with `freeco auth hash-password`. Only affects users with `[auth] enabled = true`.

### Fixed

- Dashboard passwords were hashed with plain SHA256 (no salt), making them vulnerable to rainbow table and GPU-accelerated brute force attacks. Now uses Argon2id with random salts.

## [0.1.0] - 2026-02-24

### Added

#### Core Platform
- 15-crate Rust workspace: types, memory, runtime, kernel, api, channels, wire, cli, migrate, skills, hands, extensions, desktop, xtask
- Agent lifecycle management: spawn, list, kill, clone, mode switching (Full/Assist/Observe)
- SQLite-backed memory substrate with structured KV, semantic recall, vector embeddings
- 41 built-in tools (filesystem, web, shell, browser, scheduling, collaboration, image analysis, inter-agent, TTS, media)
- WASM sandbox with dual metering (fuel + epoch interruption with watchdog thread)
- Workflow engine with pipelines, fan-out parallelism, conditional steps, loops, and variable expansion
- Visual workflow builder with drag-and-drop node graph, 7 node types, and TOML export
- Trigger system with event pattern matching, content filters, and fire limits
- Event bus with publish/subscribe and correlation IDs
- 7 Hands packages for autonomous agent actions

#### LLM Support
- 3 native LLM drivers: Anthropic, Google Gemini, OpenAI-compatible
- 27 providers: Anthropic, Gemini, OpenAI, Groq, OpenRouter, DeepSeek, Together, Mistral, Fireworks, Cohere, Perplexity, xAI, AI21, Cerebras, SambaNova, Hugging Face, Replicate, Ollama, vLLM, LM Studio, and more
- Model catalog with 130+ built-in models, 23 aliases, tier classification
- Intelligent model routing with task complexity scoring
- Fallback driver for automatic failover between providers
- Cost estimation and metering engine with per-model pricing
- Streaming support (SSE) across all drivers

#### Token Management & Context
- Token-aware session compaction (chars/4 heuristic, triggers at 70% context capacity)
- In-loop emergency trimming at 70%/90% thresholds with summary injection
- Tool profile filtering (cuts default 41 tools to 4-10 for chat agents, saving 15-20K tokens)
- Context budget allocation for system prompt, tools, history, and response
- MAX_TOOL_RESULT_CHARS reduced from 50K to 15K to prevent tool result bloat
- Default token quota raised from 100K to 1M per hour

#### Security
- Capability-based access control with privilege escalation prevention
- Path traversal protection in all file tools
- SSRF protection blocking private IPs and cloud metadata endpoints
- Ed25519 signed agent manifests
- Merkle hash chain audit trail with tamper detection
- Information flow taint tracking
- HMAC-SHA256 mutual authentication for peer wire protocol
- API key authentication with Bearer token
- GCRA rate limiter with cost-aware token buckets
- Security headers middleware (CSP, X-Frame-Options, HSTS)
- Secret zeroization on all API key fields
- Subprocess environment isolation
- Health endpoint redaction (public minimal, auth full)
- Loop guard with SHA256-based detection and circuit breaker thresholds
- Session repair (validates and fixes orphaned tool results, empty messages)

#### Channels
- 40 channel adapters: Telegram, Discord, Slack, WhatsApp, Signal, Matrix, Email, Teams, Mattermost, Google Chat, Webex, Feishu/Lark, LINE, Viber, Facebook Messenger, Mastodon, Bluesky, Reddit, LinkedIn, Twitch, IRC, XMPP, and 18 more
- Unified bridge with agent routing, command handling, message splitting
- Per-channel user filtering and RBAC enforcement
- Graceful shutdown, exponential backoff, secret zeroization on all adapters

#### API
- 100+ REST/WS/SSE API endpoints (axum 0.8)
- WebSocket real-time streaming with per-agent connections
- OpenAI-compatible `/v1/chat/completions` API (streaming SSE + non-streaming)
- OpenAI-compatible `/v1/models` endpoint
- WebChat embedded UI with Alpine.js
- Google A2A protocol support (agent card, task send/get/cancel)
- Prometheus text-format `/api/metrics` endpoint for monitoring
- Multi-session management: list, create, switch, label sessions per agent
- Usage analytics: summary, by-model, daily breakdown
- Config hot-reload via polling (30-second interval, no restart required)

#### Web UI
- Chat message search with Ctrl+F, real-time filtering, text highlighting
- Voice input with hold-to-record mic button (WebM/Opus codec)
- TTS audio playback inline in tool cards
- Browser screenshot rendering in chat (inline images)
- Canvas rendering with iframe sandbox and CSP support
- Session switcher dropdown in chat header
- 6-step first-run setup wizard with provider API key help (12 providers)
- Skill marketplace with 4 tabs (Installed, ClawHub, MCP Servers, Quick Start)
- Copy-to-clipboard on messages, message timestamps
- Visual workflow builder with drag-and-drop canvas

#### Client SDKs
- JavaScript SDK (`@freeco/sdk`): full REST API client with streaming, TypeScript declarations
- Python client SDK (`freeco_client`): zero-dependency stdlib client with SSE streaming
- Python agent SDK (`freeco_sdk`): decorator-based framework for writing Python agents
- Usage examples for both languages (basic + streaming)

#### CLI
- 14+ subcommands: init, start, agent, workflow, trigger, migrate, skill, channel, config, chat, status, doctor, dashboard, mcp
- Daemon auto-detection via PID file
- Shell completion generation (bash, zsh, fish, PowerShell)
- MCP server mode for IDE integration

#### Skills Ecosystem
- 60 bundled skills across 14 categories
- Skill registry with TOML manifests
- 4 runtimes: Python, Node.js, WASM, PromptOnly
- FangHub marketplace with search/install
- ClawHub client for OpenClaw skill compatibility
- SKILL.md parser with auto-conversion
- SHA256 checksum verification
- Prompt injection scanning on skill content

#### Desktop App
- Tauri 2.0 native desktop app
- System tray with status and quick actions
- Single-instance enforcement
- Hide-to-tray on close
- Updated CSP for media, frame, and blob sources

#### Session Management
- LLM-based session compaction with token-aware triggers
- Multi-session per agent with named labels
- Session switching via API and UI
- Cross-channel canonical sessions
- Extended chat commands: `/new`, `/compact`, `/model`, `/stop`, `/usage`, `/think`

#### Image Support
- `ContentBlock::Image` with base64 inline data
- Media type validation (png, jpeg, gif, webp only)
- 5MB size limit enforcement
- Mapped to all 3 native LLM drivers

#### Usage Tracking
- Per-response cost estimation with model-aware pricing
- Usage footer in WebSocket responses and WebChat UI
- Usage events persisted to SQLite
- Quota enforcement with hourly windows

#### Interoperability
- OpenClaw migration engine (YAML/JSON5 to TOML)
- MCP client (JSON-RPC 2.0 over stdio/SSE, tool namespacing)
- MCP server (exposes Freeco tools via MCP protocol)
- A2A protocol client and server
- Tool name compatibility mappings (21 OpenClaw tool names)

#### Infrastructure
- Multi-stage Dockerfile (debian:bookworm-slim runtime)
- docker-compose.yml with volume persistence
- GitHub Actions CI (check, test, clippy, format)
- GitHub Actions release (multi-platform, GHCR push, SHA256 checksums)
- Cross-platform install script (curl/irm one-liner)
- systemd service file for Linux deployment

#### Multi-User
- RBAC with Owner/Admin/User/Viewer roles
- Channel identity resolution
- Per-user authorization checks
- Device pairing and approval system

#### Production Readiness
- 1731+ tests across 15 crates, 0 failures
- Cross-platform support (Linux, macOS, Windows)
- Graceful shutdown with signal handling (SIGINT/SIGTERM on Unix, Ctrl+C on Windows)
- Daemon PID file with stale process detection
- Release profile with LTO, single codegen unit, symbol stripping
- Prometheus metrics for monitoring
- Config hot-reload without restart

[0.1.0]: https://github.com/FreecoDAO/FreEco-ai/releases/tag/v0.1.0
