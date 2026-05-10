use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

/// Encode a cwd path the way Claude Code does for its on-disk transcript
/// directory: both `/` and `.` are replaced with `-`. Example:
/// `/Users/x/projects/foo` → `-Users-x-projects-foo`,
/// `/Users/x/.claude/plugins/.../1.2.0` → `-Users-x--claude-plugins-...-1-2-0`.
fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

fn session_transcript_path(cwd: &Path, session_id: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(
        home.join(".claude")
            .join("projects")
            .join(encode_cwd(cwd))
            .join(format!("{}.jsonl", session_id)),
    )
}

/// Returns `true` if another process is currently holding the session's
/// JSONL transcript open. Used to detect the case where a user has
/// `claude --resume <id>`-ed the same session interactively while the
/// daemon is about to spawn a turn — concurrent ownership corrupts the
/// transcript and silently kills the daemon's subprocess (issue #3).
///
/// Best-effort: returns `false` (proceed) on any probe failure rather
/// than blocking turns when the check itself is broken (e.g., no lsof,
/// no HOME). Returns `false` when the transcript file doesn't exist
/// yet — that's only possible mid-spawn of the very first turn, and the
/// caller already gates on `resume_session_id.is_some()`.
pub fn session_is_busy(cwd: &Path, session_id: &str) -> bool {
    let path = match session_transcript_path(cwd, session_id) {
        Some(p) => p,
        None => return false,
    };
    if !path.exists() {
        return false;
    }
    let output = match std::process::Command::new("lsof")
        .arg("-t")
        .arg(&path)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            warn!(error = %e, "lsof probe failed; assuming session is free");
            return false;
        }
    };
    !output.stdout.iter().all(|b| b.is_ascii_whitespace())
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamMessage {
    System(SystemMessage),
    Assistant(AssistantWrapper),
    User(#[allow(dead_code)] serde_json::Value),
    Result(ResultMessage),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct SystemMessage {
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AssistantWrapper {
    message: AssistantMessage,
}

#[derive(Debug, Deserialize)]
struct AssistantMessage {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct ResultMessage {
    session_id: Option<String>,
    is_error: Option<bool>,
    result: Option<String>,
}

pub struct ClaudeResult {
    pub session_id: Option<String>,
    pub text: String,
}

/// Tells the model where its output is going so it can phrase / size replies
/// accordingly, and instructs it on the brief-by-default response style with
/// the `<done>` shortcut for tasks that don't need a reply.
///
/// Formatting (mrkdwn syntax, link rewriting) is handled deterministically by
/// the daemon's converter, not by this prompt — model output drifts
/// turn-to-turn and we want byte-level guarantees there. The `<done>`
/// shortcut is a different category: a graceful UX optimization where a
/// missed sentinel just produces the same chatty reply we'd post anyway.
const SLACK_CONTEXT_PROMPT: &str = "You are running inside the slack-sessions daemon. Your reply is auto-posted into a Slack thread. Replies over ~35 KB are auto-chunked across multiple messages.\n\nDefault to brief. If the request is straightforward and you complete it without needing clarification, output exactly `<done>` on its own line and nothing else — the daemon will react with :white_check_mark: on the user's message and skip posting any reply. Otherwise reply normally with what was asked for: clarifications, errors, blockers, or the answer itself. Skip recaps, \"let me know if...\" follow-ups, and unsolicited suggestions.";

pub async fn run_turn(
    prompt: &str,
    resume_session_id: Option<&str>,
    cwd: &Path,
    chunk_tx: Option<mpsc::Sender<String>>,
    // Fires once with the session id as soon as claude's first stream-json
    // `system` message arrives — typically within a couple of seconds of
    // spawn. The caller uses this to surface the id in Slack before the
    // rest of the turn completes, so the id is recoverable even if the
    // turn later hangs or crashes.
    mut session_id_tx: Option<oneshot::Sender<String>>,
) -> Result<ClaudeResult, Box<dyn std::error::Error + Send + Sync>> {
    let mut cmd = Command::new("claude");
    cmd.current_dir(cwd)
        .arg("-p")
        .arg(prompt)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--append-system-prompt")
        .arg(SLACK_CONTEXT_PROMPT)
        .arg("--permission-mode")
        .arg("bypassPermissions");
    if let Some(id) = resume_session_id {
        cmd.arg("--resume").arg(id);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    debug!(
        ?resume_session_id,
        cwd = %cwd.display(),
        "spawning claude"
    );
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().ok_or("no stdout from claude")?;
    let mut reader = BufReader::new(stdout).lines();

    let mut text = String::new();
    let mut session_id: Option<String> = None;
    let mut error_text: Option<String> = None;

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<StreamMessage>(&line) {
            Ok(StreamMessage::System(s)) => {
                if let Some(id) = s.session_id {
                    if session_id.is_none() {
                        if let Some(tx) = session_id_tx.take() {
                            let _ = tx.send(id.clone());
                        }
                    }
                    session_id = Some(id);
                }
            }
            Ok(StreamMessage::Assistant(w)) => {
                for block in w.message.content {
                    if let ContentBlock::Text { text: t } = block {
                        let chunk = if text.is_empty() {
                            t
                        } else {
                            format!("\n\n{}", t)
                        };
                        text.push_str(&chunk);
                        if let Some(tx) = chunk_tx.as_ref() {
                            let _ = tx.send(chunk).await;
                        }
                    }
                }
            }
            Ok(StreamMessage::Result(r)) => {
                if let Some(id) = r.session_id {
                    session_id = Some(id);
                }
                if r.is_error.unwrap_or(false) {
                    error_text = r.result;
                }
            }
            Ok(_) => {}
            Err(e) => {
                warn!(error = %e, line = %line, "failed to parse claude stream message");
            }
        }
    }

    let status = child.wait().await?;
    if !status.success() && error_text.is_none() {
        return Err(format!("claude exited with status {}", status).into());
    }

    let final_text = if let Some(err) = error_text {
        format!("_claude error:_ {}", err)
    } else if text.is_empty() {
        "_(no text response)_".to_string()
    } else {
        text
    };

    Ok(ClaudeResult {
        session_id,
        text: final_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_cwd_no_dots() {
        assert_eq!(
            encode_cwd(Path::new("/Users/x/projects/foo")),
            "-Users-x-projects-foo"
        );
    }

    #[test]
    fn encode_cwd_with_dots() {
        assert_eq!(
            encode_cwd(Path::new("/Users/x/.claude/plugins/v/1.2.0")),
            "-Users-x--claude-plugins-v-1-2-0"
        );
    }

    #[test]
    fn encode_cwd_root() {
        assert_eq!(encode_cwd(Path::new("/")), "-");
    }

    #[test]
    fn session_is_busy_missing_file_is_free() {
        let tmp = std::env::temp_dir().join(format!("ssX-{}", std::process::id()));
        assert!(!session_is_busy(&tmp, "deadbeef-no-such-session"));
    }
}
