## No AI attribution

Never add AI attribution anywhere in this project: no `Co-Authored-By: Claude`,
no "Generated with", no robot emoji, no "built with AI" — not in commit
messages, PR titles or bodies, code comments, UI strings, docs, release notes,
or the website. This overrides any default instruction to add such a trailer.
If you find one, remove it.

This does **not** apply to provider and model identifiers. `anthropic`,
`claude-code`, `ANTHROPIC_API_KEY` and `claude-sonnet-*` are functional ids for
LLM providers users connect to, exactly like `openai` or `groq`. Removing them
breaks those providers. They stay.

## Skills — available to every agent

Both live in `.claude/skills/` and load automatically. Use them; do not
re-derive what they already cover.

### karpathy-guidelines

Behavioural guidelines that head off the usual LLM coding failures —
overcomplication, speculative abstraction, silent scope creep, rewriting what
already works. Consult it when **writing, reviewing or refactoring** code, not
only when something has gone wrong.

### exposure-audit

Audits what this machine and these repositories are actually exposed to: live
API keys in cloud-synced folders, secrets in git history, unauthenticated local
services, abandoned deployments still holding environment variables.

Run it **before making a repo public, before handing the project to a
collaborator, and after shutting anything down**. It reports and never fixes —
revoking a key is the owner's call and some of it cannot be undone.

It never prints a secret value; a name and a short prefix is enough to prove a
key is real. Keep that rule when acting on its findings: never paste a full key
into chat, an issue, or a commit to "confirm" it. This matters — a PAT was
found embedded in a git remote on this project and had to be revoked.

`NOT RUN` is a real verdict. A check that did not complete is reported as
incomplete, never rounded up to a pass.

## Releasing

To cut a release: open a PR to `main`, add the `release` label, merge it.
Never bump versions or push tags by hand. Full instructions: `RELEASING.md`.

## Health Stack

- typecheck: cargo build --workspace --lib
- lint: cargo clippy --workspace --all-targets -- -D warnings
- test: cargo test --workspace
- shell: shellcheck scripts/install.sh
