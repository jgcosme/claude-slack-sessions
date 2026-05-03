---
description: Walk through slack-sessions onboarding — print Slack app manifest (and copy it to clipboard), then list the remaining steps to get the daemon running.
allowed-tools:
  - Bash(*/slack-sessions manifest*)
---

Run this single bash block — it prints the Slack app manifest, copies it to the clipboard, and prints the rest of the onboarding steps with paths already resolved:

```bash
"${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions" manifest --copy

cat <<EOF

==> next steps

1. Open https://api.slack.com/apps → "Create New App" → "From a manifest".
   Pick your workspace, paste the YAML above (it's already in your clipboard),
   and confirm.

2. After Slack creates the app, install it to your workspace:
   "OAuth & Permissions" → "Install to <Workspace>".
   Copy the **Bot User OAuth Token** (starts with xoxb-).

3. Open "Basic Information" → "App-Level Tokens" → "Generate Token and Scopes".
   Add the connections:write scope, copy the **App-Level Token** (starts with xapp-).

4. In a real terminal (not inside Claude Code — token entry needs a TTY), run:

       ${CLAUDE_PLUGIN_ROOT}/bin/slack-sessions setup

   Paste both tokens at the hidden prompts. They write to
   ~/.config/slack-sessions/credentials.json (mode 0600).

5. Back in Claude Code, run /slack-sessions:start to register the launchd
   service and start the daemon. Then /slack-sessions:status to verify.

6. Find your Slack user_id (Slack desktop → click your avatar → "Profile" →
   ⋮ menu → "Copy member ID", looks like U0B12ABC34) and run:

       /slack-sessions:allow add <your-user-id>

   Until you do this, the bot ignores everyone — including you.

7. DM your bot in Slack to test. Try !list (no projects yet),
   then !add my-app ~/projects/my-app, then start a fresh top-level DM with:
       !start my-app what's in this directory?
EOF
```

After printing, do not add any commentary — the numbered list is self-contained.
