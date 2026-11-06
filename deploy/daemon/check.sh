#!/bin/sh
# The full checks (fmt, clippy -D warnings, workspace tests) in the Podman VM,
# in a container capped at 5 GB so a check can never take the memory Windows
# needs. Start it from Windows, through Podman's service:
#
#   podman machine ssh "cd ~/src/freeco-ai-os && sh deploy/daemon/check.sh"
#   podman logs -f freeco-checks          # ends with CHECKS_OK or CHECKS_FAILED
#
# Uses localhost/freeco-checks-base:1 (deploy/daemon/check.Containerfile),
# which carries the system libraries, so a run starts compiling at once;
# without it the libraries are installed first (about 400 MB).
set -u
SRC=$(cd "$(dirname "$0")/../.." && pwd)
BASE=localhost/freeco-checks-base:1
INSTALL=""
if ! podman image exists "$BASE"; then
  BASE=docker.io/library/rust@sha256:2775a09d208ff0d7c1f50490c45b62db929e87ba1dcbc3f2132ac71a704bcdd3
  INSTALL='apt-get update -qq && apt-get install -y -qq pkg-config libssl-dev perl make libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev >/dev/null && '
fi
podman rm -f freeco-checks >/dev/null 2>&1 || true
# freeco-rustup keeps the pinned toolchain (rust-toolchain.toml) between runs;
# a fresh container otherwise downloads it again (about 300 MB).
podman run -d --name freeco-checks \
  --memory=5g \
  --security-opt label=disable \
  -v "$SRC:/src" \
  -v freeco-check-target:/src/target \
  -v freeco-cargo-registry:/usr/local/cargo/registry \
  -v freeco-rustup:/usr/local/rustup \
  -w /src \
  -e CARGO_BUILD_JOBS=2 \
  "$BASE" \
  sh -c "${INSTALL}sh /src/deploy/daemon/checks-inner.sh"
echo "checks started: podman logs -f freeco-checks"
