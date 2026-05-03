---
description: Stop the daemon and remove the launchd plist. With --purge also removes log files and ~/.config/slack-sessions/ (including stored tokens).
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
- Without `--purge`, Slack tokens remain in `~/.config/slack-sessions/credentials.json`. Delete that file (or re-run with `--purge`) to wipe them.
- The plugin's binaries live at `${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions-cli` and `${CLAUDE_PLUGIN_ROOT}/bin/slack-sessionsd`. They're removed when the plugin is uninstalled via `/plugin`.
