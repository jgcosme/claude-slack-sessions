---
description: Walk through the slack-sessions onboarding — Slack app creation via manifest, token entry, daemon install. Use when a user is setting up slack-sessions for the first time or wants a refresher on the install steps.
---

Walk the user through the full slack-sessions onboarding. Be concise; print the steps as a numbered list and stop.

1. Run `/slack-sessions:manifest` to copy the Slack app manifest and paste it into Slack's "Create New App → From a manifest" form.
2. After Slack creates the app, install it to the workspace from the OAuth & Permissions page. Copy the **Bot User OAuth Token** (starts with `xoxb-`).
3. Open **Basic Information → App-Level Tokens**. Click **Generate Token and Scopes**, add the `connections:write` scope, and copy the **App-Level Token** (starts with `xapp-`).
4. In a terminal, run `slack-sessions setup`. Paste each token at the hidden prompts. (This step can't be done from inside Claude Code — `rpassword` requires a real TTY.)
5. Run `/slack-sessions:install` to build the daemon binaries and register the launchd service.
6. Run `/slack-sessions:status` to confirm it's running, then DM the bot in Slack to test.
