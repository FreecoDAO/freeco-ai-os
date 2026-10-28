# MCP endpoint: external clients as team members

`POST /mcp` on the daemon is an MCP server that uses the Streamable HTTP
transport. It lets an external coding client, such as Claude Code or Codex,
work as a member of the owner's FreEco agent team. The client acts as one
of the team's agents:

- it reads and claims work on the shared task board
- it posts progress into agents' conversations, where the owner sees it in
  the dashboard
- it asks other agents
- it runs commands in that agent's sandbox container

The Claude Code plugin and the Codex setup are in
[`integrations/claude-code-plugin/`](../integrations/claude-code-plugin/README.md).
The code is in `crates/freeco-api/src/mcp_http.rs`.

## Transport

- `POST /mcp` takes one JSON-RPC 2.0 message, or a batch, and answers with
  `Content-Type: application/json`. There is no event stream and no
  `Mcp-Session-Id`, because the server keeps no session state.
- Notifications, including `notifications/initialized`, and responses sent
  by the client get `202 Accepted` with an empty body.
- `GET /mcp` and `DELETE /mcp` return `405`.
- Methods: `initialize`, `ping`, `tools/list` and `tools/call`. Any other
  method returns JSON-RPC error `-32601`.
- `initialize` echoes the client's `protocolVersion` if it is one of
  `2025-06-18`, `2025-03-26` or `2024-11-05`. Otherwise it answers
  `2025-06-18`. The server advertises `capabilities: { tools: {} }` and
  `serverInfo: { name: "freeco", version: <crate version> }`.
- A tool that fails returns a normal result with `isError: true` and a text
  block that explains the failure. It is not a JSON-RPC error, so the model
  sees the failure and can act on it. Only an unknown tool name is a
  protocol error (`-32602`).

## Auth

Every request needs `Authorization: Bearer <token>`, where the token equals
the daemon's `FREECO_MCP_TOKEN` environment variable. The comparison runs in
constant time.

**If `FREECO_MCP_TOKEN` is unset or empty, every request is refused with
`401`.** The response tells the owner to set the variable in
`~/.freeco-ai/secrets.env`. Loopback alone is not enough to be trusted here,
because a tool on this endpoint runs commands.

The daemon reads the variable from its environment on each request. A value
added to `secrets.env` takes effect at the next daemon start. A value saved
through `POST /api/vault/secrets` takes effect immediately.

### Authentication layers

Requests to `/mcp` pass through the daemon's global middleware first, the
same as every other route:

1. **CSRF guard** (`csrf.rs`). A `POST` must be addressed to a loopback
   authority. Without an `Origin` header, it must not carry a form content
   type. MCP clients send `application/json` and no `Origin`, so they pass.
   A web page cannot forge a call: a form cannot send JSON, and a browser's
   `fetch` carries a cross-site `Origin`.
2. **API auth** (`middleware::auth`). With no `api_key` set and dashboard
   login off, loopback requests pass. **If an `api_key` is set, or dashboard
   login is on, this layer requires that key or a session cookie. It
   rejects the MCP bearer token before `/mcp` sees the request**, unless the
   two values are the same. `/mcp` is not on that middleware's public list.
   Exempting `/mcp` there, so that it relies on its own token, is a change
   to `middleware.rs` that has not been made yet.
3. The endpoint's own `FREECO_MCP_TOKEN` check.

## Identity and capabilities

The caller names the agent it acts as with the header
`X-Freeco-As-Agent: <agent name or id>`. The default is `Developer`. The
token may act as **any existing agent**. Names resolve against the agent
registry in this order: id, then exact name, then the name in any case.
Before any tool runs, the endpoint resolves the name. If it matches no
agent, the call is refused and the error lists the agents that exist.

**The client gets the acting agent's own capabilities, never more.** Before
an acting tool runs, the endpoint checks that the agent's manifest
(`[capabilities] tools`) grants the matching FreEco tool:

| MCP tool | FreEco tool the agent must hold |
|---|---|
| `freeco_sandbox_exec` | `sandbox_exec` |
| `freeco_crm` | `crm_api` |
| `freeco_claim_task` | `task_claim` |
| `freeco_complete_task` | `task_complete` |
| `freeco_post_task` | `task_post` |

The comparison is the one `tool_runner::execute_tool` enforces: both names
go through `normalize_tool_name`, so a manifest that lists `docker_exec`
grants `sandbox_exec`. `"*"` grants every tool. If the agent lacks the tool,
the result is `isError` with `agent <name> does not hold tool <tool>`.

The kernel treats an empty `tools` list as "all tools". This endpoint does
not: an empty list grants nothing here, so an external client gets only what
the manifest writes down.

So acting as an agent whose manifest has `"*"`, such as the CEO, gives that
agent's tools, and acting as `Developer` gives Developer's. To let a client
use the task board, the agent it acts as needs `task_post`, `task_claim` and
`task_complete` in its tools.

The other tools only read or talk, and they may target any agent, because
that is how the team talks to each other: `freeco_list_agents`,
`freeco_list_tasks`, `freeco_read_chat`, `freeco_post_to_chat` and
`freeco_ask_agent`.

The acting agent is also recorded:

- `freeco_claim_task` claims the task for this agent.
- `freeco_post_task` records the task as created by this agent.
- `freeco_sandbox_exec` runs in this agent's sandbox.
- `freeco_ask_agent` tells the agent it asks who is sending the message.
- `freeco_post_to_chat` labels every post `[<client> as <acting agent>] `,
  for example `[Claude Code as Developer] …`. The label names the acting
  agent whichever conversation the post goes into.

`X-Freeco-Client` is optional and defaults to `Claude Code`. It sets the
client part of that label, for example `[Codex as Tester] …`.

## Tools

| Tool | Arguments | What it does |
|---|---|---|
| `freeco_list_agents` | none | Each agent's id, name, state, model provider and model |
| `freeco_list_tasks` | `status?` (`pending`, `in_progress`, `completed`) | The shared task board, newest first, with agent names beside agent ids |
| `freeco_post_task` | `title`, `description`, `assignee?` | Needs `task_post`. Posts a task. The assignee name is resolved and stored as the agent id, which is what the board's claim matches on |
| `freeco_claim_task` | `task_id?` | Needs `task_claim`. With an id, claims that task if it is pending and open or assigned to the caller. Without an id, claims the next pending task for the caller (`MemorySubstrate::task_claim`) |
| `freeco_complete_task` | `task_id`, `result` | Needs `task_complete`. Marks the task completed (`task_complete`) |
| `freeco_read_chat` | `agent`, `limit?` (default 30, max 200) | Recent messages of any agent's conversation, oldest first, as role and text. This is the conversation the dashboard shows (`canonical_context`) |
| `freeco_post_to_chat` | `agent`, `text` | Appends a user-role message labelled `[<client> as <acting agent>] ` to any agent's conversation (`append_canonical`). Dashboard chats that have that agent open are updated live over the WebSocket (`ws::broadcast_to_ws`) |
| `freeco_ask_agent` | `agent`, `message` | Runs a turn of any agent through `send_message_with_handle_and_blocks`, the same call `POST /api/agents/{id}/message` makes, and returns its reply. Waits up to 600 s. After a timeout the turn keeps running and its reply lands in the agent's conversation |
| `freeco_sandbox_exec` | `command`, `tier?` (`exec` or `workspace`, default `workspace`) | Needs `sandbox_exec`. Runs the command in the acting agent's sandbox container (`tool_runner::tool_docker_exec`), the same path as the agent's `sandbox_exec` tool and the dashboard terminal |
| `freeco_crm` | `method?`, `path`, `body?` | Needs `crm_api`. Passes the request to `crm_tool::crm_api`, which reads the CRM key from the vault itself |

The emergency freeze (`/api/system/freeze`) refuses `freeco_ask_agent` and
`freeco_sandbox_exec`.

## How it differs from `freeco-ai mcp`

| | `freeco-ai mcp` (stdio, `freeco-cli`) | `POST /mcp` (this endpoint) |
|---|---|---|
| Transport | stdio, Content-Length framing; the client launches a process | HTTP on the running daemon |
| Model | Each agent is a tool, `freeco_agent_<name>`, that you send a message to | The client is a team member acting as one agent, with that agent's capabilities, through a fixed set of ten tools |
| Scope | Talking to agents | Task board, chats, asking agents, sandbox, CRM |
| Auth | Whoever can start the process | Bearer token `FREECO_MCP_TOKEN`, refused when unset |
| Daemon needed | No: it falls back to an in-process kernel | Yes |

## Security model

- **Loopback only.** The CSRF guard refuses a state-changing request that
  is not addressed to `127.0.0.1`, `localhost` or `::1`. That covers every
  MCP call and defeats DNS rebinding. The daemon's default bind is
  loopback.
- **Token required, fail closed.** No token configured means no access, even
  from loopback. Wrong or missing tokens get `401`. The route is not exempt
  from the CSRF guard.
- **The token is as strong as the most capable agent.** It may act as any
  agent, and it gets exactly that agent's manifest tools. A token holder who
  acts as an agent granted `"*"` can do whatever that agent can. Treat the
  token like the owner's own key, and narrow what it can reach by
  narrowing agents' `[capabilities] tools`.
- **Sandbox-only execution.** No tool runs anything on the host.
  `freeco_sandbox_exec` uses the agent's container with its workspace
  mounted at `/workspace`, under the same tier limits, image pinning and
  runtime checks as the agent's own `sandbox_exec`. If the sandbox is
  disabled or the container runtime is down, the call fails and nothing runs.
- **No secrets through the endpoint.** The CRM key is read from the vault
  by the CRM tool. No tool returns a key. The plugin skill tells the client
  never to ask for keys in chat.
- **Visible to the owner.** Anything posted to a chat is labelled with the
  client's name and the agent it acted as. Every tool call is logged with the tool name, the acting
  agent and the client (`MCP tool call` in the daemon log). Arguments are
  not logged, because they can contain data the owner would not want in a
  log.
