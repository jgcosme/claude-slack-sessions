---
description: "List or resume claude sessions on disk (slack-bound + standalone). Without args, lists the most recent. Subcommands: list [--limit N], resume <session-id>."
allowed-tools:
  - Bash(*/slack-sessions sessions*)
argument-hint: "list|resume [session-id]"
---

If `$ARGUMENTS` is empty, default to `list`:

```bash
ARGS="${ARGUMENTS:-list}"
"${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" sessions $ARGS
```

Print the output verbatim. Reminders:

- `list` walks `~/.claude/projects/*/*.jsonl`, recovers each session's true cwd from the JSONL preamble, and shows the 30 most recent. Use `list --limit N` to widen.
- `resume <session-id>` looks up the session's cwd and `exec`'s `claude --resume <id>` in that directory — replacing this CLI process so the user lands directly in claude. Use this to continue a Slack-driven session interactively without juggling cwd manually.
- The same listing is available inside Slack as `!sessions` (or `!sessions list` / `!sessions resume <id>`).
