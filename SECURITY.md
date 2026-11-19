# Security Policy

## Supported Versions

| Version | Supported          |
|---------|--------------------|
| 0.3.x   | :white_check_mark: |

## Reporting a Vulnerability

If you discover a security vulnerability in FreEco.ai, please report it responsibly.

**Do NOT open a public GitHub issue for security vulnerabilities.**

### How to Report

1. Email: **jaber@rightnowai.co**
2. Include:
   - Description of the vulnerability
   - Steps to reproduce
   - Affected versions
   - Potential impact assessment
   - Suggested fix (if any)

### What to Expect

- **Acknowledgment** within 48 hours
- **Initial assessment** within 7 days
- **Fix timeline** communicated within 14 days
- **Credit** given in the advisory (unless you prefer anonymity)

### Scope

The following are in scope for security reports:

- Authentication/authorization bypass
- Remote code execution
- Path traversal / directory traversal
- Server-Side Request Forgery (SSRF)
- Privilege escalation between agents or users
- Information disclosure (API keys, secrets, internal state)
- Denial of service via resource exhaustion
- Supply chain attacks via skill ecosystem
- WASM sandbox escapes

## Security Architecture

FreEco.ai implements defense-in-depth with the following security controls:

### Access Control
- **Capability-based permissions**: Agents only access resources explicitly granted
- **RBAC multi-user**: Owner/Admin/User/Viewer role hierarchy
- **Privilege escalation prevention**: Child agents cannot exceed parent capabilities
- **API authentication**: Bearer token with loopback bypass for local CLI

### Input Validation
- **Path traversal protection**: `safe_resolve_path()` / `safe_resolve_parent()` on all file operations
- **SSRF protection**: Private IP blocking, DNS resolution checks, cloud metadata endpoint filtering
- **Image validation**: Media type whitelist (png/jpeg/gif/webp), 5MB size limit
- **Prompt injection scanning**: Skill content scanned for override attempts and data exfiltration

### Cryptographic Security
- **Ed25519 signed manifests**: Agent identity verification
- **HMAC-SHA256 wire protocol**: Mutual authentication with nonce-based replay protection
- **Secret zeroization**: `Zeroizing<String>` on all API key fields, wiped on drop

### Runtime Isolation
- **WASM dual metering**: Fuel limits + epoch interruption with watchdog thread
- **Subprocess sandbox**: Environment isolation (`env_clear()`), restricted PATH
- **Taint tracking**: Information flow labels prevent untrusted data in privileged operations

### Network Security
- **GCRA rate limiter**: Cost-aware token buckets per IP
- **Security headers**: CSP, X-Frame-Options, X-Content-Type-Options, HSTS
- **Health redaction**: Public endpoint returns minimal info; full diagnostics require auth
- **CORS policy**: Restricted to localhost when no API key configured

### Audit
- **Merkle hash chain**: Tamper-evident audit trail for all agent actions
- **Tamper detection**: Chain integrity verification via `/api/audit/verify`

## Dependencies

Security-critical dependencies are pinned and audited:

| Dependency | Purpose |
|------------|---------|
| `ed25519-dalek` | Manifest signing |
| `sha2` | Hash chain, checksums |
| `hmac` | Wire protocol authentication |
| `subtle` | Constant-time comparison |
| `zeroize` | Secret memory wiping |
| `rand` | Cryptographic randomness |
| `governor` | Rate limiting |

## Agent-written code and secrets (2026-11-19)

Three controls landed together, closing the "agent bypassed `web_fetch` SSRF
via `browser_run_js`" report:

- **`browser_run_js` is not a fetch channel.** Every expression is screened
  before the browser sees it (`freeco-kernel-runtime/src/js_guard.rs`):
  network APIs (`fetch`, `XMLHttpRequest`, `WebSocket`, `EventSource`,
  `navigator.sendBeacon`, `import()`, `new Request`/`Image`, workers),
  dynamic code (`eval`, `Function`, `constructor`, `atob`, string timers),
  escapes, global-object bracket access or aliasing, string-built property
  names, HTML code injection and navigation sinks are refused. The error
  points to `web_fetch` (which applies `check_ssrf`) and says credentials
  belong in connectors, hands or environment variables, never in
  agent-written code. The tool also requires human approval by default,
  next to `shell_exec`.
- **The browser's own requests are intercepted.** Every page request is
  paused over CDP (`Fetch.requestPaused`) and failed if it targets a
  private, loopback, link-local or cloud-metadata address, using the same
  `check_ssrf`. This also covers requests the static screen cannot see.
  WebSocket connections are not covered by CDP's Fetch domain; the
  expression screen refuses them instead.
- **Secrets are redacted out of agent output and refused in written
  files.** Known secret values (vault entries, provider keys and channel
  tokens from env/`.env`, MCP header credentials) and well-known key
  patterns are replaced with `[REDACTED:<name|kind>]` in assistant replies
  before they are stored or sent to channels, in tool results before they
  enter the model context and the dashboard, and in outbound channel
  messages. `file_write` content and `apply_patch` added lines containing
  a secret are refused with an instruction to read it from env or the
  vault at runtime.

### Reported `ghp_` token in a release script — checked, not in this repo

A board item reported a GitHub personal access token (`ghp_...`, value not
reproduced here) committed in a `fix_release.py` history. Checked
2026-11-18 against every fetched ref: `git log --all -S` for the token, a
`ghp_[A-Za-z0-9]{36}` regex scan across all 103 local refs, `git log --all
-- fix_release.py`, and a working-tree scan. **No match**: the token is not
in this repository's reachable history on any branch we hold, and no such
file ever existed here. It may live in another repository, or in a
force-pushed or unfetched ref; GitHub retains unreachable commits.
**Revoking it is the owner's decision** and is still recommended regardless
of what this repo contains; this codebase cannot revoke it for you.
