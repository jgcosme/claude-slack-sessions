---
type: reference
description: "Overview of slack-sessions plugin - one isolated Claude Code session per Slack thread, early scaffold status."
created: 2026-05-02
project: claude-slack-sessions
---

# slack-sessions

Drive Claude Code from Slack with **one isolated session per Slack thread**.

- Top-level message in DM with the bot → fresh `claude` session.
- Reply in the thread → resumes that session via `claude --resume`.
- Each thread is its own transcript, so unrelated tasks don't pollute each other's context.

## Why this exists

Most Slack ↔ Claude Code bridges share a single session per channel or per user, which causes context rot as unrelated tasks accumulate in the same transcript. This plugin's only opinion is: **threads are sessions.**

## When to use this vs Remote Control

Anthropic ships [Remote Control](https://code.claude.com/docs/en/remote-control), which lets you drive your local Claude Code from claude.ai/code in a browser or the Claude mobile app. It's the right choice when your goal is *"drive my own Claude Code from another device."* Zero install, native UI, up to 32 parallel sessions with `--spawn=worktree` for per-session isolation — use it.

`slack-sessions` solves a different problem: **making Slack itself the surface.** Pick this if any of the following matter to you:

- **You live in Slack.** Conversations with Claude land where the rest of your day already happens. No context-switch to a separate app, no second notification stream.
- **You want the chat platform's UI to drive session structure.** Top-level DM = new session; threaded reply = `claude --resume`. The thread your eyes are already in is the session your next reply continues. Remote Control requires explicitly choosing "new session" in the UI.
- **You want trigger surfaces beyond a human at a screen.** Anything that can post to Slack — incoming webhooks, Slack workflows, the GitHub–Slack integration, scheduled posts, on-call paging tools — can DM the bot to spawn or continue a session. Remote Control is human-driven only.
- **You want shared access.** Remote Control sessions are tied to your individual claude.ai login. Anyone who can DM the bot in your Slack workspace can talk to Claude on your machine — relevant if you ever want a collaborator, intern, or workspace teammate to share access (with the security tradeoffs that implies).

If none of those apply, prefer Remote Control.

## Architecture (planned)

- A long-lived local daemon on macOS connects to Slack via Socket Mode (no inbound port, no SSH).
- Per Slack `thread_ts`, the daemon manages a `claude` subprocess and resumes it on reply.
- The Claude Code plugin layer (this repo) provides slash commands to install, start, stop, and inspect the daemon.

## Status

Early. The Slack DM → daemon → `claude --resume` → threaded reply loop works end-to-end on macOS. Plugin slash commands (`install` / `start` / `stop` / `status`), `launchd` service install, per-thread working directory, and progressive output streaming are still TODO.

## License

MIT — see [LICENSE](LICENSE).
