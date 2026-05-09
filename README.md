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
- @-mention the bot in a channel where it's been added → same model, but the bot's bookkeeping (thread + session id) is DM'd to you so the channel thread stays clean.
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
| `/slack-sessions:status` | Health check: binaries, tokens (live `auth.test`), bot OAuth scopes, config, daemon |
| `/slack-sessions:logs [N]` | Tail last N lines of the daemon log |
| `/slack-sessions:allow <verb>` | Manage the Slack user_id allowlist |
| `/slack-sessions:project <verb>` | Manage the project registry |
| `/slack-sessions:delete <link>` | Delete a bot-authored Slack message by permalink (clean up after the bot from the terminal) |

## Slack-side commands

In a DM with the bot (or as the @mention text in a channel where it's been added), prefix the message with `!`:

**Session control** — affect how the current turn or thread runs:

```
!start <project> [<msg>]     # bind the *first* message of a thread to a project's directory
!reset                       # clear the thread's claude session; next message starts fresh (keeps cwd)
!reset <project> [<msg>]     # clear and rebind to a different project, optionally kicking off in the same turn
!silent <message>            # run silently — reactions only (:eyes: → :white_check_mark: / :x:),
                             #   no streaming, no thread reply. Composes with !start.
!delete <message-link>       # delete a bot-authored message by permalink (Slack rejects with
                             #   `cant_delete_message` if the target wasn't authored by the bot)
```

**Project registry**:

```
!list                        # show registered projects + default cwd
!add <name> <path>           # register a project (supports ~)
!remove <name>  (or !rm)     # remove a project
!set-default <path>          # default working dir for unprefixed DMs
```

**Allowlist** (allowlisted users only):

```
!allow add <user-id>         # grant a Slack user full-tools access
!allow list                  # show allowlisted user IDs
!allow remove <user-id>      # revoke access
```

**Misc**:

```
!help                        # show all of the above in-thread
```

## How replies work

The daemon converts Claude's standard Markdown to Slack's [mrkdwn flavor](https://api.slack.com/reference/surfaces/formatting) before posting — `**bold**`, headings, fenced code blocks with language tags, `[label](url)` links all render correctly in Slack instead of as literal asterisks and broken links.

Status is conveyed primarily through **reactions on your message**:

| Reaction | Meaning |
|---|---|
| `:eyes:` | Message received and queued (added before the per-thread mutex is acquired, so a second message arriving mid-turn isn't invisible) |
| `:hourglass_flowing_sand:` | Claude is running for this turn |
| `:white_check_mark:` | Turn completed cleanly, no reply (used by `<done>` shortcut and `!silent` success) |
| `:x:` | `!silent` turn failed (also gets a brief error reply for visibility) |

There is no `_thinking..._` placeholder in the thread anymore — reactions handle the in-progress signaling, and the bot's reply only appears once content is ready to post.

By default the bot is **brief**: for clear, executable requests it completes the task and either replies tersely or — when no reply is needed at all — emits a `<done>` sentinel that the daemon turns into a `:white_check_mark:` reaction with no message in the thread. If the model misses the sentinel and produces a brief reply instead, that's the same UX as the old chatty default — graceful degradation, not a violation.

On a thread's first turn (or after `!reset`), the daemon posts a small announce with the Slack `thread_ts` and the Claude session UUID — useful if you ever want to attach to that session manually with `claude --resume <uuid>` from a terminal. For DM-originated threads the announce sits with the conversation; for channel-mention threads it's DM'd to the user instead so the channel thread stays clean.

## Architecture

- A long-lived local daemon on macOS connects to Slack via Socket Mode (no inbound port, no SSH).
- Per Slack `thread_ts`, the daemon manages a `claude -p --resume` subprocess. Each turn is a fresh invocation that resumes the thread's session.
- Both DM messages (`message.im` events) and channel @-mentions (`app_mention` events) flow into the same handler; the surface only changes where the thread+session announce lands.
- Outbound text is converted from GitHub-flavored Markdown to Slack mrkdwn at the daemon's posting boundary; conversion is byte-deterministic (no model-side formatting hint to drift).
- The plugin's slash commands wrap a sibling `slack-sessions` CLI that handles app manifest, token storage (`~/.config/slack-sessions/credentials.json`, mode 0600), project registry, allowlist, launchd lifecycle, and bot-message deletion by permalink.

## Required Slack scopes

The manifest in `cli/templates/slack-app-manifest.yaml` is the source of truth. Current bot scopes:

```
chat:write, chat:write.public, im:history, im:read,
app_mentions:read, channels:history, groups:history,
reactions:write, im:write
```

When upgrading, run `/slack-sessions:status` — it parses the live `x-oauth-scopes` header and warns about any expected scope your installed app token is missing (typically because new scopes were added after install). Fix is "OAuth & Permissions → Install to Workspace" to reinstall and grant.

## License

MIT — see [LICENSE](LICENSE).
