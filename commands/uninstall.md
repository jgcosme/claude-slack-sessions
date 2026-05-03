---
description: Stop the daemon and remove the launchd plist. With --purge also removes log files. Tokens in the OS keyring are preserved.
allowed-tools:
  - Bash(slack-sessions *)
argument-hint: "[--purge]"
---

If the user passed `--purge`, run `slack-sessions service uninstall --purge`. Otherwise run `slack-sessions service uninstall`.

Print the output verbatim. After running, remind the user that:
- The launchd plist is gone (daemon won't auto-start on reboot).
- Slack tokens are still stored in the OS keyring; clear with `security delete-generic-password -s slack-sessions -a app-token` and same for `bot-token`.
- The cargo-installed `slack-sessions` and `slack-sessionsd` binaries are still in `~/.cargo/bin/`; remove with `cargo uninstall slack-sessions-cli slack-sessionsd`.
