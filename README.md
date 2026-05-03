---
type: reference
description: "Overview of slack-sessions plugin — one isolated Claude Code session per Slack thread, with manifest-driven Slack-app onboarding, launchd service, and `!`-prefixed Slack-side admin commands."
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

## Install

Requires a macOS machine. Inside Claude Code:

```
/plugin marketplace add jgcosme/claude-plugins
/plugin install slack-sessions@jgcosme-plugins
/slack-sessions:setup
```

Binaries fetch automatically from GitHub Releases on first command (Apple Silicon and Linux x86_64 prebuilts; everything else falls back to a local `cargo build` and needs [Rust](https://rustup.rs) installed).

`/slack-sessions:setup` prints the Slack app manifest (and copies it to your clipboard for paste into "Create New App → From a manifest"), shows you where to grab the two tokens (`xoxb-` from OAuth & Permissions, `xapp-` from Basic Information → App-Level Tokens), and tells you to run `slack-sessions setup` in a real terminal to paste them. Then `/slack-sessions:start` registers the daemon with `launchd` so it survives reboots.

Other slash commands (all wrap the `slack-sessions` CLI):

| Command | What it does |
|---|---|
| `/slack-sessions:start` | Register the launchd service (idempotent) and start the daemon |
| `/slack-sessions:stop [--purge]` | Bootout + remove plist; `--purge` also wipes logs and tokens |
| `/slack-sessions:restart` | `launchctl kickstart -k` |
| `/slack-sessions:status` | Health check: binaries, tokens (live `auth.test`), config, daemon |
| `/slack-sessions:logs [N]` | Tail last N lines of the daemon log |
| `/slack-sessions:allow <verb>` | Manage the Slack user_id allowlist |
| `/slack-sessions:project <verb>` | Manage the project registry |

## Slack-side admin (any time, no Claude spawn)

In the bot's DM, prefix messages with `!` to manage the project registry without leaving Slack:

```
!list                        # show registered projects + default cwd
!start <project> [<msg>]     # bind the *first* message of a thread to a project's directory
!add <name> <path>           # register a project (supports ~)
!remove <name>  (or !rm)     # remove a project
!set-default <path>          # default working dir for unprefixed DMs
!help                        # this list
```

## Architecture

- A long-lived local daemon on macOS connects to Slack via Socket Mode (no inbound port, no SSH).
- Per Slack `thread_ts`, the daemon manages a `claude -p --resume` subprocess. Each turn is a fresh invocation that resumes the thread's session.
- The plugin's slash commands wrap a sibling `slack-sessions` CLI that handles app manifest, token storage (`~/.config/slack-sessions/credentials.json`, mode 0600), project registry, and launchd lifecycle.

## Status

Working: DM → `claude --resume` loop, project registry with bang-prefix selection, launchd persistence with `caffeinate` keep-awake, plugin slash commands, app manifest. Coming next: bot-channel participation (allowlist + read-only by default) and progressive output streaming.

## License

MIT — see [LICENSE](LICENSE).
