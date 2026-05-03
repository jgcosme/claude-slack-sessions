---
description: Stop the daemon and remove the launchd plist. With --purge also removes log files and ~/.config/slack-sessions/. Tokens in the OS keyring are preserved.
allowed-tools:
  - Bash(*/slack-sessions service uninstall*)
argument-hint: "[--purge]"
---

If the user passed `--purge`, run with `--purge`. Otherwise plain `uninstall`.

```bash
if [ "$ARGUMENTS" = "--purge" ]; then
    "${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" service uninstall --purge
else
    "${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" service uninstall
fi
```

Print the output verbatim. After running, remind the user that:
- The launchd plist is gone (daemon won't auto-start on reboot).
- Slack tokens are still stored in the OS keyring; clear with `security delete-generic-password -s slack-sessions -a app-token` and same for `bot-token`.
- The cargo-installed `slack-sessions` and `slack-sessionsd` binaries are still in `~/.cargo/bin/`; remove with `cargo uninstall slack-sessions-cli && cargo uninstall slack-sessionsd`.
