use serde::Deserialize;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, warn};

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
/// accordingly. Kept short on purpose — formatting (mrkdwn syntax, link
/// rewriting) is handled deterministically by the daemon's converter, not
/// here, since the model would drift turn-to-turn.
const SLACK_CONTEXT_PROMPT: &str = "You are running inside the slack-sessions daemon. Your reply is auto-posted into a Slack thread. Replies over ~35 KB are auto-chunked across multiple messages.";

pub async fn run_turn(
    prompt: &str,
    resume_session_id: Option<&str>,
    cwd: &Path,
    chunk_tx: Option<mpsc::Sender<String>>,
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
