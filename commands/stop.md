---
description: Stop the slack-sessions daemon (launchctl bootout). The daemon will not auto-restart until /slack-sessions:start or /slack-sessions:install is run.
allowed-tools:
  - Bash(*/slack-sessions service stop)
---

```bash
"${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" service stop
```

Report the output verbatim.
