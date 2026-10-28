# FreEco.ai feature register

_State of the build on the owner's machine, **2026-09-30**, updated **2026-10-02** (branch
`fix/approvals-explain-themselves-and-chat-routing`, to be released as v0.9.8)._

This is the one list of what FreEco.ai does, what state each part is in, and
how to check it. It exists so anyone — the owner, the CEO agent, a developer —
can verify a claim instead of trusting it. `CHANGELOG.md` has the detail of
each change; `ROADMAP.md` has plans.

## Status legend

| Mark | Meaning |
|---|---|
| ✅ | **Verified live** — checked against the running system on the date given. |
| 🟨 | **Built, not yet verified live** — compiles, tests pass, nobody has watched it work. |
| ⚠️ | **Partial or known broken** — some of it exists; do not describe it as done. |
| 📋 | **Planned** — not built. |
| ❔ | **Claimed, never checked here** — stated in the README; no check recorded. |

**Who can check it:**
- **CEO** — the CEO agent, with its own tools (`sandbox_exec`, `crm_api`,
  `hand_status`, file tools, memory, schedules), from inside the OS.
- **Owner** — needs the dashboard, a browser or a microphone. Agents cannot
  reach the dashboard API; that is deliberate.

Rule for every check: **nothing is run on the host.** No host `shell_exec`, no
installing software. If a check cannot be done from the sandbox or with a
FreEco tool, write "cannot check from inside" and move on.

---

## 1. Chat and the popup (the CEO's front door)

| Feature | Status | Who | How to check |
|---|---|---|---|
| The CEO assistant is first in every agent list | ✅ 2026-09-29 | Owner | Open Chat and Agents: the CEO is at the top. |
| Popup routes a message: this chat, a new task, an existing thread, a new project or company — asks first | 🟨 | Owner | Write a request in the popup; a card offers where to send it; choosing one delivers it there. |
| Popup keeps unanswered messages across a model change or page reload | 🟨 | Owner | Send while the model is slow, reload: the message is still there, marked pending. |
| Pressing Enter twice sends once | 🟨 | Owner | Press Enter twice quickly: one message. |
| Action chips (Ask, Translate, Shell) can be copied, dragged or opened as a link | 🟨 | Owner | Drag a chip into another app; copy it; open the link. |
| No auto-scroll while reading; "↓ New messages" button | 🟨 | Owner | Scroll up while an agent writes: the view stays put and the button appears. |
| Tool and technical steps collapsed by default; only the running step open | 🟨 | Owner | Run a task with tools: finished steps fold. |
| Conversation menu: open in new window, rename, fork, archive, delete, restore from archive | 🟨 (rename ✅ 2026-09-29) | Owner | ⋯ menu on a chat. Archive then restore must bring the messages back. |
| Output styles: default, concise, explanatory, learning, proactive | ✅ 2026-09-29 | Owner | ⋯ → Output style; an invalid style is refused (HTTP 400). |
| Side panel: files, changes, artifacts, terminal, browser, background, archive | 🟨 (terminal ✅, file escape blocked ✅) | Owner | Panel button in the chat. Terminal runs in the sandbox, never on the host. |
| History pages instead of stopping at 50 messages | ✅ 2026-09-23 | Owner | "Load earlier" on a long chat. |
| Attachments of any supported type reach the agent (docx, pdf, xlsx, images) | ✅ 2026-09-23 | CEO | Owner uploads a .docx; the agent reads it in its sandbox with `docx2txt`. |

## 2. Security and privacy

| Feature | Status | Who | How to check |
|---|---|---|---|
| **Secrets masked before any model call — chat and popup included** (new) | 🟨 | Owner | Send "test key: sk-proj-0123456789abcdef0123". The daemon log shows "Data gateway masked values … (streaming)". Until now only background calls were masked. |
| Gateway recognises JWT keys and values announced as "key: …", `NAME_KEY=…`, "password is …" (new) | ✅ 2026-09-30 (scan endpoint) | Owner | Ordinary sentences ("the key is to plan first") are not flagged — verified. |
| **Chat and popup stop a message holding a key** and offer "Save in vault and send" (new) | 🟨 | Owner | Paste a key into a message: a dialog appears; the message goes out with `[NAME saved in the vault]`. |
| Keys stored in the vault and `secrets.env`, never echoed back | 🟨 | Owner | — |
| CSRF protection on every state-changing request | ✅ 2026-09-29 | Owner | A cross-site form POST is refused. |
| Dashboard fonts served locally; no request to Google | ✅ 2026-09-29 | Owner | Browser network tab shows no Google request. |
| File tools cannot leave the agent's workspace | ✅ 2026-09-30 (CEO) | CEO | `file_read` on a path outside your workspace is refused. |
| Host `shell_exec` refused, pointing to `sandbox_exec` | 🟨 | CEO | Do not run it; read the tool's refusal text in a past approval instead. |
| Local speech service never receives the OpenAI key | 🟨 | Owner | — |
| Approval requests say what, why and at what cost; sound and desktop alert | 🟨 | Owner | Trigger an approval with the dashboard in the background. |
| `web_fetch` refuses local and private addresses (SSRF) | ✅ 2026-09-30 (CEO) | CEO | `web_fetch http://127.0.0.1:3200` is refused. This is intended. |

## 3. Sandbox and the local services

| Feature | Status | Who | How to check |
|---|---|---|---|
| **Agents reach the installed services from their sandbox** (new) | ✅ 2026-09-30 — CEO's sandbox: speech and accounting answered on its first run; CRM, voice API and voice UI answered on a re-check from the same sandbox minutes later (they were restarting during the first) | **CEO** | `sandbox_exec` with `tier: "workspace"`: `curl -s http://127.0.0.1:8010/health` → `{"message":"OK"}`; `curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8000/api/v1/health` → 200; same for `:3200/healthz` (CRM), `:3010` (voice UI), `:8080` (accounting). |
| Only the service ports are opened, not databases | ✅ 2026-09-30 — CEO: 5432, 3306, 6379 closed | CEO | From the sandbox, `curl -s -m3 telnet://127.0.0.1:5432` fails. |
| Sandbox tiers: `exec` (no network, 60 s) and `workspace` (network, 4 GB, 30 min, persistent) | ✅ | CEO | Run `hostname` in each tier. |
| Sandbox image t3 reads documents: `pdftotext`, `tesseract` (en/fr/de/ru), `docx2txt`, `pypdf`, `python-docx`, `openpyxl` | ✅ 2026-09-23 | CEO | `sandbox_exec`: `tesseract --list-langs`. |
| Sandbox survives a daemon restart; `npm install` works in it | 🟨 | CEO | Create a file in the workspace, check it after the next restart. |
| Container watchdog no longer takes down a healthy VM | ✅ 2026-09-29 | — | VM stayed up with the new daemon. |
| VM recovers after the machine sleeps | ⚠️ code in place, real sleep/wake not reproduced | Owner | After waking the PC, services answer within minutes without help. |
| Service containers come back after a VM restart | ✅ 2026-09-23 (12 containers in 50 s) | CEO | `hand_status` for each service shows `serving`. |
| Images downloaded only when missing — no re-download on restart | ✅ 2026-09-30 (code read) | — | — |

## 4. CRM (Twenty) — tasks and schedules

| Feature | Status | Who | How to check |
|---|---|---|---|
| Twenty CRM running at port 3200 | ✅ 2026-09-30 | CEO | From the sandbox: `curl -s http://127.0.0.1:3200/healthz`. |
| **`crm_api` tool**: tasks (with due dates = the schedule), people, companies, notes, opportunities (new) | ✅ 2026-09-30 not-connected path (CEO); 🟨 real reads and writes wait for the new key | **CEO** | `crm_api GET /rest/tasks?limit=5`. Until the owner connects a key it answers "The CRM is not connected … Services → Twenty CRM → Connect" — that is the correct answer today. After connecting: list tasks, create one (`POST /rest/tasks {"title":"CRM check","status":"TODO"}`), mark it DONE (`PATCH`). |
| **Connect field** on Services → Twenty CRM, key checked before it is stored (new) | ✅ 2026-09-30 (bad key refused) | Owner | Paste a new key made in the CRM (Settings → APIs & Webhooks). |
| crm and voice agents have `sandbox_exec` (and crm has `crm_api`) (new) | ✅ 2026-09-30 (CEO) | CEO | `hand_status` / agent list shows the tools. |
| The key given in chat on 2026-09-29 | ⚠️ **revoked** — the CRM answers "This API Key is revoked" | Owner | A new key is needed. The revoked one is still stored in agent memory; removal waits for the owner. |
| Unified inbox and contacts inside FreEco (not Twenty) | ⚠️ tables only, no API or UI | — | — |

## 5. Voice

| Feature | Status | Who | How to check |
|---|---|---|---|
| Speaches (local speech-to-text and text-to-speech) in its own container, from the FreecoDAO fork, CPU-only 0.9 GB image | ✅ running 2026-09-30 | CEO | `curl http://127.0.0.1:8010/health` from the sandbox. |
| Dictation uses local Speaches, not a cloud service | 🟨 end-to-end not tested | Owner | Dictate a sentence in English and Ukrainian. |
| Dictation languages: browser default, en-US, en-GB, uk-UA, fr-FR, de-DE, it-IT | 🟨 | Owner | Settings → Dictation. |
| Brave: falls back to local recording when browser speech recognition is unavailable | 🟨 | Owner | Dictate in Brave. |
| Spoken replies in the main chat | 🟨 | Owner | Voice-out button. |
| Dograh voice calls | ⛔ removed 2026-10-01 at the owner's decision; containers deleted, data volumes kept | — | — |
| Dictation through FreEco to local Speaches (`[media] audio_model`) | ✅ 2026-09-30 — a spoken test sentence came back word for word through FreEco's own upload path; ~25 s per short clip on this CPU | Owner | Dictate a sentence. |
| Read-aloud without "asterisk" and with a natural local voice | 🟨 | Owner | Read a reply with bold text aloud. |

## 6. Agents, models and providers

| Feature | Status | Who | How to check |
|---|---|---|---|
| Models chosen from a list of working models everywhere (agent config, fallback, spawn) | 🟨 | Owner | Agent settings: model and fallback are drop-downs. |
| Model availability probed at boot (e.g. NVIDIA 70 of 95) | ✅ 2026-09-23 | — | — |
| Fallback chain on timeout, overload, model-not-found; failed chains report every link | ⚠️ parts verified, no live failover observed | Owner | — |
| The popup shows the model the assistant actually uses | 🟨 | Owner | — |
| Scheduled agents not declared crashed between runs | ✅ 2026-09-29 | — | — |
| Reactive agents with an `[autonomous]` block (freeco-shopping) not flagged "unresponsive" | 🟨 fixed in the v0.9.8 build, test added | — | No "Agent is unresponsive agent=freeco-shopping" lines in the log after deploy. |

## 7. Backups

| Feature | Status | Who | How to check |
|---|---|---|---|
| Encrypted backup including agent workspaces | ✅ 2026-09-30 (80 MB, 1,727 files, dry-run restore read all) | Owner | Settings → Backups. |
| Log folded into the database before copying (new) | ✅ 2026-09-30 | — | Archive fell from 222 MB to 80 MB. |
| An archive restore would refuse is never written, and never replaces a good one (new) | 🟨 | — | — |
| Service data (CRM and voice databases, volumes) exported | 🟨 | Owner | `POST /api/backups/services`. |

## 8. Organisation: companies, projects, teams, tasks, schedules

| Feature | Status | Who | How to check |
|---|---|---|---|
| Company, project and team records (`/api/org/*`, dashboard tab) | ✅ v0.9.2 | Owner | — |
| Task board tools: `task_post`, `task_claim`, `task_complete`, `task_list` | ✅ `task_list` 2026-09-30 (CEO) | CEO | `task_list`. |
| Schedules: `schedule_create`, `schedule_list`, `schedule_delete` | ✅ `schedule_list` 2026-09-30 (CEO) | CEO | `schedule_list`. |
| Management rhythm, team fields, authority ladder (Yanovsky) | 📋 | — | See ROADMAP. |

## 8b. Claude Code and Codex as team members

| Feature | Status | Who | How to check |
|---|---|---|---|
| `claude-code` provider runs Claude with FreEco's tools, in a container (`podman:freeco-claude`) | 🟨 not verified live — needs the owner's `CLAUDE_CODE_OAUTH_TOKEN` | Owner | Give an agent model `claude-code`, ask it to use a tool. |
| `POST /mcp` endpoint, 10 tools, `integrations/claude-code-plugin` | 🟨 unit-tested, not verified live — needs `FREECO_MCP_TOKEN` | Owner | Install the plugin in Claude Code, list agents. |
| A reached rate limit or used-up allowance moves down the fallback chain | 🟨 | — | — |

## 9. Known problems — not fixed yet

These are real and open. Do not report them as done.

1. **Idle agents spend tokens.** Continuous-mode hands (trader-hand, browser-hand,
   crm, voice, accounting) are woken every hour with no task; each wake-up
   resends the whole history. About 15 M of 27.8 M input tokens since
   2026-09-15. The CEO's proposal (option C: asleep by default, work wakes an
   agent, the CEO does the work itself) awaits the owner's approval; the
   permanent fix (B) is the next build.
2. ~~"Pause" on a hand does nothing~~ — fixed in the v0.9.8 build: pausing stops the agent's loop, resume restarts it, and a paused hand stays paused across restarts. Not yet verified live.
3. **The database log grows while the daemon runs** — something holds an old
   read open, so the log cannot be folded in (106 MB found on 2026-09-30).
   Backups now work around it; the cause is not found.
4. **Waiting for answers** — FreEco only queues messages to the *same* agent;
   the delays seen are the provider (Novita answering HTTP 500 "overloaded")
   and the retries. Routing through OmniRoute across providers is the CEO's
   decision.
5. **An answer arriving in the wrong chat** — reported by the owner, not yet
   reproduced. Needed: which chat and when.
6. While a reply streams, a masked value can briefly show as
   `FRECO_REDACTED_…`; the finished reply shows the real value.
7. The dashboard walk-through of every changed item (section 1, 2, 5, 6 rows
   marked 🟨 with "Owner") is still to be done before release.
8. 24 old workspace folders with no agent (0.2 MB) wait for the owner's decision.
9. **The `exec` sandbox tier once answered "Docker is not running"** right after
   the `workspace` tier had worked (CEO, 2026-09-30). The runtime probe has a
   five-second limit and the VM was busy; not yet reproduced.
10. **An old CEO schedule runs a host Python command every 6 hours** with the
   revoked CRM key. It contradicts the no-host rule; the CEO recommends
   deleting it once the new key is connected. Waits for the owner.
11. The voice API has an open crash-loop task (4a7733a7, since 2026-09-13); it
   answered 200 on 2026-09-30.
12. **The daemon stops when the Claude desktop app quits.** It is started from
   the assistant's session; on 2026-10-01 quitting the app took it down. Fix:
   start it from Windows at login (a scheduled task) — waits for the owner's
   approval, since it changes a Windows setting.
13. `config.toml` says `listen_addr = 127.0.0.1:4200`, but the daemon answers on
   50051 and 4200 refuses. Not yet explained.
14. The Developer agent lacks `task_post`, `task_claim`, `task_complete` and
   `crm_api`, so through the MCP endpoint it can only use the sandbox.
15. Changing a key at runtime calls `std::env::set_var` from request handlers
   (six places), which is not thread-safe. Safe fix (a secret overlay) is
   planned for 0.9.9; until then, restart after changing keys to be safe.
16. The exact minimum Rust version is unmeasured; 1.87 is a proven lower bound.

## 10. Claimed in the README, never checked in this register (❔)

40 channel adapters; 7 bundled hands (Clip, Lead, Collector, Predictor,
Researcher, Twitter, Browser); 16 security systems (WASM dual-metered sandbox,
Merkle hash-chain audit trail, taint tracking, Ed25519 signed manifests,
secret zeroisation, OFP mutual authentication, capability gates, security
headers, health redaction, subprocess sandbox, prompt-injection scanner, loop
guard, session repair, path traversal prevention, GCRA rate limiter);
27 providers / 123+ models; OpenAI-compatible API; migration from OpenClaw;
desktop app (the installed one is out of date). Each needs its own check
before it is described as working.
