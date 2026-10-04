# VHS with git, so the demo's projects are real repositories with branches,
# worktrees and a remote to review (see demo/README.md).
FROM ghcr.io/charmbracelet/vhs
RUN apt-get update \
    && apt-get install -y --no-install-recommends git \
    && rm -rf /var/lib/apt/lists/*
