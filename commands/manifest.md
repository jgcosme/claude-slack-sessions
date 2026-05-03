---
description: Print the Slack app manifest YAML and copy it to the clipboard. Use when the user is about to create a new Slack app and needs the manifest for "Create New App → From a manifest".
allowed-tools:
  - Bash(cat *)
  - Bash(pbcopy)
---

Copy the bundled Slack app manifest to the clipboard, then print it for reference:

```bash
cat "${CLAUDE_PLUGIN_ROOT}/cli/templates/slack-app-manifest.yaml" | pbcopy && \
  echo "[ok] manifest copied to clipboard" && \
  echo && cat "${CLAUDE_PLUGIN_ROOT}/cli/templates/slack-app-manifest.yaml"
```

Tell the user the manifest is on their clipboard. Direct them to https://api.slack.com/apps → **Create New App → From a manifest**, paste, and confirm. After Slack creates the app, they install it to the workspace and copy two tokens (xoxb- bot, xapp- app-level) — covered by `/slack-sessions:setup` if they need a refresher.
