# Running real software inside FreEco.ai

How OmniRoute, a CRM, or a voice service run as containers this platform
manages — and why the sandbox that runs agent commands is not the thing that
hosts them.

## The three tiers

One sandbox type cannot serve two opposite purposes. Running an untrusted
command wants a small, offline container that is thrown away. Building a
project or hosting a CRM wants a large, connected one that is kept. Trying to
serve both from one set of defaults is why the sandbox could not build
anything for months: 512 MB and a 60-second timeout are correct for a command
and useless for `pnpm install`, which is killed mid-flight, or for Postgres,
which will not start at all.

| | **exec** | **workspace** | **service** |
|---|---|---|---|
| For | One untrusted command | Build and test | Hosting software |
| Memory | 512 MB | 4 GB | 8 GB |
| Command timeout | 60 s | 30 min | none |
| PIDs | 100 | 1024 | 2048 |
| Network | none | bridge | bridge |
| Workspace | read-only | writable | writable |
| Reaped when idle | yes | hibernated | **never** |
| Ports published | no | no | yes |
| Approval to start | tool policy | once per agent | **every time** |

`timeout_secs` bounds a single command, not the container's life. A container
lives until idle cleanup or max age; a service tier container is exempt from
both, which is what "runs permanently" means here.

Set the tier per call:

```json
{ "command": "pnpm install", "tier": "workspace" }
```

or in `~/.freeco-ai/config.toml`:

```toml
[docker]
tier = "workspace"
```

Explicitly tuned limits still win over the tier, so a config that set
`memory_limit` by hand keeps working.

## Why containers rather than WASM

WASM is the right sandbox for a skill or a plugin: deny-by-default, no
filesystem or network unless granted. It is the wrong one for a service.

A CRM is several processes (a server, a worker, Postgres, Redis), keeps state
on disk, accepts inbound connections, and links native libraries like `libpq`.
None of that fits a WASM module, and forcing it would mean reimplementing an
operating system badly. Containers already solve it.

So: WASM for untrusted extension code, containers for software.

## Runtime

Either Docker or Podman; the runtime is detected at startup and cached.

Podman is preferred where both exist. It is daemonless, so there is no
background service that can wedge and take every call that probes it down with
it — a failure this project hit repeatedly on Windows, where Docker's engine
runs inside WSL and `docker version` can block forever with no error. It is
also rootless by default, where Docker runs containers as root unless
configured otherwise.

The images are OCI and the command surface used here is identical, so nothing
but the binary name changes.

## Adding a service

Services are declared in `~/.freeco-ai/config.toml`, next to everything else.
Deliberately not a separate YAML manifest: a second config language means two
parsers, two schemas, and two places to look when something is wrong.

```toml
[services.omniroute]
image = "ghcr.io/freecodao/omniroute@sha256:..."   # digest, never a tag
ports = ["20128:20128"]
volumes = ["omniroute-data:/app/data"]
healthcheck = "/health"
mcp = true
```

`mcp = true` means the gateway asks the service for its tool list once it is
healthy, and registers those tools for agents to call.

### Multi-container services

Twenty needs a server, a worker, Postgres and Redis. That is not one container
and pretending otherwise is the hardest unsolved part of this design — a
`depends_on: [postgres]` line that never defines postgres is a plan with a hole
in it.

Two honest options, neither yet chosen:

- **A pod.** Podman pods share a network namespace, so the containers reach
  each other on `localhost` exactly as compose expects. Closest to how these
  projects document themselves.
- **`podman-compose`** against the project's own compose file, with FreEco
  supervising the pod rather than each container.

The second is less work and inherits upstream's file, including whatever it
mounts and exposes — which is precisely what the adoption rules below exist to
catch.

## Before adopting anything

Nothing third-party is used directly from upstream. For each service:

1. **Mirror it into `FreecoDAO`**, or update an existing mirror from upstream's
   main.
2. **Audit it** — `npm audit` / `cargo audit`, `postinstall` scripts, telemetry
   endpoints, an SBOM, and the licence.
3. **Compare the forks.** A fork is often well ahead of upstream; this project
   is 300+ PRs beyond OpenFang. Do not assume upstream is the best source.
4. **Pin by digest.** A tag is mutable, and `trivy-action` was compromised in
   this project's own CI while pinned to one.
5. **Then** write the plan.

## Handing over the screen

A service container can show a person what it is doing. `freeco-screen` starts
a virtual display, publishes it over VNC, and serves that over HTTP with noVNC:

```toml
[services.example]
ports = ["6080:6080"]
```

Open `http://127.0.0.1:6080` and the container's screen is in a browser tab, no
VNC client required. The image carries Brave for this — the browser tools
otherwise drive a browser on the *host*, outside the isolation, which makes
"agents work sandboxed" and "agents can browse" two different claims.

It is not started at boot: an exec sandbox runs one command and exits, and
paying for a virtual display on every one of those is waste.

## What exists today, and what does not

| | State |
|---|---|
| Three tiers, limits from the tier | Done |
| Tier requestable per call | Done |
| Podman detection | Done |
| Ports, restart policy, never reaped | Done |
| Approval before a service starts | Done |
| Hibernate instead of destroy | Done |
| Brave + noVNC in the image | In the Dockerfile, **image not yet built** |
| `[services.*]` config and supervisor | **Not built** |
| MCP registration from a service | **Not built** |
| Multi-container (pod or compose) | **Not built, and not yet decided** |
| Volume backup and restore | **Not built** |

The last four are the work. The rest is done and only needed saying.
