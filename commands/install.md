---
description: Build slack-sessions binaries with cargo install and register the daemon as a macOS launchd service. Idempotent — safe to re-run after updates.
allowed-tools:
  - Bash(cd *)
  - Bash(cargo install *)
  - Bash(slack-sessions *)
---

Install slack-sessions end-to-end:

1. `cd "${CLAUDE_PLUGIN_ROOT}"`
2. `cargo install --path cli` — installs the `slack-sessions` binary to `~/.cargo/bin/` (incremental; fast after first build).
3. `cargo install --path daemon` — installs `slack-sessionsd` next to it.
4. `slack-sessions service install` — writes `~/Library/LaunchAgents/io.thinkingmachines.slack-sessions.plist`, runs `launchctl bootstrap`, daemon starts.
5. `slack-sessions service status` — confirm the daemon is running.

Surface any errors verbatim. If `cargo` is not found, tell the user they need Rust installed (`curl https://sh.rustup.rs -sSf | sh`).

If the daemon fails to start, suggest they run `/slack-sessions:logs` to see what went wrong (most likely a missing token — point them at `slack-sessions setup` in their terminal).
