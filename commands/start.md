---
description: Start the slack-sessions daemon (load via launchctl if not already loaded, kickstart if loaded but stopped).
allowed-tools:
  - Bash(slack-sessions *)
---

Run `slack-sessions service start` and report the output. If the binary isn't on PATH, suggest the user run `/slack-sessions:install` first.
