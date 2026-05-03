---
description: Stop the slack-sessions daemon and remove its launchd registration. Pass --purge to also wipe log files and ~/.config/slack-sessions/ (tokens included).
allowed-tools:
  - Bash(*/slack-sessions service stop*)
argument-hint: "[--purge]"
---

If the user passed `--purge`, run with `--purge`. Otherwise plain `stop`.

```bash
if [ "$ARGUMENTS" = "--purge" ]; then
    "${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" service stop --purge
else
    "${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" service stop
fi
```

Report the output verbatim. After running, remind the user that:
- The launchd plist is gone — daemon won't auto-start on reboot until they run `/slack-sessions:start` again.
- Without `--purge`, tokens remain in `~/.config/slack-sessions/credentials.json`. To wipe them, re-run with `--purge` or delete the file manually.
