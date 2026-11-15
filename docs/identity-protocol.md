# FreEco.ai identity and node pairing protocol

_Design draft v0.1, 2026-11-15. Status: proposal for Free to review. Nothing
here is built yet. Code references are to `main` at d382cea (v0.9.8) unless a
branch is named. Domain names such as `app.freeco.ai` are placeholders until
the production hosts are chosen. Sizes in section 13 are estimates, not
measurements._

---

## 1. The short version

FreEco.ai keeps three identities apart and never lets one stand in for
another:

| Identity | Proves | Issued by | Lives where |
|---|---|---|---|
| **Account** (human) | "This is the person who signed in" | The identity provider (Zitadel, OIDC, passkeys) | Cloud |
| **Node** (device) | "This is one particular registered computer or phone, controlled by that person" | The node itself makes its key; the FreEco.ai node registry records the public half | Private key on the device, public key in the registry |
| **Session and scopes** (application) | "Right now, this caller may do these specific things" | Short-lived tokens from the cloud, checked again by the node | Memory only |

Everything in this document follows from four rules:

1. **The cloud recognises nodes; it never holds their private keys.** A node
   makes its own key pair, keeps the private key in the operating system's
   secure storage, and proves possession on every connection.
2. **The node has the last word.** Every request from the cloud is signed,
   scoped and checked against the node's own policy, which the cloud cannot
   widen on its own. A cloud login therefore never equals control of a home
   node.
3. **A channel address is not a credential.** A WhatsApp number identifies a
   guest and starts a conversation. It never on its own approves a payment, a
   stored-password login or a change to a node.
4. **Each kind of recovery has its own path.** Recovering the account does not
   recover the node's keys, and neither one recovers the user's encrypted
   data. For a node the user runs, only the user's Recovery Kit does that. A
   hosted cell is different, and section 11 says how.

This gives Fre.Eco a privacy story Instinct cannot match: Instinct keeps your
credentials on a cloud computer it controls, while a Fre.Eco node keeps them
on your own machine and only accepts the requests you allowed.

The same protocol serves all three deployment modes (section 11): cloud only,
hybrid, and fully sovereign. A FreEco-hosted workspace cell is simply a node
that FreEco runs for you, connected the same way and labelled as hosted.

---

## 2. What exists today and what this builds on

The codebase already has most of the parts. The protocol mainly gives them a
shared identity model.

| Area | Today (file) | What changes |
|---|---|---|
| Dashboard login | One API key, or one password with an HMAC cookie `username:expiry:hmac` (`freeco-api/src/session_auth.rs`, `middleware.rs`) | Kept for local and sovereign mode. In the cloud, the web app signs in through OIDC and the browser holds only a session cookie (section 5.1) |
| Users and roles | `AuthManager` maps `channel:platform_id` to users from config `channel_bindings` (`freeco-kernel/src/auth.rs`); `TenancyStore` has tenants, users and Owner/Admin/User/Kid/Viewer (`freeco-memory/src/tenancy.rs`) | `tenant_id` becomes the workspace id. `users.provider` records `oidc:zitadel` and the subject. Channel bindings come from verified links, each with an assurance level (section 8) |
| Device pairing | `PairingManager` makes a 64-hex token valid 5 minutes, max 10 devices (`freeco-kernel/src/pairing.rs`, `/api/pairing/*` in `routes.rs`) | Today the device sends no key, the server picks a random id, and nothing is issued that the device could later prove itself with. `/api/pairing/complete` also sits behind the API key, so a phone must already hold the admin key to pair. Phase A makes local pairing key-based (section 6.4) |
| Node-to-node | OFP handshake with one pre-shared secret and HMAC, plus a 5-minute nonce tracker (`freeco-wire/src/peer.rs`) | One shared secret cannot revoke a single node. Later, peers authenticate with node certificates (Phase F). The `NonceTracker` is reused for DPoP and request replay caches |
| MCP endpoint | Static bearer `FREECO_MCP_TOKEN` plus `X-Freeco-As-Agent` (`freeco-api/src/mcp_http.rs`) | Accepts DPoP-bound node and client tokens whose claims name the agents they may act as |
| Secrets | AES-256-GCM vault, master key in the OS keyring (`freeco-extensions/src/vault.rs`) | The node identity key is stored next to it, in hardware where available (section 4) |
| Signing | `ed25519-dalek` is already a workspace dependency (`freeco-types/src/manifest_signing.rs`) | Reused for software node keys and certificate checks; `p256` is added for hardware-backed keys |
| Local policy | Approvals with Low/Medium/High/Critical risk (`freeco-kernel/src/approval.rs`, `freeco-types/src/approval.rs`), per-agent capabilities (`capabilities.rs`) | This is the node's policy engine. Cloud requests carry a risk class that maps straight onto `RiskLevel` |
| Audit | Merkle hash chain (`freeco-kernel-runtime/src/audit.rs`) | New actions: `NodePaired`, `NodeRevoked`, `CloudRequest`, `GrantChanged`, `ApprovalProof` |
| WhatsApp | Cloud API adapter has an allowlist that is never called (`freeco-channels/src/whatsapp.rs`, `is_allowed` is dead code). The Baileys gateway gains `WHATSAPP_ALLOWED_NUMBERS` / `WHATSAPP_ALLOWED_GROUPS` and one conversation per contact on branch `claude/project-thread-antq0c` (c6afffe), not yet merged | The allowlist becomes per-node policy. In the cloud, numbers map to accounts through verified channel links (section 8) |
| Desktop app | Tauri app with tray and updater (`freeco-desktop`) | Shows the pairing code, the node fingerprint and the local permission screen |

One rule from the current code matters for the relay: several handlers trust
a loopback caller as the owner (for example `auth_disable` in `routes.rs`, and
the no-key mode in `middleware.rs`). **Relayed requests must enter through the
node-link handler and never be proxied to `127.0.0.1:4200`**, or every cloud
request would look like the owner sitting at the machine.

---

## 3. Components

```
                         ┌──────────────── FreEco.ai cloud ────────────────┐
  Browser ──HTTPS──▶ app.freeco.ai (web app + backend-for-frontend)        │
  Phone app ─HTTPS─▶ id.freeco.ai   Zitadel: OIDC, passkeys, MFA, recovery │
                         │ api.freeco.ai   control plane (freeco-cloud):    │
  WhatsApp ─webhook▶     │   accounts, workspaces, node registry, pairing, │
  (Meta Cloud API)       │   channel links, entitlements, trust document   │
                         │ relay.freeco.ai node link (WebSocket over TLS)  │
                         │ mcp.freeco.ai   MCP gateway; LLM and research   │
                         │                 gateways under api.freeco.ai    │
                         └───────────▲──────────────────▲──────────────────┘
                                     │ outbound only     │ outbound only
                         ┌───────────┴────┐     ┌────────┴───────────────┐
                         │ Home node      │     │ Hosted node (cell)     │
                         │ agents, memory,│     │ same kernel, run by    │
                         │ files, keys,   │     │ FreEco.ai, labelled    │
                         │ local policy   │     │ "Hosted" on the device │
                         └────────────────┘     │ page                   │
                                                └────────────────────────┘
```

- **Zitadel** authenticates humans only. Nodes never become Zitadel users, so
  node trust does not depend on the identity vendor, and a sovereign
  organisation can run the registry without it.
- **The node registry** (in the new `freeco-cloud` crate, as planned in
  `/mnt/project-files/plans/concierge-production-plan.md` section 4) is a
  small authorization server for nodes. It records public keys, issues node
  certificates and short-lived node tokens, and publishes revocations.
- **The relay** holds the outbound WebSocket each node opens. Nodes and
  hosted cells connect to it the same way, so no node ever needs an open
  inbound port.

### 3.1 Cloud keys and the root of trust

The cloud uses separate keys so that one leak does not give everything:

| Key | Signs | Rotation |
|---|---|---|
| Registry token key | Node access tokens and node certificates | Every 90 days. The retired key signs nothing new but stays in the trust document as verify-only for 31 days, longer than the longest certificate it signed (30 days) plus clock skew |
| Request key | Every cloud-to-node request envelope and approval decision | Every 90 days, verify-only for 1 day after (envelopes live 60 s) |
| Zitadel keys | Human ID and access tokens | Managed by Zitadel |
| **Root key** (offline, two hardware tokens held by two people in separate places) | Only the trust document listing the keys above | Rare; used for emergency rotation |

Nodes pin the root public key in the binary and fetch
`GET https://api.freeco.ai/.well-known/freeco-trust.json` daily. The document
is signed by the root key, so a stolen online key can be replaced without
shipping a new app, and a forged trust document is rejected.

A valid signature proves the document is genuine, not that it is current, so
nodes also check freshness:

- Every document has a `serial` that only goes up. The node stores the
  highest serial it has accepted and refuses any document with a lower one,
  so a relay or edge cannot replay an older document that still lists a
  leaked key.
- Every `kid` the node has ever seen in `revoked_kids` stays revoked locally,
  whatever a later document says.
- A document is valid until its `expires_at`, 7 days after issue. If the node
  cannot fetch a newer one by then, it keeps working locally but refuses
  cloud requests until it can (section 10).

```json
{
  "v": 1,
  "serial": 42,
  "issued_at": "2026-11-15T00:00:00Z",
  "next_update": "2026-11-16T00:00:00Z",
  "expires_at": "2026-11-22T00:00:00Z",
  "keys": [
    { "kid": "reg-2026-11", "use": "node-token", "status": "active", "kty": "EC", "crv": "P-256", "x": "…", "y": "…" },
    { "kid": "reg-2026-08", "use": "node-token", "status": "verify_only", "verify_until": "2026-12-16T00:00:00Z", "kty": "EC", "crv": "P-256", "x": "…", "y": "…" },
    { "kid": "req-2026-11", "use": "node-request", "status": "active", "kty": "EC", "crv": "P-256", "x": "…", "y": "…" }
  ],
  "revoked_kids": [],
  "sig": "<JWS by the root key over the canonical JSON above>"
}
```

---

## 4. Identifiers and keys

### 4.1 Identifiers

| Thing | Format | Note |
|---|---|---|
| Account | `usr_<ULID>` | Maps to the Zitadel subject; never a display name (`tenancy.rs` explains why) |
| Workspace | `ws_<ULID>` | The existing tenant id. One person can have several (personal, business) |
| Node | `nod_<26 chars>` | The first 26 characters (130 bits) of the lower-case, unpadded base32 encoding of the 32-byte SHA-256 JWK thumbprint (RFC 7638) of the node's public key; the full encoding is 52 characters, and this short form is deliberately truncated. The registry also stores the full thumbprint as `jkt` (base64url, 43 characters), refuses a registration whose short id already exists, and compares the full `jkt` in every security check. Derived from the key, so it cannot be claimed by another device |
| Channel link | `chl_<ULID>` | One per verified WhatsApp number, Telegram id or email address |
| Pairing | `pair_<ULID>` | Lives at most 10 minutes |
| Request | `req_<ULID>` | Unique per cloud-to-node request; replay-checked |
| Approval | existing UUID | As in `approval.rs` today |

**Fingerprint words.** Wherever a person must compare two screens, show the
key thumbprint as four words from the EFF short word list (about 41 bits), for
example `otter maple violet nine`, plus the first 8 hex characters. The EFF
list rather than BIP-39, so nobody mistakes it for a wallet seed.

### 4.2 Keys on a node

| Key | Type | Where | Purpose |
|---|---|---|---|
| **Node identity key** | P-256 (ES256) when hardware-backed; Ed25519 (EdDSA) in software | Windows TPM through CNG, macOS and iOS Secure Enclave, Android StrongBox or Keystore; otherwise the OS keyring, like the vault key | Signs DPoP proofs, the registration payload, token assertions and every frame the node sends. Never exported |
| **Node encryption key** | X25519 | Wrapped by the vault master key | Receives sealed messages and queued jobs (HPKE, RFC 9180) |
| Vault master key | AES-256 | OS keyring (exists today) | Protects local secrets |
| Data keys | AES-256-GCM | Wrapped by the vault key | Encrypt databases and backup snapshots |
| **Backup key** | AES-256 | Wrapped twice: by the vault key, and by the Recovery Key | Lets a backup be restored on a new machine |
| **Recovery Key** | 160 bits, printed as 8 groups of 4 characters (Crockford base32) and a QR code | On paper only, generated on the node | The only way to read backups without the old node |

The registry records how the node **says** its identity key is protected
(`hardware`, `os_keystore` or `software`). Possession of a P-256 key proves
nothing about where it is stored, so until the registry verifies hardware
attestation (TPM key attestation, Apple App Attest, Android Key Attestation;
Phase F), this value is advisory: the device page shows it as "reported by
the device", and no policy or plan may depend on it. Once attestation is
verified, the registry records `key_protection_verified = true`, and only
then can a policy require hardware keys. Hardware keys are P-256 because the
Secure Enclave offers only P-256, and TPM 2.0 chips rarely offer Ed25519.

Podman nodes, such as the daemon on X2, usually have no TPM inside the
container. They use a software key stored in a mounted secrets volume,
`key_protection = "software"`, and the device page says so.

---

## 5. The four credentials, with lifetimes

There is no universal token. Each credential does one job.

| # | Credential | Format | Held by | Lifetime | Bound to | Revoked by |
|---|---|---|---|---|---|---|
| 1 | **Web session** | Opaque `__Host-freeco_sess` cookie (HttpOnly, Secure, SameSite=Lax) plus a CSRF token, as `csrf.rs` does now | Browser | Idle 12 h, absolute 14 days | The backend-for-frontend's session record | Sign out, "sign out everywhere", account recovery |
| 2 | **Cloud access token** | JWT from Zitadel (humans) or the registry (nodes), audience-restricted | The web backend, the phone app, a node | 10 minutes | DPoP key for apps and nodes (`cnf.jkt`) | Expiry; node status checked on every relay request |
| 3 | **Node credential** | The node identity key, plus a **node certificate** (a registry-signed JWT) | Node | Key: until revoked. Certificate: 30 days, renewed from day 15 | The node's public key | Device page, self-unpair, registry revocation list |
| 4 | **Local encryption keys** | Section 4.2 | Node, and paper for the Recovery Key | Until the user rotates them | The device and the user | Only the user |

Around these sit a few short-lived values:

| Value | Lifetime |
|---|---|
| Phone app refresh token (rotating, DPoP-bound) | 30 days sliding, 90 days absolute |
| Step-up freshness for sensitive actions (revoke, widen grants, link a channel) | Passkey used in the last 10 minutes |
| Node token assertion (RFC 7523) | At most 60 s, single-use `jti` |
| DPoP proof | `iat` within ±60 s; `jti` cached 5 minutes |
| Pairing user code | 10 minutes, single use, 5 wrong attempts |
| QR pairing offer | 5 minutes (the current `token_expiry_secs` default) |
| Cloud-to-node request envelope | Must be accepted within 60 s; a stream may then run until done |
| Approval request | 4 h default, 24 h maximum (the existing `ApprovalPolicy.timeout_secs`) |
| Channel link code, claim link | 10 and 15 minutes, single use |
| Revocation list | `next_update` 1 h; the relay enforces revocation within 5 s |
| Queue for an offline node | 7 days, sealed to the node's encryption key |

### 5.1 Why these choices

- **Browsers never hold tokens.** The web app uses the backend-for-frontend
  pattern: it runs OIDC Authorization Code with PKCE against Zitadel on the
  server side and gives the browser only the session cookie. A script
  injected into the page has no token to steal.
- **Phone and desktop apps** use Authorization Code with PKCE through the
  system browser (RFC 8252), with DPoP-bound tokens.
- **Nodes use DPoP (RFC 9449), not bearer tokens.** A stolen node token is
  useless without the node's private key. DPoP works at the HTTP layer, so it
  survives an edge proxy or web application firewall that terminates TLS.
  Mutual TLS would not, which is why mTLS is kept as an option for enterprise
  relays behind FreEco's own ingress (Phase F).
- **Nodes need no refresh token.** A node mints a new 10-minute access token
  whenever it needs one by signing an assertion with its key. The only
  long-lived secret is the key itself, which stays in hardware where possible.

---

## 6. Flows

All endpoints below are on `https://api.freeco.ai` unless shown otherwise.
Request and response bodies are JSON. Errors follow RFC 6749 and RFC 8628
names (`authorization_pending`, `slow_down`, `access_denied`,
`expired_token`, `invalid_grant`), and DPoP errors use
`WWW-Authenticate: DPoP error="…"`, including `use_dpop_nonce` when the server
wants a fresh nonce.

### 6.1 Account registration (web first)

1. The user opens `app.freeco.ai` and picks "Create account".
2. The web backend starts OIDC Authorization Code with PKCE against
   `id.freeco.ai`. Zitadel offers a passkey first, then Google, Apple or
   Microsoft. Email is verified.
3. On callback the backend calls `POST /v1/accounts` (internal) with the OIDC
   subject. The control plane creates `usr_…` and a personal workspace
   `ws_…` (the tenant), and records the terms and privacy versions accepted,
   with the time.
4. The browser gets the session cookie. The user lands on "My devices", which
   offers "Connect WhatsApp", "Add this computer" and "Create your Recovery
   Kit" (the last once a node exists).

Passkey-only accounts are first class. The crypto panel asked for sign-in
without a phone number, and this gives it.

### 6.2 Account from WhatsApp (today's identifier)

This is the host-QR flow from the founder tests: a guest scans a QR code at
check-in and starts chatting, with no sign-up. Section 8 has the assurance
rules that keep it safe.

1. The host's QR code opens
   `https://wa.me/<FreEco number>?text=Hi%20Fre.Eco%20%23H7K2Q`. `H7K2Q` is a
   host invite code: it names the host and the stay, and limits how many
   guests can start from it.
2. The channel gateway receives the Cloud API webhook, verifies
   `X-Hub-Signature-256`, and looks up the sender. No link exists, so it
   validates the invite code and creates a **provisional account**:
   `usr_…` with `status = "provisional"`, a personal workspace, the plan the
   host's deal gives, and a channel link at assurance `a1`.
3. The first reply says it is FreEco's AI assistant (EU AI Act Art. 50) and
   the guest can chat with the Concierge right away.
4. A message from an unknown number **without** a valid invite code gets a
   short introduction and the sign-up link, and no agent run. That keeps bots
   from spending model budget.

**Upgrading to a full account.** When the guest asks, or the first time an
action needs more than `a1`, the Concierge sends a single-use claim link
`https://app.freeco.ai/claim/<token>` (15 minutes, bound to that channel
link). The claim page asks the guest to create a passkey, then shows a code
and a one-tap button: "Send `FRE 482193` to Fre.Eco on WhatsApp". When that
message arrives from the same number, the account becomes `verified`. The
reverse code proves that the person at the browser controls the WhatsApp
account at that moment, so a forwarded link alone cannot take the account.

### 6.3 Pairing a node with the account

Two entry points, one result. Both bind the pairing to the node's key from the
first message, so a stolen code is useless to any other device.

**A. The node shows a code** (default; headless Podman nodes, the dashboard).
This follows the OAuth Device Authorization Grant (RFC 8628), with every
request DPoP-signed by the node key.

```http
POST /v1/node-pairing/start
DPoP: eyJ0eXAiOiJkcG9wK2p3dCIsImFsZyI6IkVTMjU2IiwiandrIjp7Li4ufX0.…

{
  "registration": "<compact JWS, header {typ: freeco-node-reg+jwt, alg, jwk: node_key}, signed by the node key>"
}
```

The registration JWS payload carries everything the registry will record, so
nothing in it can be swapped on the way:

```json
{
  "node_key": { "kty": "EC", "crv": "P-256", "x": "…", "y": "…" },
  "enc_key":  { "kty": "OKP", "crv": "X25519", "x": "…" },
  "node_info": {
    "name": "Home computer",
    "kind": "personal_node",
    "platform": "windows",
    "os_version": "11 24H2",
    "app_version": "0.9.9",
    "key_protection": "hardware"
  },
  "aud": "https://api.freeco.ai/v1/node-pairing/start",
  "iat": 1763235602,
  "jti": "b1f0…"
}
```

A DPoP proof binds only the method, the URL and the token, not the body. The
server therefore checks three things before it registers any key: the JWS
signature verifies with the `jwk` in its header; that `jwk` equals
`node_key` in the payload and the DPoP proof's `jwk`; and `aud`, `iat` (±60 s)
and `jti` (single use) are valid. An edge that terminates TLS can then
neither replace `enc_key` nor rename the node. Then the server answers:

```json
{
  "pairing_id": "pair_01JD3Q…",
  "device_code": "GmRhmhcxhwAzkoEqiMEg_DnyEysNkuNhszIySk9eS",
  "user_code": "WDJB-MJHT",
  "verification_uri": "https://app.freeco.ai/pair",
  "verification_uri_complete": "https://app.freeco.ai/pair?code=WDJB-MJHT",
  "fingerprint": "otter maple violet nine",
  "expires_in": 600,
  "interval": 5
}
```

The node shows the code and the fingerprint words in its dashboard. The user,
signed in at `app.freeco.ai/pair`, types the code (typed, not clicked, which
blunts device-code phishing):

```http
POST /v1/pairing/lookup            (web backend, session cookie + CSRF)
{ "user_code": "WDJB-MJHT" }

200 { "pairing_id": "pair_01JD3Q…", "node_info": { … }, "fingerprint": "otter maple violet nine",
      "approx_location": "Geneva, CH", "requested_at": "2026-11-15T19:40:02Z" }
```

The page asks: "Does your computer show *otter maple violet nine*?" and offers
a workspace and a permission preset. Confirming requires a passkey used in the
last 10 minutes:

```http
POST /v1/pairing/approve
{ "pairing_id": "pair_01JD3Q…", "workspace_id": "ws_01H…", "name": "Home computer",
  "preset": "concierge" }
```

Meanwhile the node polls, signing each poll with the same key:

```http
POST /v1/node-pairing/token
DPoP: <proof, htm=POST, htu=https://api.freeco.ai/v1/node-pairing/token>
{ "device_code": "GmRhmhcxhwAzkoEqiMEg_DnyEysNkuNhszIySk9eS" }

400 { "error": "authorization_pending" }        (until approved)
200 {
  "node_id": "nod_k3f9q2…",
  "account_id": "usr_01H…",
  "workspace_id": "ws_01H…",
  "node_certificate": "eyJ0eXAiOiJmcmVlY28tbm9kZS1jZXJ0K2p3dCIs…",
  "access_token": "eyJ…",
  "token_type": "DPoP",
  "expires_in": 600,
  "proposed_grants": ["node.status:read", "node.chat:relay", "node.agents:read", "node.approvals:relay"],
  "cloud_scopes": ["cloud.llm:invoke", "cloud.research:invoke", "cloud.channels:send", "cloud.channels:receive"],
  "relay_url": "wss://relay.freeco.ai/v1/node/connect"
}
```

**B. The cloud shows a QR code** (desktop app and phone app). The web app
calls `POST /v1/pairing/offers` and shows a QR code for
`freeco://pair?c=7Q4M-2KXH&h=api.freeco.ai`, valid 5 minutes. The node scans
or pastes it and calls `POST /v1/node-pairing/claim` with the same signed
`registration`, whose payload also carries `"offer_code"` and the `claim`
URL as `aud`. The web page then shows the node's details and
fingerprint words for the user to confirm, exactly as in A, and the node polls
the same token endpoint.

**The node confirms which account it joined.** A code shown on the node's
screen could be read by someone else, who could approve it into *their*
account with their own passkey. So a successful token response does not
activate anything. The response also carries `"account_display": "f***@proton.me"`
and `"workspace_name": "Personal"`, and the node keeps the pairing in a
`pending_local` state, connects to nothing and grants nothing, until its own
owner confirms on the node: "This computer is joining **f***@proton.me**,
workspace **Personal**. Is that your account?" For a headless Podman node
the same question appears in its local dashboard, behind the owner's login,
and in `POST /api/node/cloud/pair/confirm` on the local API. The node then
sends a signed `POST /v1/node-pairing/confirm` (DPoP plus the node's
assertion), and only then does the registry mark the node `active`. If the
owner answers no, the node calls `DELETE /v1/nodes/self` and the attacker's
account is left with a revoked entry.

**What pairing can and cannot grant.** The grants in the token response are a
proposal. Once the owner has confirmed the account, a node accepts the
default safe set (`node.status:read`, `node.chat:relay`, `node.agents:read`,
`node.approvals:relay`) on its own. Anything wider waits until someone
confirms it on the node itself. Pairing alone can therefore never give the
cloud access to files, memory or tools, and those defaults themselves are
limited by the request context rules in section 6.7.

**What the registry stores:** account, workspace, node id, both public keys,
`node_info`, kind, reported key protection, status (`pending`, `pending_local`,
`active`, `revoked`),
created and last-seen times, coarse last location (country and city, kept 30
days), the cloud's view of the grants, the grants hash the node last reported,
and `rev_epoch`. It never receives a private key.

### 6.4 Pairing a phone with a home node, without the cloud

Fully sovereign users pair their phone straight to their node on the local
network. This is the existing `/api/pairing/*`, upgraded:

- `POST /api/pairing/request` (owner session) returns the token and
  `freeco://pair?token=…&fp=<node fingerprint>` as today.
- `POST /api/pairing/complete` becomes reachable without the API key, gated
  by the one-time token instead, and now requires `public_jwk` and a DPoP
  proof. It returns a **device certificate signed by the node's own key**
  (same claims as section 7.2, with the node as issuer).
- The phone then signs each request with DPoP, and the API middleware
  accepts `Authorization: DPoP <device certificate>` next to the API key and
  session cookie. Removing the device in the dashboard refuses its next
  request.

The node acts as its own small registry, so this works with no account at
all.

### 6.5 Node tokens after pairing

```http
POST /v1/nodes/token
DPoP: <proof>
Content-Type: application/x-www-form-urlencoded

grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer
&assertion=<JWT signed by the node key>
&scope=cloud.llm:invoke cloud.research:invoke
```

Assertion claims:
`{ "iss": "nod_k3f9q2…", "sub": "nod_k3f9q2…", "aud": "https://api.freeco.ai/v1/nodes/token", "iat": …, "exp": iat + 60, "jti": "…" }`.

The response is a 10-minute DPoP-bound access token. Its claims:

```json
{
  "iss": "https://api.freeco.ai",
  "sub": "nod_k3f9q2…",
  "aud": ["https://api.freeco.ai", "https://relay.freeco.ai", "https://mcp.freeco.ai"],
  "account": "usr_01H…",
  "workspace": "ws_01H…",
  "scope": "cloud.llm:invoke cloud.research:invoke",
  "plan": "green",
  "cnf": { "jkt": "0ZcOCORZNYy-DWpqq30jZyJGHTN0d2HglBV3uiguA4I" },
  "rev_epoch": 3,
  "iat": 1763235602,
  "exp": 1763236202
}
```

The scope is the intersection of what the node asked for, what the user
granted, and what the plan includes (section 9). A revoked node gets
`invalid_grant`.

### 6.6 The outbound connection

```http
GET /v1/node/connect HTTP/1.1
Host: relay.freeco.ai
Upgrade: websocket
Sec-WebSocket-Protocol: freeco-node.v1
Authorization: DPoP eyJ…(access token)
DPoP: eyJ…(proof with htm=GET, htu=https://relay.freeco.ai/v1/node/connect, ath=<hash of token>)
```

Once open, both sides bind the socket to the key, so a copied token cannot be
replayed on another connection:

```json
relay → node  { "t": "hello", "session": "rs_01JD…", "nonce": "Zq3…", "server_time": "2026-11-15T19:40:02Z", "trust_epoch": 7 }
node → relay  { "t": "hello_ack", "session": "rs_01JD…", "sig": "<node-key JWS over nonce|session|node_id>",
                "node_version": "0.9.9", "grants_hash": "sha256:4f1c…" }
```

- The node refreshes its access token in-band
  (`{ "t": "token", "access_token": "…" }`) before it expires. The relay
  closes the socket if it does not.
- `grants_hash` lets the cloud see when its view of the grants differs from
  the node's. The node's view wins, and the cloud updates its own.
- Ping every 25 s. Close codes: `4401` token expired, `4403` revoked, `4409`
  replaced by a newer connection from the same node, `4429` rate limited,
  `4503` relay restarting. The node reconnects with jittered backoff from 1 s
  to 60 s.
- Where a network blocks WebSockets, the node long-polls
  `POST https://relay.freeco.ai/v1/node/jobs:poll` with the same token and
  proof.

### 6.7 A request from the cloud to a node

The user opens `app.freeco.ai`, picks "Home computer" and writes to the
Concierge. The web backend calls `POST /v1/nodes/{node_id}/requests`. The
control plane checks the session, the user's role in the workspace, the plan,
that the node is active and online, and that the cloud-side grants allow the
operation. It then signs an envelope with the request key and the relay
forwards it:

```json
{ "t": "req", "jws": "eyJhbGciOiJFUzI1NiIsImtpZCI6InJlcS0yMDI2LTExIiwidHlwIjoiZnJlZWNvLXJlcStqd3QifQ.…" }
```

Envelope payload:

```json
{
  "id": "req_01JD8M…",
  "iss": "https://api.freeco.ai",
  "aud": "nod_k3f9q2…",
  "iat": 1763235602,
  "exp": 1763235662,
  "account": "usr_01H…",
  "workspace": "ws_01H…",
  "origin": { "kind": "web_session", "assurance": "a2", "session_ref": "s_9f…" },
  "scope": "node.chat:relay",
  "op": "agent.message",
  "params": { "agent": "concierge", "conversation": "c_7b…", "text": "Book a table for four at 8 tonight" },
  "risk": "low",
  "approval": null
}
```

`origin.kind` is one of `web_session`, `app`, `channel` (with `channel` and
the `chl_…` link id), `schedule` or `cloud_agent`. The node checks, in order:

1. The signature, against the request key in the trust document.
2. `aud` is this node, `exp` has not passed (±5 minutes of clock skew), and
   `id` has not been seen in the last 10 minutes.
3. `scope` is in the node's **local** grants, and `op` is one that this scope
   covers.
4. Its local policy for `op`: allow, ask for approval, or deny.
   `origin.assurance` counts here (section 8.2).

**The grants travel with the request.** Passing the outer check only starts
the work. The node attaches a request context to the run: the origin, its
assurance, and the effective grants (cloud grant ∩ node grant ∩ plan). That
context follows the run through the agent loop, every tool call, memory
access and `agent_delegate` or `agent_send` hand-off, and each of those is
checked against agent capabilities ∩ node policy ∩ the request context. So a
chat relayed with only the default grants can make the Concierge answer,
research through cloud scopes, and work in its own agent workspace and the
user's own conversation, but it cannot read files outside the workspace, use
memory beyond that conversation, open the local browser or run anything the
grants do not name, whatever the message asks for. The check lives where tool
calls are already gated (`freeco-kernel-runtime/src/tool_runner.rs`) and
next to the existing capability checks.

**The node decides the risk.** The envelope's `risk` is a hint for display.
The node classifies every operation and tool call itself (section 8.2) and
uses the higher of its own class and the hint, so a request labelled `low`
cannot carry a payment.

Then it answers on the same socket. Every frame the node sends is a compact
JWS signed by the node key, so the relay can forward but not change it:

```json
{ "t": "res", "jws": "eyJ0eXAiOiJmcmVlY28tbm9kZS1mcmFtZStqd3QiLCJhbGciOiJFUzI1NiJ9.…" }
```

Each payload binds the frame type, the relay session, the request id and a
sequence number that counts up from 1 per request. The control plane, not
the relay, verifies the signature against the node's registered key and
refuses gaps, repeats or a frame from another session before it consumes or
shows anything. Decoded, a reply looks like this:

```json
{ "t": "res",    "session": "rs_01JD…", "id": "req_01JD8M…", "seq": 1, "status": "accepted" }
{ "t": "stream", "session": "rs_01JD…", "id": "req_01JD8M…", "seq": 2, "delta": "I found three places near the lake…" }
{ "t": "res",    "session": "rs_01JD…", "id": "req_01JD8M…", "seq": 3, "status": "done", "result": { "conversation": "c_7b…" } }
```

The same applies to `event` and `approval_request` frames, which carry their
own id and sequence.

`status` is `accepted`, `done`, `needs_approval` (with an `approval_id`),
`denied` (with a reason: `not_granted`, `policy`, `assurance_too_low`,
`expired`, `replay`, `bad_signature`) or `error`. Every request and its
outcome go into the node's audit chain as `CloudRequest`.

If the node is offline, the cloud says so and offers what it can do without
the node. It does not pretend the node answered.

### 6.8 Approvals through the cloud

When a request needs approval, the node sends:

```json
{ "t": "approval_request", "approval_id": "a6f1…", "req": "req_01JD8M…", "risk": "high",
  "summary": "Pay a CHF 240 deposit to Restaurant du Lac",
  "challenge": "<base64url SHA-256 of the canonical approval>",
  "expires_at": "2026-11-15T23:40:02Z", "accept": ["passkey_fresh", "approver_key"] }
```

The cloud delivers it to the user's phone app, web app or channel, according
to the user's settings, and returns the decision. The decision is signed by
the control plane with the request key, exactly like a request envelope, so
the relay, which does not hold that key, cannot fabricate or alter one:

```json
{ "t": "approval_decision", "jws": "eyJhbGciOiJFUzI1NiIsImtpZCI6InJlcS0yMDI2LTExIiwidHlwIjoiZnJlZWNvLWFwcHJvdmFsK2p3dCJ9.…" }
```

Decoded payload:

```json
{
  "iss": "https://api.freeco.ai",
  "aud": "nod_k3f9q2…",
  "approval_id": "a6f1…",
  "req": "req_01JD8M…",
  "challenge": "<the challenge from the approval request>",
  "decision": "approve",
  "approver": { "account": "usr_01H…", "assurance": "a3", "auth_time": "2026-11-15T19:41:10Z" },
  "iat": 1763236870,
  "exp": 1763236930,
  "jti": "d7c2…"
}
```

The node verifies the signature, that `aud`, `approval_id`, `req` and
`challenge` match a request it is still waiting on, that the approver is the
account the node belongs to with enough assurance for the risk, and that
`exp` has not passed, and it consumes the decision exactly once.

Until Phase F, this means the control plane vouches for a fresh passkey
sign-in. Phase F adds **end-to-end signed approvals**: the phone app's hardware key (or
a passkey registered by the web app) signs the `challenge`, and the node
checks that signature against an approver list it keeps itself. After that,
not even a compromised cloud can approve a payment on the user's behalf.

### 6.9 Changing permissions

```http
GET  /v1/devices/{node_id}/grants
PUT  /v1/devices/{node_id}/grants      { "add": ["node.files:read:~/Documents/FreEco/**"], "remove": [] }
```

- **Narrowing** (removing a grant) is safe in every direction, so it is the
  one grant change the cloud may make effective on its own: it applies at once
  in the cloud, and the node applies it at once when it arrives in a signed
  `{ "t": "grants_update", "remove": [ … ] }`.
- **Widening** (adding a grant) is only ever a *proposal* from the cloud:
  `{ "t": "grants_update", "proposal": "gp_01…", "add": [ … ], "by": "usr_01H…" }`.
  It is a Critical operation (section 8.2), so it becomes effective only when
  someone confirms it on the node's own dashboard. That holds in Phase F too:
  approver signatures reach High, not Critical. Until then the proposal shows
  as waiting on the device page.
- The effective permission for any request is **cloud grant ∩ node grant ∩
  plan entitlement**. A compromised cloud that widens its own grants gains
  nothing the node did not agree to.

### 6.10 Revocation

| Trigger | Endpoint | What happens |
|---|---|---|
| User removes a device | `POST /v1/devices/{node_id}/revoke` with a passkey from the last 10 minutes | Status becomes `revoked`, `rev_epoch` goes up, the relay closes the socket with `4403` within 5 s, token minting refuses, pending requests are cancelled, the node certificate goes on the revocation list, channels routed to it fall back to the user's other choice, and every other device and the user's email get "Work laptop was removed by you at 21:04" |
| Node unpairs itself | `DELETE /v1/nodes/self` (DPoP) | The same, with reason `self` |
| Lost or stolen device | Same as removal | Local data stays encrypted at rest with the device-bound key. A remote wipe request is delivered if the device ever reconnects. The device page says plainly that this is best effort |
| Sign out everywhere | `POST /v1/account/sessions:revoke-all` | Ends web and app sessions and refresh tokens. **Nodes stay paired**: they are devices, not sessions. "Remove all devices" is a separate button |
| A cloud signing key leaks | New trust document signed by the root key | Nodes drop the old `kid` on their next daily fetch, or at once when the relay pushes `{ "t": "trust_update" }` |

Other nodes and the phone app check certificates offline against
`GET /v1/nodes/revocations`, a registry-signed list with `next_update` one hour
ahead.

---

## 7. Message formats in one place

### 7.1 DPoP proof (RFC 9449)

Header `{ "typ": "dpop+jwt", "alg": "ES256" | "EdDSA", "jwk": { …public key… } }`,
claims `{ "jti", "htm", "htu", "iat", "ath"?, "nonce"? }`. `ath` is present
when a token is sent with the proof. `nonce` is present when the server asked
for one with `DPoP-Nonce`.

### 7.2 Node certificate

Header `{ "typ": "freeco-node-cert+jwt", "alg": "ES256", "kid": "reg-2026-11" }`:

```json
{
  "iss": "https://api.freeco.ai",
  "sub": "nod_k3f9q2…",
  "account": "usr_01H…",
  "workspace": "ws_01H…",
  "kind": "personal_node",
  "cnf": { "jwk": { "kty": "EC", "crv": "P-256", "x": "…", "y": "…" } },
  "enc": { "kty": "OKP", "crv": "X25519", "x": "…" },
  "key_protection": "hardware",
  "rev_epoch": 3,
  "iat": 1763235602,
  "nbf": 1763235602,
  "exp": 1765827602
}
```

`kind` is `personal_node` (runs agents), `client` (phone or desktop app with
no agents) or `hosted_node` (a cell FreEco runs). On receipt, the node checks
that `cnf.jwk` and `enc` are its own keys and refuses a certificate that names
anything else. The node renews it with
`POST /v1/nodes/self/certificate` (DPoP plus assertion) from day 15.

### 7.3 Relay frames

| `t` | Direction | Purpose |
|---|---|---|
| `hello`, `hello_ack` | both | Bind the socket to the node key (6.6) |
| `token` | node → relay | In-band access token refresh |
| `req` | relay → node | Request envelope signed with the request key (6.7) |
| `res`, `stream` | node → relay | Outcome and streamed output, node-signed with session, id and sequence (6.7) |
| `event` | node → relay | Unprompted output (a finished schedule, a proactive message), node-signed |
| `approval_request` | node → relay | Node-signed (6.8) |
| `approval_decision` | relay → node | Signed with the request key and consumed once (6.8) |
| `grants_update` | relay → node | Signed with the request key; removals apply, additions are proposals (6.9) |
| `trust_update` | relay → node | Fetch the trust document now |
| `revoked` | relay → node | Sent just before close code `4403` |
| `sealed` | relay → node | A queued message, HPKE-sealed to the node's encryption key (section 10) |

### 7.4 Channel link record

```json
{
  "id": "chl_01H…",
  "account_id": "usr_01H…",
  "channel": "whatsapp",
  "provider_user_id": "enc:v1:Qm9…",
  "address": "enc:v1:x7T…",
  "lookup_hash": "hmac-sha256:9c0e…",
  "verified_by": "reverse_code",
  "assurance": "a1",
  "linked_at": "2026-11-15T19:45:12Z",
  "route": "auto"
}
```

Both `provider_user_id` (for WhatsApp, today the number's digits) and
`address` are encrypted at rest; the example shows them as stored. Inbound
messages are matched by `lookup_hash`, a keyed hash (HMAC with a key kept
outside the database) of the channel name and the normalised
`provider_user_id`, so a database dump lists no phone numbers. `route` is `node`,
`cloud` or `auto`. Keep `provider_user_id` separate from the phone number,
because Meta has announced WhatsApp usernames, and the number may not always
be delivered; check the Cloud API documentation when Phase D starts.

---

## 8. WhatsApp and other channel identities

### 8.1 Linking and unlinking

| Situation | Flow |
|---|---|
| WhatsApp first (host QR) | Section 6.2: provisional account at `a1`, upgraded by passkey plus reverse code |
| Web first, add WhatsApp | "Connect WhatsApp" (`POST /v1/channels/links { "channel": "whatsapp" }`, passkey within 10 minutes) shows `FRE 482193` and a `wa.me` button. When the code arrives from a number, that number is linked |
| Telegram | `t.me/<bot>?start=<one-time token>` from the web app, as planned in section 1C of the production plan |
| Email | Verified by link; inbound mail must pass SPF, DKIM and DMARC alignment before it counts as `a1` |
| New number | Link the new one while signed in with a passkey, then unlink the old one (`DELETE /v1/channels/links/{id}`) |
| Lost phone or SIM swap | Whoever now holds the old number stays at `a1` at most, so nothing above Medium. The user unlinks it from the web app or another device |
| Groups | A message in a group counts as its sender's own channel identity. The agent answers only in groups the account owner allowed (the allowlist from branch `claude/project-thread-antq0c`). A guest in the group cannot approve the owner's actions |
| Self-hosted node with its own WhatsApp (Baileys) | Identity stays local: `WHATSAPP_ALLOWED_NUMBERS` becomes `[channels.whatsapp] allowed_numbers` in the node policy. The cloud is not involved. Baileys stays self-hosted only, because it breaks WhatsApp's terms |

### 8.2 Assurance levels

Each request carries the assurance of whoever started it, and the node's
policy maps it onto the existing `RiskLevel`:

| Level | Meaning | Highest risk it can approve |
|---|---|---|
| `a0` | Unknown address, no link, no invite | Nothing; it gets the introduction only |
| `a1` | Verified channel link (code sent from that number, or account created from it) | Low on its own. Medium with a preview and a reply code ("Reply YES 4821 to send") |
| `a2` | Signed-in web or app session | Medium |
| `a3` | Passkey used in the last 10 minutes, or (Phase F) a signature from an approver key the node trusts | High |
| on the node | Someone at the node's own dashboard | Critical |

Risk classes, so agents and the policy speak the same language:

| `RiskLevel` | Examples |
|---|---|
| Low | Answering, research, reading the user's own conversation |
| Medium | Sending a message to a known contact, a calendar entry, a schedule, a booking without payment |
| High | Anything that spends money, uses a stored credential, shares personal data with a new party, or writes files outside the agent workspace |
| Critical | Widening grants, changing the approver list or node policy; exporting all data; deleting data. Narrowing a grant is not Critical: it only takes access away, and the cloud may do it (section 6.9) |

---

## 9. Scopes

Names are `area.resource:action`, with an optional qualifier after a further
colon. There are three families, and each is enforced by a different party.

### 9.1 Human-facing cloud scopes (enforced by the cloud)

| Scope | Allows |
|---|---|
| `openid profile email` | OIDC basics |
| `account:read`, `account:write` | Own profile, terms acceptance, export and delete requests |
| `workspace:read`, `workspace:admin` | Workspace details; members and roles (Owner and Admin only) |
| `devices:read`, `devices:manage` | Device page; pair, rename, change grants, revoke (manage needs `a3`) |
| `channels:link` | Link and unlink channels (`a3`) |
| `nodes:invoke` | Send requests to the user's own nodes through the relay |
| `billing:read`, `billing:manage` | Plan and invoices; changes |
| `support:grant` | Give support time-limited access, logged in the audit chain |

### 9.2 What a node may use in the cloud (enforced by the cloud, capped by plan)

| Scope | Plan |
|---|---|
| `cloud.llm:invoke:standard` (Nemotron and the base model pool) | Member and up |
| `cloud.llm:invoke:top` (top-tier models) | Executive, Elite |
| `cloud.research:invoke` (shopping research, places, web search; platform keys stay server-side) | Member and up |
| `cloud.channels:send`, `cloud.channels:receive` (FreEco's shared numbers) | Member and up |
| `cloud.calls:place` (calls to businesses, within the plan's minutes) | Green and up |
| `cloud.agents:invoke` (CEO, team and business agents in the user's cloud cell) | Business, Enterprise |
| `cloud.backup:write`, `cloud.backup:read` (encrypted snapshots only) | Member and up |
| `cloud.push:send` (notify the user's phone) | Member and up |
| `cloud.registry:read` (certificates of the user's other nodes) | All |

Paid features are enforced here, on the server, never inside the node binary.
That matches rule 4 of the production plan's section 4.

### 9.3 What the cloud may ask of a node (enforced by the node)

| Grant | Default | Note |
|---|---|---|
| `node.status:read` | on | Online, version, health |
| `node.chat:relay` | on | The user's own messages from the web, app or linked channels to the node's agents, and the replies |
| `node.agents:read` | on | List agents and conversations |
| `node.approvals:relay` | on | Deliver approval requests to the user and bring decisions back |
| `node.memory:read`, `node.memory:write` | off | |
| `node.files:read:<glob>`, `node.files:write:<glob>` | off | Always limited to a path pattern |
| `node.tools:invoke:<tool>` | off | For example `browser`, to use the user's own logged-in browser with approval each time |
| `node.schedule:write` | off | Create schedules on the node |
| `node.llm:serve` | off | Lend the node's local model to a cloud task |
| `node.backup:export` | off | Push encrypted snapshots; offered once a Recovery Kit exists |

**Never grantable remotely:** reading vault secrets, `shell_exec`, changing the
approver list, changing node policy, widening grants, and turning off
approvals. These exist only on the node's own dashboard. Narrowing grants is
the one change the cloud can make on its own (section 6.9).

**Presets** keep the choice simple on the device page: "Concierge only" (the
four defaults), "Assistant with documents" (adds reading one documents folder)
and "Full hybrid" (adds memory and the browser tool, with approval on every
use).

---

## 10. Offline operation

1. **A node works without the cloud.** Its agents, memory, local models,
   local channels (its own Telegram bot, Signal or self-hosted WhatsApp) and
   dashboard approvals need neither the account nor the network. Losing the
   cloud account does not stop a working node.
2. **Cloud services pause.** Calls to the LLM, research and phone gateways
   fail with a clear message. The node falls back to a local model or the
   user's own provider key in the vault, if set.
3. **No licence check for local features.** Paid features live in the cloud
   (section 9.2), so the node has nothing to check while offline.
4. **Certificates while offline.** A node certificate expires after 30 days,
   but that only matters for reconnecting. On return, the registry renews an
   expired certificate up to 90 days past expiry if the node key signs and the
   node is not revoked. After that, the user re-pairs, and it takes one code.
5. **Messages for an offline node.** If the user chose `route = "node"`, the
   cloud seals each message to the node's encryption key (HPKE) and queues it
   for up to 7 days, then delivers it as `sealed` frames and deletes it. With
   `auto`, a hosted cell answers instead where the plan includes one.
6. **Be honest about the shared numbers.** A WhatsApp or Telegram message to
   FreEco's shared number passes through the cloud in plain text, because
   Meta and Telegram deliver it that way. The cloud does not store it beyond
   delivery. Users who want the cloud never to see content connect their own
   channel to their node.
7. **Revocation while offline.** The cloud refuses a revoked node at once.
   The node itself learns on reconnect, and stops using cloud scopes then.
8. **Clocks.** Tokens and envelopes depend on time. The `hello` frame carries
   server time; a node more than 5 minutes off warns in its dashboard and
   uses the server offset for its own proofs.
9. **A stale trust document stops cloud requests, not the node.** If the node
   has not fetched a trust document newer than its stored one before that
   document's `expires_at` (section 3.1), it refuses cloud requests and says
   why in its dashboard, and keeps every local feature running.

---

## 11. Deployment modes

| | Cloud only | Hybrid | Fully sovereign |
|---|---|---|---|
| Who runs the node | FreEco.ai (a hosted cell, labelled "Hosted by FreEco.ai in Geneva") | The user (home computer, laptop, Podman server), optionally plus a hosted cell | The user |
| Registry | FreEco.ai | FreEco.ai | The node itself (section 6.4), or the organisation's own `freeco-cloud` |
| Human login | Zitadel | Zitadel | Local password or passkey; an organisation's own OIDC provider |
| Who can read the data | FreEco.ai's operations can, technically: the cell vault key is wrapped by the control plane master key (production plan, section 1B), and the cell's backup key is wrapped by that vault key, so FreEco can also restore a hosted cell's backups without the Recovery Kit. That is also what lets FreEco restore a cell after a server failure. Say so on the device page | Data on the home node: only the user. Data in a hosted cell: as in cloud only | Only the user |
| Channels | FreEco's shared numbers | Shared numbers routed to the node, or the node's own | The node's own |

The protocol is the same in every column. Moving from cloud only to hybrid is
"Add this computer"; moving to sovereign is unpairing and keeping the node.

---

## 12. Recovery

**Account recovery** (getting back into the cloud account):

- Encourage two passkeys from the start (phone and laptop, or a hardware key).
- Ten single-use recovery codes, shown once.
- Email-only recovery waits **72 hours**, with notices to every device and
  linked channel, and can be cancelled from any of them. That stops a quick
  takeover through a hacked mailbox.
- A WhatsApp number is never a recovery factor on its own, because of SIM
  swaps. It can send the cancel notice.
- **Recovery quarantine:** after an account recovery, every node drops to
  `node.status:read` until it is confirmed on the node itself. Recovering the
  account never hands over a home node.

**Node recovery** (a new or replacement device):

- Pair the new device normally (section 6.3) and revoke the old one.
- If the old node still runs, it can hand over to the new one: the old node
  encrypts its data to the new node's encryption key, after approval on the
  old node.

**Data recovery** (the old device is gone). These guarantees are for nodes
the user runs; hosted cells follow section 11.

- Backups a personal node uploads to the cloud are ciphertext only. The
  backup key is wrapped by the node's vault key and by the Recovery Key, and
  the cloud stores only those wrapped copies.
- With the Recovery Kit and a newly paired node, the user restores
  everything. Without both the old node and the Recovery Kit, the data cannot
  be recovered, by anyone. The app says this in plain words when it shows the
  kit: "FreEco.ai cannot reset this. That is what keeps it yours."
- A hosted cell's backups can be restored by FreEco.ai, because the control
  plane can unwrap the cell's keys. A later option for users who want a hosted
  cell without that: wrap the cell's backup key to the Recovery Key only, at
  the cost that FreEco can no longer restore the cell for them.
- Later (Phase F), a 2-of-3 split of the Recovery Key between the paper kit, a
  trusted person and a safe place suits executive users who want no single
  point of loss.

---

## 13. How to build it

Each phase is useful on its own, and each ends with a live test as CLAUDE.md
requires, not only unit tests.

**Before Phase A:** merge the WhatsApp allowlist and one-conversation-per-contact
work from branch `claude/project-thread-antq0c`, and the parallel-conversation
work in PR #36. The identity work assumes one conversation per person, which
is Phase 0.3 of the production plan.

| Phase | What | Where in the code | Done when | Size |
|---|---|---|---|---|
| **A. Node identity on the node** | Node key on first boot (hardware where available); `GET /api/node/identity` (id, fingerprint words, key protection); key-based local pairing (6.4) with device certificates; DPoP accepted by the API middleware; audit actions; new columns on `paired_devices` through a migration | New `freeco-kernel/src/node_identity.rs`; `pairing.rs`; `routes.rs` pairing handlers; `middleware.rs`; `freeco-memory/src/migration.rs`; `freeco-desktop` pairing screen | A phone pairs with X2's node over the LAN, its signed request works, and after removal the same request is refused (curl test). `cargo test` and clippy green | 1 to 2 weeks |
| **B. Accounts and the node registry** | New `freeco-cloud` crate (axum): OIDC with Zitadel through a backend-for-frontend, passkeys, accounts and workspaces, terms records; registry tables; pairing start, lookup, approve and token; node token endpoint; trust document with the offline root key; device page with list, rename and revoke; role checks on every API route (Owner, Admin, User, Kid, Viewer from `tenancy.rs`), in the cloud and in the node's API | New `crates/freeco-cloud`; the desktop app's sign-in reuses the loopback PKCE flow in `freeco-extensions/src/oauth.rs` (RFC 8252), while the web app keeps tokens server-side; shared types in a new small `freeco-identity` crate (ids, JWK thumbprint, DPoP create and verify, node certificate, envelope, scopes) used by both node and cloud | The X2 node pairs with a staging account by typed code and stays inactive until its owner confirms the account on the node; it shows on the device page; a registration with a changed `enc_key` is refused; revoking it makes the next token request fail; a Viewer session cannot call an Owner route | 3 to 4 weeks |
| **C. Relay and local policy** | WebSocket relay with `hello` binding; signed envelopes; the node checks grants and policy; approvals round trip; web chat to the home Concierge; sealed queue; long-poll fallback; hosted cells connect the same way | `freeco-cloud` relay module; a node-link handler in `freeco-kernel` that calls the kernel directly (never through `127.0.0.1:4200`); `approval.rs` | From `app.freeco.ai`, chat with the Concierge on X2 behind NAT with no open port. A request the cloud allows but the node did not grant is refused by the node, and so is a tool call a relayed chat asks for outside its grants. A frame or approval decision altered in transit is refused. Revocation closes the socket within 5 s | 3 to 4 weeks |
| **D. WhatsApp identity** | Cloud API webhook with signature check; channel links table; host invite codes; provisional accounts; claim link with reverse code; assurance levels enforced; routing to node or cell | `freeco-cloud` channel gateway; `freeco-channels/src/whatsapp.rs` (wire up the allowlist); `freeco-kernel/src/auth.rs` (bindings from links, with assurance) | A test guest scans a host QR code, chats, and claims the account with a passkey; a payment-class approval cannot be given from WhatsApp alone | 2 to 3 weeks, after Meta confirms eligibility |
| **E. Recovery** | Several passkeys; recovery codes; 72-hour email recovery; recovery quarantine; Recovery Kit and encrypted cloud backups; node replacement and restore | `freeco-cloud`; `freeco-api/src/backup.rs` (encrypt snapshots with the backup key) | A node backup restores on a new machine with only the Recovery Kit; after an account recovery, nodes stay at status-only until confirmed on the node | 2 to 3 weeks |
| **F. Hardening** | End-to-end signed approvals and approver lists; X.509 node certificates and mTLS for enterprise relays; OFP between a user's nodes with node certificates instead of the shared secret; TPM and App Attest as extra trust signals; a self-hosted registry for sovereign organisations; 2-of-3 recovery | `freeco-wire/src/peer.rs`; `freeco-identity`; `freeco-cloud` | Each item with its own live test | Later, after paid launch |

A to C are the critical path, about 7 to 10 weeks in total. D can run beside
C once B is done. E must be done before paid launch.

**Where this sits in the development roadmap** (`/mnt/project-files/plans/dev-roadmap-2026-11.md`):
all six phases are its Phase 2, after its Phase 1 (one conversation per
person, WhatsApp closed by default). A is new there: it needs no cloud and can
start as soon as Phase 1 lands. B covers roadmap steps 2.1 (accounts and
passkeys), 2.2, 2.3, 2.4 and 2.6; C is 2.5; D is the channel part of 2.1; E is
2.7; F follows Phase 4. Hosted cells reuse C, as the roadmap recommends.

**Not touched:** `freeco-cli`, which Free is building by hand. Headless
pairing runs from the dashboard and `POST /api/node/cloud/pair` on the local
API until Free decides whether the CLI should get a `pair` command.

**Libraries to choose in Phase A:** `p256` and the existing `ed25519-dalek`
for keys; a JOSE library with EdDSA and ES256 (`jsonwebtoken` or `josekit`);
`windows` CNG bindings for the TPM key; `security-framework` for the Secure
Enclave; `hpke` for sealed messages. All are Rust; pick by maintenance and
audit status at the time.

---

## 14. What could go wrong, and how each is handled

| Risk | Handling |
|---|---|
| **Device-code phishing:** someone sends the user a code from the attacker's node | The user types the code rather than clicking a link; the page shows the node's name, platform, approximate location and fingerprint words, and asks the user to compare them with their own screen; a fresh passkey is required |
| **A code read off someone else's screen:** an attacker approves the victim's node into the attacker's account | The node activates nothing until its own owner confirms on the node which account and workspace it joined (section 6.3); answering no revokes the entry |
| A node access token leaks | It is DPoP-bound and lasts 10 minutes, so it is useless without the node key |
| A node key is stolen from a software keystore | Revoke on the device page; prefer hardware keys; the device page shows key protection so the user can see which devices are weaker |
| The relay or edge is compromised | Envelopes and approval decisions are signed with the request key, which the relay does not hold; every node frame is signed by the node and checked by the control plane with session, id and sequence; the registration payload is signed by the node key; trust documents carry a serial that only goes up; the node's grants still apply |
| The control plane is compromised | It cannot widen node grants without local confirmation and cannot read a personal node's Recovery-Key-protected backups; from Phase F it cannot forge approvals either. Node policy remains the last word. Hosted cells are exposed, as section 11 says, which is why they are labelled |
| SIM swap or a stolen phone | Channel identity tops out at `a1`; payments, credentials and node changes need a passkey |
| Prompt injection asks the node to act | The envelope's `origin` and the agent's taint tracking both reach the approval, which shows where the instruction came from |
| Replay of a request or proof | `jti` and request id caches (the existing `NonceTracker`), short `exp`, socket binding in `hello_ack` |
| Bots open fake WhatsApp accounts to burn model budget | Invite codes from hosts, an introduction only for unknown numbers, one account per verified number, and per-account caps at the LLM gateway |
| The user loses everything | Two passkeys and recovery codes for the account; the Recovery Kit for data; clear words up front that the data cannot be recovered without it |
| Hosted cells are less private than home nodes | Labelled on the device page, with a one-tap path to add a home node |

---

## 15. Decisions for Free

| # | Decision | Recommendation |
|---|---|---|
| 1 | Pairing entry point | The node shows a code and the user types it in the app, as the default (works headless, follows RFC 8628); a QR code from the cloud for the desktop and phone apps |
| 2 | Node authentication | DPoP as the standard (works through any edge proxy); mTLS only as an enterprise option |
| 3 | Who issues node credentials | FreEco.ai's own node registry; Zitadel for humans only |
| 4 | WhatsApp-first accounts | Yes, through host invite codes, limited to `a1` until a passkey is added; unknown numbers get the introduction only |
| 5 | Hosted cells | Treat them as nodes, labelled "Hosted by FreEco.ai", so one protocol serves every mode |
| 6 | Email-only account recovery | Allowed, with a 72-hour wait that any device can cancel |
| 7 | Who holds the offline root key | Two hardware security keys held by two named people in separate places |

---

## 16. Standards used

- OAuth 2.0 Security Best Current Practice, RFC 9700
- PKCE, RFC 7636; OAuth for native apps, RFC 8252
- OAuth Device Authorization Grant, RFC 8628
- DPoP, RFC 9449; mutual-TLS-bound tokens, RFC 8705 (option)
- JWT bearer assertions, RFC 7523; JWK thumbprint, RFC 7638; EdDSA in JOSE, RFC 8037
- OpenID Connect Core 1.0; WebAuthn Level 3 (passkeys)
- HPKE, RFC 9180
- OAuth 2.0 Protected Resource Metadata, RFC 9728 (for standard MCP clients at the MCP gateway)
