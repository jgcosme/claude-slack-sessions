---
description: Print the Slack app manifest YAML and copy it to the clipboard. Use when the user is about to create a new Slack app and needs the manifest for "Create New App → From a manifest".
allowed-tools:
  - Bash(cd *)
  - Bash(cargo run *)
  - Bash(slack-sessions *)
---

Run the slack-sessions CLI's `manifest --copy` subcommand and report the output.

Try `slack-sessions manifest --copy` first (if the binary is on PATH). If that fails with "command not found," fall back to running it from the workspace: `cd "${CLAUDE_PLUGIN_ROOT}" && cargo run -q -p slack-sessions-cli -- manifest --copy`.

After running, tell the user the manifest is on their clipboard and to paste it at https://api.slack.com/apps → **Create New App → From a manifest**.
