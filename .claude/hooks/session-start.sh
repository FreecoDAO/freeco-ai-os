#!/bin/bash
# Installs the vetted design skills into Claude Code cloud sessions.
#
# Each skill is fetched at a pinned upstream commit, so a change upstream
# cannot reach a session until someone reviews it and moves the pin here.
# Nothing fetched is executed: files are copied, tests and bytecode dropped.
# The skills are installed into the user's skills folder rather than committed,
# because apple-design carries Apple's HIG text and has no licence to
# redistribute it. To update a skill, review the upstream diff since its pin,
# then move the pin.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

skills_dir="$HOME/.claude/skills"
hashes="$skills_dir/.freeco-design-skills.sha256"
script="${CLAUDE_PROJECT_DIR:-.}/.claude/hooks/session-start.sh"

mkdir -p "$skills_dir"

# Fetch one commit of a public repository into a fresh directory and check
# that the checkout really is that commit.
fetch_pinned() {
  local url=$1 sha=$2 dest=$3
  git init -q "$dest"
  git -C "$dest" fetch -q --depth 1 "$url" "$sha"
  git -C "$dest" -c advice.detachedHead=false checkout -q FETCH_HEAD
  [ "$(git -C "$dest" rev-parse HEAD)" = "$sha" ]
}

# Install only if this script (which holds the pins) changed since the last
# successful install. Compare contents, not a recorded path, so a second
# checkout or worktree with newer pins still installs them.
want=$(sha256sum < "$script" | cut -d' ' -f1)
if [ "$(cat "$hashes" 2>/dev/null)" != "$want" ]; then
  rm -f "$hashes"
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT

  # Apple Human Interface Guidelines reviewer (dickwu/apple-design-skill).
  fetch_pinned https://github.com/dickwu/apple-design-skill \
    ff4ebddf0488598723a82bfdce1afb04de0b1c44 "$work/apple"
  rm -rf "$skills_dir/apple-design"
  mkdir -p "$skills_dir/apple-design"
  cp -R "$work/apple/SKILL.md" "$work/apple/references" "$skills_dir/apple-design/"

  # UI/UX Pro Max, plus its slides and design-system skills
  # (nextlevelbuilder/ui-ux-pro-max-skill, MIT). The search scripts use the
  # Python standard library only and make no network calls.
  fetch_pinned https://github.com/nextlevelbuilder/ui-ux-pro-max-skill \
    28b3b8dee8c976df4fa3e94aed4e092ad43fcb03 "$work/uiux"
  for skill in ui-ux-pro-max design-system slides; do
    rm -rf "${skills_dir:?}/$skill"
    cp -RL "$work/uiux/.claude/skills/$skill" "$skills_dir/$skill"
    rm -rf "$skills_dir/$skill/scripts/tests" "$skills_dir/$skill/scripts/__pycache__"
    cp "$work/uiux/LICENSE" "$skills_dir/$skill/LICENSE"
  done
  # Upstream writes its script paths for a plugin install; point them at the
  # user skills folder instead.
  sed -i 's|\${CLAUDE_PLUGIN_ROOT}/\.claude/skills/|$HOME/.claude/skills/|g' \
    "$skills_dir/ui-ux-pro-max/SKILL.md"

  echo "$want" > "$hashes"
fi
