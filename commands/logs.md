---
description: Tail recent lines from the slack-sessions daemon log file. Use when the daemon misbehaves or to verify Slack events are arriving. Accepts an optional line count argument (default 50).
allowed-tools:
  - Bash(*/slack-sessions service logs*)
argument-hint: "[lines]"
---

Use the user-supplied line count if any, otherwise default to 50:

```bash
LINES="${ARGUMENTS:-50}"
"${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" service logs --lines "$LINES"
```

Print the output verbatim. If the user wants to follow the log live (tail -f), tell them to run `slack-sessions service logs --follow` in their terminal — Claude Code can't stream a long-running command.
