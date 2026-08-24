# OmniRoute Gateway

A single OpenAI-compatible endpoint in front of 340+ model providers, 90+ of
them free. Agents point at one address; OmniRoute decides which provider serves
each request and falls through when a free model runs dry.

## What it is for

Free model tiers have daily caps. When one is exhausted mid-task the agent stops
with a provider error that names a model rather than the cap. Routing through a
gateway turns that into a fallback instead of a failure.

## Using it

The gateway serves `/v1/models` and `/v1/chat/completions` on port 20128.
Point an agent at it by setting its provider to `omniroute` and its model to one
of the `auto/` routes:

- `auto/best-chat` — general conversation
- `auto/best-coding` — code
- `auto/best-reasoning` — analysis
- `auto/best-fast` — latency over quality

## What goes wrong, and what it looks like

**It answers `/v1/models` but every completion fails.** The gateway is up with
no providers connected. It is not a model problem; connect a provider in the
OmniRoute dashboard.

**It returns a stream when a plain response was asked for.** OmniRoute streams
when `stream` is absent, where the OpenAI spec defaults it to false. Send the
field explicitly.

**The container runs but nothing answers.** Two causes, both silent: rootless
podman's default networking publishes a port that forwards nothing, so this hand
uses host networking; and `JWT_SECRET`, `API_KEY_SECRET` and
`STORAGE_ENCRYPTION_KEY` have no defaults — without them it logs `Ready` and
serves nothing.

## Reporting on it

Say what it is actually serving, not what it should. "Up, 115 models, no
providers connected" is useful. "Running" on its own is not.
