# slack-sessions

Drive Claude Code from Slack with **one isolated session per Slack thread**.

- Top-level message in DM with the bot → fresh `claude` session.
- Reply in the thread → resumes that session via `claude --resume`.
- Each thread is its own transcript, so unrelated tasks don't pollute each other's context.

Status: **early scaffold — not yet functional.**

## Why this exists

Most Slack ↔ Claude Code bridges share a single session per channel or per user, which causes context rot as unrelated tasks accumulate in the same transcript. This plugin's only opinion is: **threads are sessions.**

## Architecture (planned)

- A long-lived local daemon on macOS connects to Slack via Socket Mode (no inbound port, no SSH).
- Per Slack `thread_ts`, the daemon manages a `claude` subprocess and resumes it on reply.
- The Claude Code plugin layer (this repo) provides slash commands to install, start, stop, and inspect the daemon.

## License

MIT — see [LICENSE](LICENSE).
