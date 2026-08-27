# Dograh Voice

Real-time voice AI on the user's own machine: live calls with barge-in,
telephony, and voice-campaign workflows.

## What it is

Three containers — the API, Postgres, and Redis — so this hand declares
`compose` rather than `image`.

- API: <http://localhost:8000>
- MCP: `http://localhost:8000/api/v1/mcp`, registered automatically once the
  health check passes.

The MCP path matters. Dograh mounts its MCP server under the API prefix
(`app.mount(f"{API_PREFIX}/mcp", mcp_app)`), so it is `/api/v1/mcp`. Registering
a bare `/mcp` produces an endpoint that looks connected and answers nothing.

## Up is not the same as able to call

The gateway will pass its health check with no telephony provider configured. In
that state every outbound call fails while the service reports healthy. When
asked whether voice works, check whether a provider and credentials are present
and say which of the two is true — "running" on its own is the answer that
wastes the user's afternoon.

## Calls reach real people and cost real money

- Never dial a number or start a campaign on your own initiative. Only when the
  user asked for that specific action.
- A campaign is not a dry run. There is no preview mode that stops short of
  ringing someone's phone.

## Checking it

`hand_status voice` reports `serving` from the runtime. First activation pulls
several gigabytes, so the health timeout is 900s.

```
podman compose -p freeco-voice-<instance> logs
```

Do not start it with shell commands — a stack started outside the hand has no
instance record, and nothing that reports on services can see it. Do not run
`compose down -v`: that destroys call history and campaign state.
