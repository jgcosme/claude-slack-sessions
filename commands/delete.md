---
description: Delete a bot-authored Slack message by permalink. Pass a Slack message link (right-click → Copy link). Slack rejects with cant_delete_message if the target wasn't authored by this bot.
allowed-tools:
  - Bash(*/slack-sessions delete*)
argument-hint: "<slack-message-link>"
---

```bash
"${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" delete "$ARGUMENTS"
```

Print the output verbatim. If Slack returned `cant_delete_message`, the message wasn't authored by this bot — the bot can only delete its own posts.
