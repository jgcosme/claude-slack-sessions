---
description: Tail recent lines from the slack-sessions daemon log file. Use when the daemon misbehaves or to verify Slack events are arriving. Accepts an optional line count argument (default 50).
allowed-tools:
  - Bash(slack-sessions *)
argument-hint: "[lines]"
---

Run `slack-sessions service logs --lines $ARGUMENTS` if the user provided a line count, otherwise `slack-sessions service logs --lines 50`. Print the output verbatim.

If the user wants to follow the log live (tail -f), tell them to run `slack-sessions service logs --follow` in their terminal — Claude Code can't stream a long-running command.
