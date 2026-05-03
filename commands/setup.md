---
description: Walk through the slack-sessions onboarding — Slack app creation via manifest, token entry, daemon install, allowlist bootstrap. Use when a user is setting up slack-sessions for the first time or wants a refresher on the install steps.
---

Walk the user through the full slack-sessions onboarding. Be concise; print the steps as a numbered list and stop.

1. Run `/slack-sessions:manifest` to copy the Slack app manifest and paste it into Slack's "Create New App → From a manifest" form.
2. After Slack creates the app, install it to the workspace from the OAuth & Permissions page. Copy the **Bot User OAuth Token** (starts with `xoxb-`).
3. Open **Basic Information → App-Level Tokens**. Click **Generate Token and Scopes**, add the `connections:write` scope, and copy the **App-Level Token** (starts with `xapp-`).
4. Run `/slack-sessions:install`. The first run will `cargo install` the binaries and then tell you to run `slack-sessions setup` in your terminal — that's the only step that can't happen inside Claude Code (`rpassword` needs a real TTY). Paste both tokens at the hidden prompts.
5. Re-run `/slack-sessions:install` — this time it'll register the launchd service and start the daemon. Verify with `/slack-sessions:status`; everything should be `[ok]` except a `[warn]` saying the allowlist is empty.
6. Find your Slack user_id (Slack desktop → click your avatar → **Profile** → ⋮ menu → **Copy member ID**, looks like `U0B12ABC34`) and run `/slack-sessions:allow add <your-user-id>`. Until you do this, the bot ignores everyone — including you.
7. DM your bot in Slack to test. Try `!list` (empty), `!add my-app ~/projects/my-app`, then a fresh top-level DM with `!start my-app what's in this directory?`.
