#!/bin/sh
# Start, or replace, the FreEco.ai daemon container. Run inside the Podman VM:
#   podman machine ssh 'sh /home/user/freeco/run.sh'
#
# --restart=always plus the VM's podman-restart service bring it back after a
# crash and whenever the VM starts. --network host puts it on the VM's network,
# where the services it talks to (CRM, Speaches, accounting, OmniRoute) are
# published at 127.0.0.1, and WSL forwards its port 50051 to Windows, so the
# dashboard opens at http://127.0.0.1:50051 as before.
set -eu
DATA=/home/user/freeco
IMAGE=${IMAGE:-localhost/freeco-daemon:latest}
SOCK=/run/user/$(id -u)/podman/podman.sock

mkdir -p "$DATA"
podman rm -f freeco-daemon >/dev/null 2>&1 || true
podman run -d --name freeco-daemon \
  --restart=always \
  --network host \
  --security-opt label=disable \
  -v "$DATA:$DATA" \
  -v "$SOCK:/run/podman/podman.sock" \
  --env-file "$DATA/container.env" \
  "$IMAGE"
echo "freeco-daemon started; health: curl -s http://127.0.0.1:50051/api/health"
