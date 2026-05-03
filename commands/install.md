---
description: Fetch slack-sessions binaries (prebuilt from GitHub Releases, or build from source) and register the daemon as a macOS launchd service when tokens are present. Idempotent — safe to re-run.
allowed-tools:
  - Bash(*/install.sh)
  - Bash(*/slack-sessions *)
---

Run the install end-to-end. The flow is idempotent and self-aware: it installs binaries first (download or build-from-source), then checks whether tokens are present before registering the launchd service.

```bash
set -e
cd "${CLAUDE_PLUGIN_ROOT}" || exit 1

echo "==> installing binaries"
bash "${CLAUDE_PLUGIN_ROOT}/bin/install.sh"

WRAPPER="${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions"

echo
echo "==> checking tokens"
if "${WRAPPER}" setup --check >/dev/null 2>&1; then
    echo "[ok] tokens stored — registering launchd service"
    "${WRAPPER}" service install
    echo
    echo "==> verifying"
    "${WRAPPER}" status
else
    echo "[--] tokens not yet stored"
    echo
    echo "Next step: in your terminal, run:"
    echo "    ${WRAPPER} setup"
    echo "(paste the xoxb- bot token and xapp- app-level token at the hidden prompts)"
    echo
    echo "Then re-run /slack-sessions:install to register the launchd service."
fi
```

The install script tries to download the prebuilt binary tarball matching `plugin.json`'s version from GitHub Releases. If the platform isn't covered (anything outside Darwin-arm64, Darwin-x86_64, Linux-x86_64) or the release isn't published yet, it falls back to a local `cargo build --release` — point the user at https://rustup.rs if cargo isn't installed.

If `setup --check` fails (tokens missing), surface the printed message verbatim — the user needs to run `slack-sessions setup` in their terminal before re-running this command. Tokens write to `~/.config/slack-sessions/credentials.json` (mode 0600). The user also needs to allowlist their Slack user_id via `/slack-sessions:allow add <user-id>` before the bot will respond to them.

If the daemon fails to start after `service install`, run `/slack-sessions:logs` to see why.
