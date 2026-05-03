---
description: Manage the slack-sessions allowlist of Slack user IDs that get full tool access. Without args, lists the allowlist. Subcommands: add <user-id>, list, remove <user-id>.
allowed-tools:
  - Bash(*/slack-sessions allow*)
argument-hint: "add|list|remove [user-id]"
---

If `$ARGUMENTS` is empty, default to `list`:

```bash
ARGS="${ARGUMENTS:-list}"
"${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" allow $ARGS
```

Print the output verbatim. If the user added a new user_id, remind them: allowlisted users get full tools (`bypassPermissions`); everyone else gets a `--tools ""` chat reply (no Read, no Bash, no MCP, no network).

If the user doesn't know how to find their Slack user_id, tell them: in the Slack desktop app, click your avatar → Profile → ⋮ menu → Copy member ID. The ID looks like `U0B12ABC34`.
