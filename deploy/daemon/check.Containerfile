# The image the full checks run in: Rust plus the system libraries the
# workspace needs on Linux (GTK/WebKit for the desktop crate, as in CI).
#
# Every check run used to install these first -- about 400 MB, which over a
# slow connection took most of an hour before the first line compiled. Built
# once, they are reused by every run.
#
#   podman build -t localhost/freeco-checks-base:1 -f deploy/daemon/check.Containerfile deploy/daemon
FROM docker.io/library/rust@sha256:2775a09d208ff0d7c1f50490c45b62db929e87ba1dcbc3f2132ac71a704bcdd3
RUN apt-get update -qq \
    && apt-get install -y -qq --no-install-recommends pkg-config libssl-dev perl make \
       libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
    && rm -rf /var/lib/apt/lists/*
