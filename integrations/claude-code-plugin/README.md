# FreEco.ai plugin for Claude Code (and setup for Codex)

This plugin connects Claude Code to your FreEco.ai daemon's MCP endpoint
(`POST /mcp`). Claude Code then works as a member of your agent team. It can:

- read and claim tasks on the shared board
- post progress to an agent's chat, where you see it in the dashboard
- ask other agents
- run commands in a FreEco sandbox container. It never runs commands on
  your machine directly.

Contents:

| Path | What it is |
|---|---|
| `.claude-plugin/plugin.json` | Plugin manifest |
| `.claude-plugin/marketplace.json` | Lets this directory be added as a marketplace |
| `.mcp.json` | The `freeco` MCP server (HTTP, `http://127.0.0.1:50051/mcp`) |
| `skills/freeco-team/SKILL.md` | How to behave as a team member |

The endpoint itself, its tools and its security model are described in
[`docs/mcp-endpoint.md`](../../docs/mcp-endpoint.md).

## 1. Give the daemon a token

The endpoint refuses every request until `FREECO_MCP_TOKEN` is set in the
daemon's environment.

Generate a long random value, for example in Git Bash:

```bash
openssl rand -hex 32
```

or in PowerShell:

```powershell
$b = New-Object byte[] 32; [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($b); -join ($b | ForEach-Object { $_.ToString('x2') })
```

Store it in the daemon in one of these two ways:

- **`~/.freeco-ai/secrets.env`**: add the line `FREECO_MCP_TOKEN=<value>`.
  The daemon reads this file when it starts, so this way takes effect after
  the next restart.
- **The vault endpoint** (works immediately, no restart): post the token
  to `/api/vault/secrets`. The dashboard's "save this key" prompt uses the
  same endpoint. It stores the value in the vault, writes it to
  `secrets.env`, and sets it in the running daemon:

  ```bash
  curl -s -X POST http://127.0.0.1:50051/api/vault/secrets \
    -H "Content-Type: application/json" \
    -d "{\"name\":\"FREECO_MCP_TOKEN\",\"value\":\"$FREECO_MCP_TOKEN\"}"
  ```

  This command reads the token from `$FREECO_MCP_TOKEN` in your shell, so
  set that variable first (step 2). That keeps the value itself out of
  your shell history.

## 2. Give the client the same token

Claude Code expands `${FREECO_MCP_TOKEN}` from its own environment when it
reads `.mcp.json`. Set the variable for your user account so every new
terminal has it:

```powershell
setx FREECO_MCP_TOKEN "<the same value>"
```

On macOS or Linux, export it from your shell profile instead. Optionally set
`FREECO_AS_AGENT` to the FreEco agent Claude Code should act as. It defaults
to `Developer`. Any existing agent can be named. The name is not
case-sensitive, and an agent id works too. A name that matches no agent is
refused.

```powershell
setx FREECO_AS_AGENT "Tester"
```

Open a new terminal after `setx`. Terminals that are already open keep their
old environment.

### What the acting agent must hold

Claude Code gets the acting agent's own capabilities, never more. Each tool
that acts needs the matching tool in that agent's manifest, under
`[capabilities] tools` in `~/.freeco-ai/agents/<agent>/agent.toml`:

| To use | The agent needs |
|---|---|
| `freeco_sandbox_exec` | `sandbox_exec` (the older name `docker_exec` also counts) |
| `freeco_post_task` / `freeco_claim_task` / `freeco_complete_task` | `task_post` / `task_claim` / `task_complete` |
| `freeco_crm` | `crm_api` |

`"*"` grants every tool. An empty list grants none of these. If the agent
lacks one, the tool answers `agent <name> does not hold tool <tool>`.

Reading the board and chats, posting to chats, and asking agents work for
any acting agent. Posts are labelled `[Claude Code as <agent>] ` so you can
see who wrote them.

**The token can act as any agent, including one granted `"*"`. Treat it
like your own key.**

## 3. Install in Claude Code

**From this directory as a marketplace:**

```bash
claude plugin marketplace add ./integrations/claude-code-plugin
claude plugin install freeco@freeco-plugins
```

You can also run the same commands inside a session:
`/plugin marketplace add ./integrations/claude-code-plugin`, then
`/plugin install freeco@freeco-plugins`.

**For one session, without installing:**

```bash
claude --plugin-dir ./integrations/claude-code-plugin
```

Check the connection with `/mcp` inside Claude Code. The `freeco` server
should be listed as connected, with ten tools.

**Without the plugin**, add only the server:

```bash
claude mcp add --transport http freeco http://127.0.0.1:50051/mcp \
  --header "Authorization: Bearer $FREECO_MCP_TOKEN" \
  --header "X-Freeco-As-Agent: Developer"
```

This writes the token's value into your Claude Code config. The plugin keeps
it as a `${FREECO_MCP_TOKEN}` reference instead.

### Port

The plugin points at `127.0.0.1:50051`. If your daemon listens elsewhere,
check `[network] listen_addr` in `~/.freeco-ai/config.toml` (FreEco's default
is `127.0.0.1:4200`). Then change `url` in `.mcp.json`, or use the
`claude mcp add` form above with the right port.

### Timeout

`freeco_ask_agent` waits up to 600 seconds for an agent's reply. Claude Code
gives an HTTP MCP request at least 60 seconds to answer. A longer wait needs
the server's `timeout`, so `.mcp.json` sets `"timeout": 660000` (11 minutes,
in milliseconds).

## Codex

Codex has no plugin format for this, so set it up by hand. Add the server
to `~/.codex/config.toml`:

```toml
[mcp_servers.freeco]
url = "http://127.0.0.1:50051/mcp"
bearer_token_env_var = "FREECO_MCP_TOKEN"
env_http_headers = { "X-Freeco-As-Agent" = "FREECO_AS_AGENT" }
http_headers = { "X-Freeco-Client" = "Codex" }
tool_timeout_sec = 660
```

Or run the equivalent command, then add the header and timeout lines to the
table it creates:

```bash
codex mcp add freeco --url http://127.0.0.1:50051/mcp --bearer-token-env-var FREECO_MCP_TOKEN
```

- `bearer_token_env_var` names the variable that holds the token. The token
  itself stays out of the file.
- `env_http_headers` maps a header to the name of an environment variable.
  If `FREECO_AS_AGENT` is unset, the header is not sent and the daemon uses
  `Developer`.
- `X-Freeco-Client: Codex` labels Codex's chat posts `[Codex as <agent>]`.
  Without it they would be labelled `[Claude Code as <agent>]`.
- `tool_timeout_sec` defaults to 60 in Codex, which is too short for
  `freeco_ask_agent`.

These keys were checked against the Codex MCP documentation
(learn.chatgpt.com/docs/extend/mcp) on 2026-10-01. If your Codex version
rejects one, check `codex mcp --help` and that page for your version.

The team-member instructions in `skills/freeco-team/SKILL.md` apply to Codex
as well. Codex does not load this plugin's skill, so copy that file's body
into the `AGENTS.md` of the project you work in. The server also sends a
short version of these instructions in its `initialize` response.

## If it does not connect

- **401 "switched off: no token is configured"**: the daemon does not have
  `FREECO_MCP_TOKEN`. See step 1.
- **401 "Invalid MCP token"**: the client's value differs from the daemon's.
- **401 "API key required" / "Invalid API key"**: you have set an `api_key`
  or turned on dashboard login. The daemon's general API authentication then
  runs before the MCP endpoint and does not know the MCP token. See
  "Authentication layers" in `docs/mcp-endpoint.md`.
- **403 "not addressed to the loopback interface"**: the URL must use
  `127.0.0.1` or `localhost`.
- **A tool says "No agent named …"**: `FREECO_AS_AGENT` names an agent that
  does not exist. The error lists the agents that do.
- **A tool says "agent X does not hold tool Y"**: add `Y` to that agent's
  `[capabilities] tools`, or act as an agent that has it. See "What the
  acting agent must hold" above.
