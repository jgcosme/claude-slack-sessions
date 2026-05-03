---
description: Start the slack-sessions daemon (load via launchctl if not already loaded, kickstart if loaded but stopped).
allowed-tools:
  - Bash(*/slack-sessions service start)
---

Run the bundled wrapper:

```bash
"${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" service start
```

Report the output. If the wrapper says the binary isn't installed, suggest `/slack-sessions:install` first.
