---
description: "Manage the slack-sessions project registry used for `!start <name>` selection in Slack. Without args, lists registered projects. Subcommands: add <name> <path>, list, remove <name>, set-default <path>."
allowed-tools:
  - Bash(*/slack-sessions project*)
argument-hint: "add|list|remove|set-default [name] [path]"
---

If `$ARGUMENTS` is empty, default to `list`:

```bash
ARGS="${ARGUMENTS:-list}"
"${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" project $ARGS
```

Print the output verbatim. Reminders:

- `add <name> <path>` registers a project so `!start <name> <prompt>` in Slack spawns a session in that directory. Path must exist.
- `set-default <path>` sets the working directory used when a top-level Slack message has no `!start <name>` prefix.
- `remove <name>` deletes a registered project; sessions already running are unaffected.
