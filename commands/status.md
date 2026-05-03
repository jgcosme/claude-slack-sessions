---
description: Comprehensive slack-sessions health check — binaries, tokens (with live Slack auth.test), config, and daemon. Reports any [warn] or [FAIL] lines with one-line fix hints. Use to verify the full install or diagnose a failure.
allowed-tools:
  - Bash(*/slack-sessions status)
---

Run the bundled wrapper and report the output:

```bash
"${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" status
```

If any `[warn]` or `[FAIL]` lines appear, summarize them for the user and follow the inline fix hints. Otherwise just confirm everything passed.
