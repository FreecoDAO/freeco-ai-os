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

# Pre-warm the crates.io index. The sandbox usually runs with --network none,
# so a cargo command that needs to fetch the index would fail with a confusing
# network error rather than an obvious one. Having it baked in means offline
# builds of already-vendored dependencies work.
RUN cargo search --limit 1 serde >/dev/null 2>&1 || true

# Agents run as a non-root user. Nothing here needs root, and a container that
# grants it teaches habits that are wrong outside the container.
RUN useradd --create-home --shell /bin/bash agent
USER agent
WORKDIR /workspace

# Stated so `docker run freeco-sandbox` without a command is self-explaining
# rather than silently exiting.
CMD ["bash", "-lc", "echo 'FreEco.ai sandbox: rust, cargo, clippy, node 22, pnpm, yarn, gh, git, python3, jq, rg, socat. Mount your work at /workspace.'; bash"]
