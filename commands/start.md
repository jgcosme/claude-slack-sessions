---
description: Start the slack-sessions daemon. Writes the launchd plist and bootstraps the service if not yet registered; kickstarts if already loaded. Idempotent.
allowed-tools:
  - Bash(*/slack-sessions service start)
---

Run the bundled wrapper:

```bash
"${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" service start
```

Report the output verbatim. If the wrapper says tokens are missing, point the user at `/slack-sessions:setup` (the daemon won't start without tokens).
