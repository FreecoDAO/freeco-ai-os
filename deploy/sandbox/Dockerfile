# The agent sandbox: a Linux box agents can actually work in.
#
# The default image was python:3.12-slim, which has no compiler, no git and no
# network tooling. Agents were asked to build and test in the sandbox, found it
# empty, and went looking for the host shell instead - which is exactly what
# the sandbox exists to prevent. An unusable safe path is not a safe path; it
# is a detour sign pointing at the dangerous one.
#
# Build once:
#   docker build -t freeco-sandbox:latest deploy/sandbox
#
# Then set in ~/.freeco-ai/config.toml:
#   [docker]
#   image = "freeco-sandbox:latest"
FROM rust:1.85-slim-bookworm@sha256:9f841bbe9e7d8e37ceb96ed907265a3a0df7f44e3737d0b100e7907a679acb36

# Toolchain for building and testing this workspace, plus the basics an agent
# reaches for when investigating. Kept to one layer so the image stays small
# enough to rebuild without thinking about it.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        pkg-config \
        libssl-dev \
        ca-certificates \
        git \
        curl \
        jq \
        ripgrep \
        python3 \
        python3-pip \
        python3-yaml \
        sqlite3 \
        unzip \
        less \
        socat \
    && rm -rf /var/lib/apt/lists/*

# Node, because without it a JavaScript project cannot be built at all.
#
# The default sandbox image was python:3.12-slim, which has no Node, so an
# agent asked to build a web app could not install a single dependency.
# Most of what these agents get asked to build - a CRM, a dashboard,
# anything with a front end - is JavaScript, so this is the difference
# between a sandbox that can do the work and one that can only run scripts.
#
# NodeSource rather than Debian's package, which lags several major
# versions behind what a modern lockfile expects.
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && npm install -g pnpm yarn \
    && rm -rf /var/lib/apt/lists/*

# The GitHub CLI, so an agent can read an issue or open a pull request
# from inside the sandbox instead of asking for host shell to do it.
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
        -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
        > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends gh \
    && rm -rf /var/lib/apt/lists/*

# Clippy and rustfmt: the CI gates. An agent that can compile but cannot run
# the same checks CI runs will keep proposing changes that fail on the server,
# which wastes a full CI cycle to learn something it could have known locally.
RUN rustup component add clippy rustfmt

# Keep cargo on the PATH for login shells.
#
# The image sets PATH for normal shells, but `bash -lc` re-reads /etc/profile
# and drops it, so an agent that runs a login shell finds no cargo and
# concludes Rust is missing.
RUN echo 'export PATH=/usr/local/cargo/bin:$PATH' > /etc/profile.d/10-cargo.sh


# Pre-warm the crates.io index. The sandbox usually runs with --network none,
# so a cargo command that needs to fetch the index would fail with a confusing
# network error rather than an obvious one. Having it baked in means offline
# builds of already-vendored dependencies work.
RUN cargo search --limit 1 serde >/dev/null 2>&1 || true

# Agents run as a non-root user. Nothing here needs root, and a container that
# grants it teaches habits that are wrong outside the container.
#
# The user is created here but only switched to at the very end. `USER agent`
# used to sit on this line, which made every later `apt-get install` and
# `chmod` run unprivileged -- so the browser layers below never built, and the
# image in use predated them entirely. Everything that needs root now runs
# before the switch.
RUN useradd --create-home --shell /bin/bash agent

# Windows cross-compilation, so an agent can ship a Windows build from here.
#
# Proven before being added: a binary built in this image with this linker
# printed `std::env::consts::OS` as "windows" when run on a Windows host. The
# linker is Debian's mingw-w64; the target is the matching Rust std.
#
# iOS is deliberately absent. Apple's SDK is licensed for Apple hardware only
# and signing needs Apple's tooling, so iOS builds belong on the macOS CI
# runners the release workflow already uses, not in a Linux container.
RUN apt-get update && apt-get install -y --no-install-recommends \
        gcc-mingw-w64-x86-64 \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-pc-windows-gnu

# Brave, and a screen to watch it on.
#
# The browser tools drove a browser on the host, outside the isolation, so
# "agents work sandboxed" and "agents can browse" were two different
# claims. A browser in here closes that, and it is also what lets a
# container hand its screen to a person: Xvfb gives it a display, x11vnc
# publishes that display, and noVNC serves it over HTTP so it opens in an
# ordinary browser tab with no VNC client to install.
#
# Brave rather than Chrome: same engine, no Google telemetry, which is the
# same reason the host-side browser tools prefer it.
RUN curl -fsSL https://brave-browser-apt-release.s3.brave.com/brave-browser-archive-keyring.gpg \
        -o /usr/share/keyrings/brave-browser-archive-keyring.gpg \
    && echo "deb [signed-by=/usr/share/keyrings/brave-browser-archive-keyring.gpg] https://brave-browser-apt-release.s3.brave.com/ stable main" \
        > /etc/apt/sources.list.d/brave-browser-release.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        brave-browser \
        xvfb \
        x11vnc \
        novnc \
        websockify \
        fluxbox \
        fonts-liberation \
    && rm -rf /var/lib/apt/lists/*

# Start a viewable desktop on demand rather than at boot.
#
# An exec-tier sandbox runs one command and exits, and paying for Xvfb and
# a window manager on every one of those is waste. A service that wants a
# screen runs freeco-screen and publishes port 6080.
#
# Shipped as a file rather than written by a RUN: a Dockerfile heredoc
# needs BuildKit and Podman's parser rejects it, and a printf with a dozen
# quoted lines breaks on the first escaped newline. A real script is
# legible, diffable, and testable outside the image.
COPY freeco-screen /usr/local/bin/freeco-screen
RUN chmod +x /usr/local/bin/freeco-screen

# Drop privileges last, after every step above that needed root.
USER agent
WORKDIR /workspace

# Stated so `docker run freeco-sandbox` without a command is self-explaining
# rather than silently exiting.
CMD ["bash", "-lc", "echo 'FreEco.ai sandbox: rust (linux + windows-gnu), cargo, clippy, node 22, pnpm, yarn, gh, git, python3, jq, rg, socat, brave. Run freeco-screen and open port 6080 to watch it. Mount your work at /workspace.'; bash"]
