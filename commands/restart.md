---
description: Kill and restart the slack-sessions daemon (launchctl kickstart -k). Use after updating the daemon binary or to force a fresh state.
allowed-tools:
  - Bash(*/slack-sessions service restart)
---

```bash
"${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" service restart
```

Report the output verbatim. If the kickstart fails because the daemon was never registered, point the user at `/slack-sessions:start`.
