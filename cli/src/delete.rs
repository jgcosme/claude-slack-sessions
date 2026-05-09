//! `slack-sessions delete <link>` — delete a bot-authored Slack message
//! identified by its permalink. Exists so users at the terminal can clean up
//! after the bot without having to DM `!delete` from Slack.
//!
//! Implementation mirrors `status::auth_test`: shell out to `curl` so the cli
//! avoids an HTTP-client dep.

use anyhow::{anyhow, Context, Result};
use std::process::Command;

use crate::credentials::Credentials;

pub fn run(link: &str) -> Result<()> {
    let creds = Credentials::load().context("failed to load credentials")?;
    let bot_token = creds
        .bot_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            anyhow!("no bot token stored — run `slack-sessions setup` first")
        })?;

    let (channel, ts) = parse_slack_message_link(link).ok_or_else(|| {
        anyhow!(
            "couldn't parse `{}` as a Slack message link — expected \
             `https://<workspace>.slack.com/archives/<channel>/p<ts>`",
            link
        )
    })?;

    let body = serde_json::json!({
        "channel": channel,
        "ts": ts,
    })
    .to_string();
    let out = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "10",
            "-H",
            &format!("Authorization: Bearer {}", bot_token),
            "-H",
            "Content-Type: application/json; charset=utf-8",
            "--data",
            &body,
            "https://slack.com/api/chat.delete",
        ])
        .output()
        .context("running curl")?;
    if !out.status.success() {
        return Err(anyhow!(
            "curl failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let resp: serde_json::Value = serde_json::from_slice(&out.stdout)
        .with_context(|| {
            format!(
                "non-JSON response from Slack: {}",
                String::from_utf8_lossy(&out.stdout)
            )
        })?;
    if resp["ok"] == serde_json::Value::Bool(true) {
        println!("[ok] deleted message in channel `{}` at ts `{}`", channel, ts);
        Ok(())
    } else {
        let err = resp["error"].as_str().unwrap_or("unknown error");
        Err(anyhow!(
            "Slack rejected delete: {} (target channel `{}` ts `{}`)",
            err,
            channel,
            ts
        ))
    }
}

/// See the matching parser in `daemon/src/main.rs`. Kept duplicated rather
/// than shared via a workspace crate because it's small and the daemon
/// returns Slack-morphism types while the cli stays type-light.
fn parse_slack_message_link(link: &str) -> Option<(String, String)> {
    let trimmed = link.trim();
    let unwrapped = trimmed
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(trimmed);
    let url = unwrapped.split('|').next()?;
    let bare = url.split('?').next()?.split('#').next()?;
    let after_archives = bare.split("/archives/").nth(1)?;
    let mut parts = after_archives.split('/');
    let channel = parts.next()?;
    let p_ts = parts.next()?;
    let digits = p_ts.strip_prefix('p')?;
    if digits.len() < 7 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let split = digits.len() - 6;
    let ts = format!("{}.{}", &digits[..split], &digits[split..]);
    Some((channel.to_string(), ts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_link() {
        let (c, t) = parse_slack_message_link(
            "https://thinkdatasci.slack.com/archives/D0B1D5FCADB/p1778209425485249",
        )
        .unwrap();
        assert_eq!(c, "D0B1D5FCADB");
        assert_eq!(t, "1778209425.485249");
    }

    #[test]
    fn parses_link_with_query() {
        let (c, t) = parse_slack_message_link(
            "https://thinkdatasci.slack.com/archives/D0B1D5FCADB/p1778209425485249?thread_ts=1778209423.931249&cid=D0B1D5FCADB",
        )
        .unwrap();
        assert_eq!(c, "D0B1D5FCADB");
        assert_eq!(t, "1778209425.485249");
    }

    #[test]
    fn parses_slack_wrapped_link() {
        let (c, t) = parse_slack_message_link(
            "<https://thinkdatasci.slack.com/archives/C123/p1778209425485249>",
        )
        .unwrap();
        assert_eq!(c, "C123");
        assert_eq!(t, "1778209425.485249");
    }

    #[test]
    fn parses_slack_wrapped_link_with_label() {
        let (c, t) = parse_slack_message_link(
            "<https://x.slack.com/archives/C123/p1778209425485249|the message>",
        )
        .unwrap();
        assert_eq!(c, "C123");
        assert_eq!(t, "1778209425.485249");
    }

    #[test]
    fn rejects_non_archive_url() {
        assert!(
            parse_slack_message_link("https://example.com/foo/bar").is_none()
        );
    }

    #[test]
    fn rejects_missing_p_prefix() {
        assert!(parse_slack_message_link(
            "https://x.slack.com/archives/C1/1778209425485249"
        )
        .is_none());
    }

    #[test]
    fn rejects_too_short_ts() {
        assert!(parse_slack_message_link(
            "https://x.slack.com/archives/C1/p123"
        )
        .is_none());
    }
}
