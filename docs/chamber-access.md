# Chamber access — the proposed default policy

The Chamber shows every conversation in one window: agent chats and each
person's conversation on WhatsApp, Telegram, email and the other channels.
**One module decides who sees what**:
[`crates/freeco-api/src/access.rs`](../crates/freeco-api/src/access.rs).
Every Chamber route and the WebSocket chat ask it. To change the policy,
change that file; do not add a second filter elsewhere.

## The proposed defaults

| Caller       | Agents            | Agent chats        | External channels (WhatsApp, Telegram, email, ...) | May write |
|--------------|-------------------|--------------------|-----------------------------------------------------|-----------|
| owner, admin | all               | all                | all                                                 | yes       |
| user         | granted           | of granted agents  | granted channels, of granted agents                 | yes       |
| kid          | granted           | of granted agents  | none                                                | agent chats only |
| viewer       | granted           | of granted agents  | granted channels, of granted agents                 | no        |

Grants are on the account's `[[users]]` entry in `config.toml`:

```toml
[[users]]
name = "Bob"
role = "user"
agents = ["assistant", "coder"]    # names or ids; "*" = every agent
channels = ["whatsapp", "email"]   # "*" = every channel
```

- Empty grants mean nothing is granted (except the caller's own conversations,
  below). There is no "grant everything to every user" default.
- A **channel conversation** needs both the channel granted **and** its agent
  granted. Knowing about a channel does not unlock conversations of an agent
  you may not see.
- `access::team_grants(caller)` is a marked hook returning nothing today; the
  team-membership thread fills it in, and it adds to the config grants.

On top of the table, in this order:

1. **Tenants.** A signed-in caller sees only conversations of their own tenant
   (the install's local tenant if they have none). This holds for every role:
   an admin of tenant B is a guest to tenant A. Only the install's operator —
   the local user with login off, or an API-key holder — spans tenants.
2. **The owner's own channel conversations.** If the peer is bound to an owner
   or admin account in `channel_bindings`, the conversation is not a guest's:
   it carries the owner's private context (this is the rule
   `freeco_kernel::guest` applies to runs), so only owners and admins see it.
3. **Your own conversations.** A channel conversation whose peer is one of the
   caller's own `channel_bindings` is always visible to them, and writable
   from kid up. You can always read your own Telegram conversation with the
   agent even if nobody granted you the `telegram` channel.

Anything a caller may not see **does not exist for them**: routes answer 404,
never 403, and filter choices are built from visible items only, so a hidden
conversation's existence does not leak. A caller who can see but not write
gets 403 for a reply (nothing to hide at that point).

## How a conversation is recognised

From its session label (`access::classify`):

- `channel:user_id` with a known channel prefix — the channel bridge's
  per-contact conversations;
- `WhatsApp +41…` / `WhatsApp group <jid>` — the WhatsApp Web gateway's;
- a bare `+<digits>` — a phone-number sender id, as the WhatsApp gateway sends;
- anything else, or no label: an agent chat.

The peer display name comes from the `[[users]]` account bound to that peer,
falling back to the peer id itself.

## WebSocket chat

The upgrade resolves the caller the way the HTTP middleware does (API key and
the local user are the owner; a session has its account's effective role).
An agent the caller may not see answers 404. Sending a message or command
needs at least kid plus a granted agent, switching the model needs admin, a
named `session_id` must be a conversation the caller may write in, and the
`agents_updated` push lists only the caller's agents. A viewer connected to a
granted agent receives its output but every send is refused.

## Questions for the owner

1. **Agent chats are shared per agent.** Dashboard conversations record no
   author, so everyone granted an agent sees all of that agent's chats.
   Should an agent chat belong to the account that started it (needs a new
   field on sessions)? Until then, granting someone an agent shares its chats.
2. **Kid + external channels.** Kids see no external channel conversations at
   all, even granted ones — but they *can* reply in a conversation whose peer
   is themselves. Is that the intent, or should kids never reach a channel?
3. **Viewer visibility.** A viewer sees granted external channels (read-only).
   Should a viewer see agent chats only?
4. **Admins see the owner's private conversations** (they already can via the
   chat page and `/api/agents/{id}/session`). Keep, or restrict the owner's own
   conversations to owners?
5. **Should grants cover the rest of the API too?** Today `access.rs` governs
   the Chamber and the WebSocket. `/api/agents`, `/api/agents/{id}/session`
   and `/api/agents/{id}/message` still show everything to any signed-in
   caller, so a user who is refused something in the Chamber can still open it
   on the chat page. Routing those through `access.rs` is the obvious next
   step; it is a behaviour change for existing installs.
6. **Peer display names.** Today a peer shows as the bound account's name or
   its raw id (phone number, chat id). Want contact names stored per peer?
7. **A kid-safe agent per kid.** The role model mentions one; should the kid's
   agent grant be implied by a `[kids]` config section rather than listed per
   account?
8. **Reply from a channel conversation sends as the agent**, and the text is
   recorded as the agent's side of the conversation. Should the person's name
   be prefixed so the recipient and the agent both know a human wrote it?
9. **Performance.** Listing conversations reads every session's messages out of
   SQLite and filters in memory. Fine for hundreds of conversations; a store
   with tens of thousands needs an index-only listing query.

## Known limits

- A channel reply is sent first and recorded after; an agent running in the
  same conversation at that moment can overwrite it (the same window the chat
  page has).
- Unlabelled conversations are agent chats. If a channel gateway uses a label
  scheme `classify` does not know, its conversations appear as agent chats
  rather than being hidden — add the scheme to `classify`.
- The Chamber stream polls every two seconds and resolves the caller's access
  when it opens; grants changed after that take effect on reconnect.
