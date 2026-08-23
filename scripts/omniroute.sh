#!/bin/sh
# Start OmniRoute, the local AI gateway FreEco can route through.
#
# One endpoint in front of 340+ providers, 90+ of them free, so a free model
# running dry mid-task falls through to another instead of stopping the agent.
#
# Two things about this are not obvious and cost an afternoon to find:
#
# 1. `--network host` is required. Rootless podman's default `pasta` networking
#    accepts `-p 20128:20128`, reports the mapping in `podman ps`, and forwards
#    nothing: the app answers 200 inside the container and the port is dead from
#    the VM and from Windows alike. Host networking publishes it properly.
#
# 2. JWT_SECRET, API_KEY_SECRET and STORAGE_ENCRYPTION_KEY have no defaults.
#    Without them the container starts, logs "Ready", and never serves.
#
# Works with podman or docker; podman is tried first because it runs rootless.
set -e

IMAGE="docker.io/diegosouzapw/omniroute:latest"
NAME="omniroute"
PORT="${OMNIROUTE_PORT:-20128}"
ENV_FILE="${OMNIROUTE_ENV:-$HOME/.freeco-ai/omniroute.env}"

if command -v podman >/dev/null 2>&1; then RT=podman
elif command -v docker >/dev/null 2>&1; then RT=docker
else echo "need podman or docker" >&2; exit 1
fi

# Secrets are generated once and kept, so restarting does not invalidate
# sessions or make previously stored provider keys undecryptable.
if [ ! -f "$ENV_FILE" ]; then
  mkdir -p "$(dirname "$ENV_FILE")"
  gen() { head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n'; }
  cat > "$ENV_FILE" <<EOF
JWT_SECRET=$(gen)
API_KEY_SECRET=$(gen)
STORAGE_ENCRYPTION_KEY=$(gen)
STORAGE_ENCRYPTION_KEY_VERSION=v1
INITIAL_PASSWORD=$(gen | cut -c1-24)
PORT=$PORT
NODE_ENV=production
AUTH_COOKIE_SECURE=false
REQUIRE_API_KEY=false
BASE_URL=http://localhost:$PORT
NEXT_PUBLIC_BASE_URL=http://localhost:$PORT
OMNIROUTE_AUTO_FREE_FALLBACK_TO_FULL_POOL=true
EOF
  chmod 600 "$ENV_FILE" 2>/dev/null || true
  echo "generated $ENV_FILE"
fi

case "${1:-start}" in
  start)
    if $RT ps --format '{{.Names}}' 2>/dev/null | grep -qx "$NAME"; then
      echo "already running"; exit 0
    fi
    $RT rm -f "$NAME" >/dev/null 2>&1 || true
    # Retry: image layers already fetched are kept, so a dropped connection
    # resumes rather than starting the 2 GB pull again.
    if ! $RT image exists "$IMAGE" 2>/dev/null; then
      i=1
      while [ $i -le 6 ]; do
        $RT pull "$IMAGE" && break
        echo "pull attempt $i failed, retrying"; i=$((i+1)); sleep 15
      done
    fi
    $RT run -d --name "$NAME" --network host --env-file "$ENV_FILE" \
        --restart unless-stopped "$IMAGE" >/dev/null
    printf "waiting for it to serve"
    i=1
    while [ $i -le 40 ]; do
      if curl -s -o /dev/null -m 3 "http://localhost:$PORT/v1/models" 2>/dev/null; then
        echo; echo "ready on http://localhost:$PORT"; exit 0
      fi
      printf "."; sleep 3; i=$((i+1))
    done
    echo; echo "started but not answering after 2 minutes; check: $RT logs $NAME" >&2
    exit 1
    ;;
  stop)   $RT stop "$NAME" >/dev/null 2>&1; echo "stopped" ;;
  status)
    $RT ps --filter "name=$NAME" --format '{{.Names}} {{.Status}}' 2>/dev/null
    curl -s -o /dev/null -m 5 -w "endpoint: %{http_code}\n" "http://localhost:$PORT/v1/models" 2>/dev/null \
      || echo "endpoint: unreachable"
    ;;
  logs)   $RT logs --tail 40 "$NAME" ;;
  *) echo "usage: $0 [start|stop|status|logs]" >&2; exit 2 ;;
esac
