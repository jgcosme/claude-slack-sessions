---
description: Manage the slack-sessions allowlist of Slack user IDs that get full tool access. Without args, lists the allowlist. Subcommands: add <user-id>, list, remove <user-id>.
allowed-tools:
  - Bash(slack-sessions allow *)
argument-hint: "add|list|remove [user-id]"
---

Run `slack-sessions allow $ARGUMENTS`. If `$ARGUMENTS` is empty, run `slack-sessions allow list` instead.

Print the output verbatim. If the user asked to add a user_id, remind them that allowlisted users get full tools (bypassPermissions); everyone else gets a no-tools chat reply.

If the user doesn't know how to find their Slack user_id, tell them: in the Slack desktop app, click your avatar → Profile → ⋮ menu → Copy member ID. The ID looks like `U0B12ABC34`.
