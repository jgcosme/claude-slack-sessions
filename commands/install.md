---
description: Build slack-sessions binaries with cargo install and (when tokens are stored) register the daemon as a macOS launchd service. Idempotent — safe to re-run after updates or partial setups.
allowed-tools:
  - Bash(export *)
  - Bash(cd *)
  - Bash(cargo install *)
  - Bash(*/slack-sessions *)
  - Bash(*/codesign-binaries.sh)
---

Run the install end-to-end. The flow is idempotent and self-aware: it builds binaries first, then checks whether tokens are stored before deciding whether to register the launchd service.

```bash
export PATH="${HOME}/.cargo/bin:${PATH}"
cd "${CLAUDE_PLUGIN_ROOT}" || exit 1

echo "==> building binaries (cargo install --path cli, then daemon)"
cargo install --path cli || { echo "cargo install cli failed"; exit 1; }
cargo install --path daemon || { echo "cargo install daemon failed"; exit 1; }

echo
echo "==> re-signing binaries with stable code-signing cert"
"${CLAUDE_PLUGIN_ROOT}/bin/codesign-binaries.sh" || {
    echo "codesign step failed — install will continue, but expect keychain re-prompts on restart"
}

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
    echo "    slack-sessions setup"
    echo "(paste the xoxb- bot token and xapp- app-level token at the hidden prompts)"
    echo
    echo "Then re-run /slack-sessions:install to register the launchd service."
fi
```

If `cargo install` fails because cargo isn't installed, point the user at https://rustup.rs (one-line install).

If `cargo install` succeeds but `setup --check` fails (tokens missing), surface the printed message verbatim — the user needs to run `slack-sessions setup` in their terminal before re-running this command. They also need to allowlist their Slack user_id via `/slack-sessions:allow add <user-id>` before the bot will respond to them.

If the daemon fails to start after `service install`, run `/slack-sessions:logs` to see why.
