mod allowlist;
mod claude;
mod config;
mod credentials;
mod mrkdwn;
mod projects;
mod session;

use slack_morphism::prelude::*;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tracing::{info, warn};

use crate::allowlist::Allowlist;
use crate::credentials::Credentials;
use crate::projects::ProjectsRegistry;
use crate::session::{now_unix, SessionStore};

const SLACK_MAX_TEXT: usize = 38_000;
/// During streaming, once the in-flight message accumulates this many bytes,
/// finalize it as `_(part N)_` and start a new placeholder in the same thread.
/// Keeps each `chat.update` body well under SLACK_MAX_TEXT and gives the user
/// progressive output across multiple messages instead of a 38 KB freeze.
const STREAM_ROLLOVER: usize = 35_000;
/// Wall-clock threshold above which we post a separate `<@user> _done_` reply
/// in the thread after the final edit. `chat.update` does not fire mention
/// notifications, so a fresh `chat.postMessage` is required to actually ping.
/// Trivial replies (under this threshold) stay clean — no extra message.
const PING_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(30);

static BOT_TOKEN: OnceLock<SlackApiToken> = OnceLock::new();
static SESSION_STORE: OnceLock<Arc<SessionStore>> = OnceLock::new();

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,slack_sessionsd=debug".into()),
        )
        .init();

    let creds = Credentials::load()?;
    let app_token_str = read_secret("SLACK_APP_TOKEN", creds.app_token.as_deref(), "app-level")?;
    let bot_token_str = read_secret("SLACK_BOT_TOKEN", creds.bot_token.as_deref(), "bot")?;
    let app_token: SlackApiToken = SlackApiToken::new(app_token_str.into());
    let bot_token: SlackApiToken = SlackApiToken::new(bot_token_str.into());
    let _ = BOT_TOKEN.set(bot_token);

    spawn_caffeinate();

    let state_path = sessions_state_path()?;
    let store = Arc::new(SessionStore::load(state_path.clone()).await?);
    info!(path = %state_path.display(), "session store loaded");
    let _ = SESSION_STORE.set(store);

    let client = Arc::new(SlackClient::new(SlackClientHyperConnector::new()?));

    let callbacks = SlackSocketModeListenerCallbacks::new().with_push_events(on_push_event);

    let listener_env = Arc::new(
        SlackClientEventsListenerEnvironment::new(client.clone()).with_error_handler(on_error),
    );

    let listener = SlackClientSocketModeListener::new(
        &SlackClientSocketModeConfig::new(),
        listener_env,
        callbacks,
    );

    info!("connecting to Slack via Socket Mode");
    listener.listen_for(&app_token).await?;
    listener.serve().await;

    Ok(())
}

async fn on_push_event(
    event: SlackPushEventCallback,
    client: Arc<SlackHyperClient>,
    _state: SlackClientEventsUserState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match event.event {
        SlackEventCallbackBody::Message(msg) => on_message_event(client, msg).await,
        SlackEventCallbackBody::AppMention(mention) => on_mention_event(client, mention).await,
        _ => Ok(()),
    }
}

async fn on_message_event(
    client: Arc<SlackHyperClient>,
    msg: SlackMessageEvent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let is_im = msg
        .origin
        .channel_type
        .as_ref()
        .map(|c| c.0.as_str() == "im")
        .unwrap_or(false);
    if !is_im || msg.sender.bot_id.is_some() || msg.subtype.is_some() {
        return Ok(());
    }

    let Some(channel) = msg.origin.channel.clone() else {
        return Ok(());
    };
    let Some(text) = msg.content.as_ref().and_then(|c| c.text.clone()) else {
        return Ok(());
    };
    let ts = msg.origin.ts.clone();
    let thread_ts = msg.origin.thread_ts.clone().unwrap_or_else(|| ts.clone());
    let user_id = msg
        .sender
        .user
        .as_ref()
        .map(|u| u.0.clone())
        .unwrap_or_default();

    let allowlist = Allowlist::load().unwrap_or_default();
    let is_allowlisted = allowlist.contains(&user_id);

    info!(
        user = %user_id,
        allowlisted = is_allowlisted,
        tier = if is_allowlisted { "full" } else { "denied" },
        surface = "dm",
        channel = %channel.0,
        ts = %ts.0,
        thread_ts = %thread_ts.0,
        text = %text,
        "event"
    );

    if is_allowlisted {
        tokio::spawn(async move {
            if let Err(e) = handle_full_session(
                client,
                channel,
                thread_ts,
                ts,
                text,
                user_id,
                Surface::Dm,
            )
            .await
            {
                warn!(error = %e, "DM handling failed");
            }
        });
    } else {
        tokio::spawn(async move {
            if let Err(e) = handle_denied_reply(client, channel, thread_ts, user_id).await {
                warn!(error = %e, "denied reply failed");
            }
        });
    }

    Ok(())
}

async fn on_mention_event(
    client: Arc<SlackHyperClient>,
    mention: SlackAppMentionEvent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let channel = mention.channel.clone();
    let user_id = mention.user.0.clone();
    let ts = mention.origin.ts.clone();
    let thread_ts = mention.origin.thread_ts.clone().unwrap_or_else(|| ts.clone());
    let raw_text = mention.content.text.clone().unwrap_or_default();
    let text = strip_leading_mention(&raw_text);

    let allowlist = Allowlist::load().unwrap_or_default();
    let is_allowlisted = allowlist.contains(&user_id);

    info!(
        user = %user_id,
        allowlisted = is_allowlisted,
        tier = if is_allowlisted { "full" } else { "denied" },
        surface = "channel-mention",
        channel = %channel.0,
        ts = %ts.0,
        thread_ts = %thread_ts.0,
        text = %text,
        "event"
    );

    if is_allowlisted {
        tokio::spawn(async move {
            if let Err(e) = handle_full_session(
                client,
                channel,
                thread_ts,
                ts,
                text,
                user_id,
                Surface::ChannelMention,
            )
            .await
            {
                warn!(error = %e, "channel mention handling failed");
            }
        });
    } else {
        tokio::spawn(async move {
            if let Err(e) = handle_denied_reply(client, channel, thread_ts, user_id).await {
                warn!(error = %e, "denied reply failed");
            }
        });
    }

    Ok(())
}

/// Strip a leading Slack user mention like `<@U0B230S8FFS>` (typically the bot
/// itself) and surrounding whitespace from the start of a message. Leaves the
/// rest of the text intact, including any other mentions deeper in the message.
fn strip_leading_mention(text: &str) -> String {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<@") {
        if let Some(end) = rest.find('>') {
            return rest[end + 1..].trim_start().to_string();
        }
    }
    trimmed.to_string()
}

/// Reply to a non-allowlisted Slack user with a static refusal. No claude
/// invocation, no session creation, no LLM cost.
async fn handle_denied_reply(
    client: Arc<SlackHyperClient>,
    channel: SlackChannelId,
    thread_ts: SlackTs,
    user_id: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = BOT_TOKEN.get().ok_or("bot token not initialized")?;
    let session = client.open_session(token);
    let text = format!(
        "You are not in the allow list. This bot is invitation-only. \
         To request access, share your Slack member ID `{}` with the bot owner.",
        user_id
    );
    let req = SlackApiChatPostMessageRequest::new(
        channel,
        SlackMessageContent::new().with_text(text),
    )
    .with_thread_ts(thread_ts);
    session.chat_post_message(&req).await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    Dm,
    ChannelMention,
}

/// Outer wrapper that posts a `:eyes:` reaction on the user's message before
/// queuing on the per-thread mutex, then removes it once the turn finishes
/// (success or error). Without this, a second message arriving mid-turn has
/// no acknowledgement and the bot looks frozen.
async fn handle_full_session(
    client: Arc<SlackHyperClient>,
    channel: SlackChannelId,
    thread_ts: SlackTs,
    trigger_ts: SlackTs,
    text: String,
    user_id: String,
    surface: Surface,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let acked = add_reaction(&client, &channel, &trigger_ts, "eyes")
        .await
        .is_ok();
    let result = handle_full_session_inner(
        client.clone(),
        channel.clone(),
        thread_ts,
        trigger_ts.clone(),
        text,
        user_id,
        surface,
    )
    .await;
    if acked {
        if let Err(e) = remove_reaction(&client, &channel, &trigger_ts, "eyes").await {
            warn!(error = %e, "failed to remove ack reaction");
        }
    }
    result
}

async fn handle_full_session_inner(
    client: Arc<SlackHyperClient>,
    channel: SlackChannelId,
    thread_ts: SlackTs,
    trigger_ts: SlackTs,
    text: String,
    user_id: String,
    surface: Surface,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let started = std::time::Instant::now();
    let store = SESSION_STORE
        .get()
        .ok_or("session store not initialized")?
        .clone();

    // `!silent` prefix → suppress placeholder/streaming/final reply for the
    // claude turn. Reactions on the user's message convey status. Composes
    // with magic-command prefixes: `!silent !start <project> <msg>` works.
    // Magic-command replies that produce structured output (lists, binds,
    // errors) are always shown, since silencing them would hide the entire
    // response.
    let (silent, text) = match text.strip_prefix("!silent ") {
        Some(rest) => (true, rest.trim_start().to_string()),
        None if text.trim() == "!silent" => {
            post_reply(
                &client,
                &channel,
                &thread_ts,
                "_`!silent` needs a message after the prefix._",
            )
            .await?;
            return Ok(());
        }
        None => (false, text),
    };

    let entry_arc = store.get_or_create(&thread_ts.0).await;
    let mut entry = entry_arc.lock().await;

    let is_first_turn = entry.cwd.is_none();

    // Resolve prompt + cwd via either a magic command (`!start ...` etc.) or the default path.
    let (prompt_text, resolved_cwd) = if let Some(parsed) = parse_magic_command(&text) {
        match parsed {
            Ok(cmd) => match execute_magic_command(cmd, is_first_turn) {
                MagicResult::ReplyOnly(reply) => {
                    post_reply(&client, &channel, &thread_ts, &reply).await?;
                    return Ok(());
                }
                MagicResult::Reject(hint) => {
                    post_reply(&client, &channel, &thread_ts, &hint).await?;
                    return Ok(());
                }
                MagicResult::BindOnly { cwd } => {
                    let cwd_str = cwd.to_string_lossy().to_string();
                    post_reply(
                        &client,
                        &channel,
                        &thread_ts,
                        &format!(
                            "_Bound this thread to `{}`. Send your prompt._",
                            cwd.display()
                        ),
                    )
                    .await?;
                    entry.cwd = Some(cwd_str);
                    entry.last_active_unix = now_unix();
                    drop(entry);
                    store.persist().await.ok();
                    return Ok(());
                }
                MagicResult::BindAndRun { cwd, prompt } => (prompt, cwd),
                MagicResult::Reset {
                    cwd: new_cwd,
                    prompt,
                } => {
                    entry.claude_session_id = None;
                    if let Some(ref c) = new_cwd {
                        entry.cwd = Some(c.to_string_lossy().to_string());
                    }
                    entry.last_active_unix = now_unix();
                    match prompt {
                        None => {
                            let cwd_display = entry
                                .cwd
                                .clone()
                                .unwrap_or_else(|| "(default)".to_string());
                            drop(entry);
                            store.persist().await.ok();
                            post_reply(
                                &client,
                                &channel,
                                &thread_ts,
                                &format!(
                                    "_Session reset. Bound to `{}`. Send your prompt._",
                                    cwd_display
                                ),
                            )
                            .await?;
                            return Ok(());
                        }
                        Some(p) => {
                            let resolved = entry
                                .cwd
                                .clone()
                                .map(PathBuf::from)
                                .unwrap_or_else(default_cwd);
                            (p, resolved)
                        }
                    }
                }
            },
            Err(hint) => {
                post_reply(&client, &channel, &thread_ts, &format!("_{}_", hint)).await?;
                return Ok(());
            }
        }
    } else if is_first_turn {
        (text.clone(), default_cwd())
    } else {
        let cwd = entry
            .cwd
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(default_cwd);
        (text.clone(), cwd)
    };

    // For channel mentions, fetch thread history every turn and prepend any
    // messages claude hasn't seen yet — i.e., posted between the previous
    // bot turn and this one but never @-mentioned. Covers both first
    // activation (no prior turns; everything before the trigger is new) and
    // mid-session interleaves (user types non-mention messages between two
    // @-mentions). Skipped for DMs: every DM message fires a turn, so there
    // are no interleaved non-mention messages to recover.
    let prompt_text = if surface == Surface::ChannelMention {
        match fetch_thread_replies(&client, &channel, &thread_ts).await {
            Ok(history) => {
                let unseen: Vec<&SlackHistoryMessage> = history
                    .iter()
                    .filter(|m| {
                        let ts = &m.origin.ts.0;
                        ts.as_str() < trigger_ts.0.as_str()
                            && entry
                                .last_seen_ts
                                .as_deref()
                                .is_none_or(|last| ts.as_str() > last)
                            && m.sender.bot_id.is_none()
                    })
                    .collect();
                if unseen.is_empty() {
                    prompt_text
                } else {
                    format_with_thread_context(&unseen, &prompt_text)
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to fetch thread context; proceeding without it");
                prompt_text
            }
        }
    } else {
        prompt_text
    };

    // Surface the thread + session ids on the first turn (or after `!reset`)
    // so they're recoverable from Slack even if claude later hangs or
    // crashes. We post the message *before* spawning claude (capturing
    // `thread_ts` immediately) and update it once claude reports its
    // session id via the oneshot below.
    let announce_first_turn = entry.claude_session_id.is_none();
    let announce_ts = if announce_first_turn {
        match post_reply(
            &client,
            &channel,
            &thread_ts,
            &format!("_thread `{}`; session pending..._", thread_ts.0),
        )
        .await
        {
            Ok(ts) => Some(ts),
            Err(e) => {
                warn!(error = %e, "failed to post thread-id announce; continuing without");
                None
            }
        }
    } else {
        None
    };

    let (sid_tx, sid_rx) = if announce_first_turn {
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    if let (Some(rx), Some(announce_ts)) = (sid_rx, announce_ts.clone()) {
        let client_a = client.clone();
        let channel_a = channel.clone();
        let thread_ts_str = thread_ts.0.clone();
        tokio::spawn(async move {
            if let Ok(sid) = rx.await {
                let body = format!("_thread `{}`; session `{}`_", thread_ts_str, sid);
                if let Err(e) = update_message(&client_a, &channel_a, &announce_ts, &body).await {
                    warn!(error = %e, "thread-id announce update failed");
                }
            }
        });
    }

    if silent {
        // Show "running" via reaction in lieu of the streaming placeholder.
        // The wrapper's `:eyes:` is already on the message; this adds an
        // hourglass that we swap for `:white_check_mark:` / `:x:` at the end.
        let _ = add_reaction(&client, &channel, &trigger_ts, "hourglass_flowing_sand").await;

        let claude_result = match crate::claude::run_turn(
            &prompt_text,
            entry.claude_session_id.as_deref(),
            &resolved_cwd,
            None,
            sid_tx,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                let _ = remove_reaction(
                    &client,
                    &channel,
                    &trigger_ts,
                    "hourglass_flowing_sand",
                )
                .await;
                let _ = post_reply(
                    &client,
                    &channel,
                    &thread_ts,
                    &format!("_claude failed:_ {}", e),
                )
                .await;
                let _ = add_reaction(&client, &channel, &trigger_ts, "x").await;
                return Err(e);
            }
        };

        if entry.claude_session_id.is_none() {
            entry.claude_session_id = claude_result.session_id;
        }
        if is_first_turn {
            entry.cwd = Some(resolved_cwd.to_string_lossy().to_string());
        }
        entry.last_active_unix = now_unix();
        entry.last_seen_ts = Some(trigger_ts.0.clone());
        drop(entry);
        if let Err(e) = store.persist().await {
            warn!(error = %e, "failed to persist session store");
        }
        let _ = remove_reaction(
            &client,
            &channel,
            &trigger_ts,
            "hourglass_flowing_sand",
        )
        .await;
        let _ = add_reaction(&client, &channel, &trigger_ts, "white_check_mark").await;
        return Ok(());
    }

    let placeholder_ts = post_placeholder(&client, &channel, &thread_ts).await?;

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
    let updater = tokio::spawn(stream_updater(
        client.clone(),
        channel.clone(),
        placeholder_ts.clone(),
        thread_ts.clone(),
        rx,
    ));
    let claude_result = match crate::claude::run_turn(
        &prompt_text,
        entry.claude_session_id.as_deref(),
        &resolved_cwd,
        Some(tx),
        sid_tx,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            let outcome = updater.await.unwrap_or(StreamerOutcome {
                current_ts: Some(placeholder_ts.clone()),
                parts_committed: 0,
                bytes_committed: 0,
            });
            let err_text = format!("_claude failed:_ {}", e);
            match outcome.current_ts.as_ref() {
                Some(ts) => {
                    let _ = update_message(&client, &channel, ts, &err_text).await;
                }
                None => {
                    let _ = post_reply(&client, &channel, &thread_ts, &err_text).await;
                }
            }
            return Err(e);
        }
    };
    let outcome = updater.await.unwrap_or(StreamerOutcome {
        current_ts: Some(placeholder_ts.clone()),
        parts_committed: 0,
        bytes_committed: 0,
    });

    // `<done>` shortcut: when the model's *entire* response is the literal
    // sentinel, treat it as a "no reply needed" signal — delete the
    // streaming placeholder, react :white_check_mark: on the user's
    // message, and skip the rest of the post path. Only triggers when
    // nothing has rolled over yet (parts_committed == 0); a long response
    // that happens to end in `<done>` falls through to normal posting.
    let is_done_shortcut =
        outcome.parts_committed == 0 && claude_result.text.trim() == "<done>";

    if is_done_shortcut {
        if entry.claude_session_id.is_none() {
            entry.claude_session_id = claude_result.session_id;
        }
        if is_first_turn {
            entry.cwd = Some(resolved_cwd.to_string_lossy().to_string());
        }
        entry.last_active_unix = now_unix();
        entry.last_seen_ts = Some(trigger_ts.0.clone());
        drop(entry);
        if let Err(e) = store.persist().await {
            warn!(error = %e, "failed to persist session store");
        }
        if let Some(ts) = outcome.current_ts.as_ref() {
            if let Err(e) = delete_message(&client, &channel, ts).await {
                warn!(error = %e, "failed to delete placeholder for <done> shortcut");
            }
        }
        let _ = add_reaction(&client, &channel, &trigger_ts, "white_check_mark").await;
        return Ok(());
    }

    let tail_start = outcome.bytes_committed.min(claude_result.text.len());
    let tail = &claude_result.text[tail_start..];
    // Convert tail in one pass before chunking. Conversion is line-by-line and
    // preserves paragraph/line breaks, so chunking on `\n\n` still works.
    let converted_tail = if tail.is_empty() {
        String::new()
    } else {
        crate::mrkdwn::to_slack_mrkdwn(tail)
    };
    let chunks = if converted_tail.is_empty() {
        Vec::new()
    } else {
        chunk_for_slack(&converted_tail)
    };
    let multi_part = outcome.parts_committed > 0 || chunks.len() > 1;

    // Persist the session id BEFORE the final Slack post. If
    // `send_with_overflow_recovery` errors out — `msg_too_long` that defeats
    // recursive splitting, a transient network drop, anything — the JSONL
    // transcript on disk still corresponds to a thread we'll need to resume,
    // and orphaning it forces the next turn to start fresh with no context.
    // Persisting first means a Slack-side failure costs us only the visible
    // reply, not the session continuity.
    if entry.claude_session_id.is_none() {
        entry.claude_session_id = claude_result.session_id;
    }
    if is_first_turn {
        entry.cwd = Some(resolved_cwd.to_string_lossy().to_string());
    }
    entry.last_active_unix = now_unix();
    entry.last_seen_ts = Some(trigger_ts.0.clone());
    drop(entry);
    if let Err(e) = store.persist().await {
        warn!(error = %e, "failed to persist session store");
    }

    if let Some(first_chunk) = chunks.first() {
        let part_n = outcome.parts_committed + 1;
        let label = if multi_part {
            format!("{}\n\n_(part {})_", first_chunk, part_n)
        } else {
            first_chunk.clone()
        };
        send_with_overflow_recovery(
            &client,
            &channel,
            &thread_ts,
            outcome.current_ts.as_ref(),
            &label,
        )
        .await?;
        for (i, chunk) in chunks.iter().enumerate().skip(1) {
            let part_n = outcome.parts_committed + i + 1;
            let body = format!("{}\n\n_(part {})_", chunk, part_n);
            if let Err(e) =
                send_with_overflow_recovery(&client, &channel, &thread_ts, None, &body).await
            {
                warn!(error = %e, part = part_n, "follow-up chunk post failed");
            }
        }
    }
    maybe_ping_done(&client, &channel, &thread_ts, &user_id, started.elapsed()).await;

    Ok(())
}

enum MagicCommand<'a> {
    List,
    Help,
    Add { name: &'a str, path: &'a str },
    Remove { name: &'a str },
    SetDefault { path: &'a str },
    Start { name: &'a str, message: &'a str },
    Reset { project: &'a str, message: &'a str },
    AllowAdd { user_id: &'a str },
    AllowList,
    AllowRemove { user_id: &'a str },
}

enum MagicResult {
    /// Post the reply text to the thread and stop (no claude spawn).
    ReplyOnly(String),
    /// Bind the thread to a project but don't run claude on this turn.
    BindOnly { cwd: PathBuf },
    /// Bind the thread and run claude with the given prompt.
    BindAndRun { cwd: PathBuf, prompt: String },
    /// Post a hint and stop (e.g. unknown project, wrong turn).
    Reject(String),
    /// Clear the thread's claude session id so the next turn starts fresh.
    /// `cwd` rebinds the working directory if `Some` (otherwise keep current).
    /// `prompt` runs claude with that prompt on this turn if `Some` (otherwise
    /// just post a confirmation and stop).
    Reset {
        cwd: Option<PathBuf>,
        prompt: Option<String>,
    },
}

/// Returns:
/// - `None` if the text is not a magic command (no `!` prefix or unknown keyword).
/// - `Some(Ok(cmd))` for a valid command.
/// - `Some(Err(hint))` for a recognized keyword used incorrectly.
fn parse_magic_command(text: &str) -> Option<Result<MagicCommand<'_>, String>> {
    let trimmed = text.trim_start();
    let rest = trimmed.strip_prefix('!')?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let cmd = parts.next()?.trim();
    let args = parts.next().unwrap_or("").trim();
    match cmd {
        "list" => Some(Ok(MagicCommand::List)),
        "help" => Some(Ok(MagicCommand::Help)),
        "add" => {
            let mut split = args.splitn(2, char::is_whitespace);
            let name = split.next().unwrap_or("").trim();
            let path = split.next().unwrap_or("").trim();
            if name.is_empty() || path.is_empty() {
                Some(Err("usage: `!add <name> <path>`".into()))
            } else {
                Some(Ok(MagicCommand::Add { name, path }))
            }
        }
        "remove" | "rm" => {
            if args.is_empty() {
                Some(Err("usage: `!remove <name>`".into()))
            } else {
                Some(Ok(MagicCommand::Remove { name: args }))
            }
        }
        "set-default" => {
            if args.is_empty() {
                Some(Err("usage: `!set-default <path>`".into()))
            } else {
                Some(Ok(MagicCommand::SetDefault { path: args }))
            }
        }
        "start" => {
            let mut split = args.splitn(2, char::is_whitespace);
            let name = split.next().unwrap_or("").trim();
            let message = split.next().unwrap_or("").trim();
            if name.is_empty() {
                Some(Err("usage: `!start <project> [<message>]`".into()))
            } else {
                Some(Ok(MagicCommand::Start { name, message }))
            }
        }
        "reset" => {
            let mut split = args.splitn(2, char::is_whitespace);
            let project = split.next().unwrap_or("").trim();
            let message = split.next().unwrap_or("").trim();
            Some(Ok(MagicCommand::Reset { project, message }))
        }
        "allow" => {
            let mut split = args.splitn(2, char::is_whitespace);
            let sub = split.next().unwrap_or("").trim();
            let arg = split.next().unwrap_or("").trim();
            match sub {
                "add" => {
                    if arg.is_empty() {
                        Some(Err("usage: `!allow add <user-id>`".into()))
                    } else {
                        Some(Ok(MagicCommand::AllowAdd { user_id: arg }))
                    }
                }
                "list" => Some(Ok(MagicCommand::AllowList)),
                "remove" | "rm" => {
                    if arg.is_empty() {
                        Some(Err("usage: `!allow remove <user-id>`".into()))
                    } else {
                        Some(Ok(MagicCommand::AllowRemove { user_id: arg }))
                    }
                }
                _ => Some(Err("usage: `!allow add|list|remove <user-id>`".into())),
            }
        }
        _ => None,
    }
}

fn execute_magic_command(cmd: MagicCommand<'_>, is_first_turn: bool) -> MagicResult {
    match cmd {
        MagicCommand::List => MagicResult::ReplyOnly(format_project_list()),
        MagicCommand::Help => MagicResult::ReplyOnly(format_help()),
        MagicCommand::Add { name, path } => {
            MagicResult::ReplyOnly(add_project_via_command(name, path))
        }
        MagicCommand::Remove { name } => MagicResult::ReplyOnly(remove_project_via_command(name)),
        MagicCommand::SetDefault { path } => MagicResult::ReplyOnly(set_default_via_command(path)),
        MagicCommand::AllowAdd { user_id } => {
            MagicResult::ReplyOnly(allow_add_via_command(user_id))
        }
        MagicCommand::AllowList => MagicResult::ReplyOnly(format_allowlist()),
        MagicCommand::AllowRemove { user_id } => {
            MagicResult::ReplyOnly(allow_remove_via_command(user_id))
        }
        MagicCommand::Start { name, message } => {
            if !is_first_turn {
                return MagicResult::Reject(
                    "_This thread is already bound. Reply normally, or DM a fresh top-level message to switch projects._".into(),
                );
            }
            let registry = ProjectsRegistry::load().unwrap_or_default();
            let cwd = match registry.lookup(name) {
                Some(p) => p,
                None => {
                    return MagicResult::Reject(format!(
                        "_No project named `{}`. Try `!list` to see registered projects._",
                        name
                    ))
                }
            };
            if message.is_empty() {
                MagicResult::BindOnly { cwd }
            } else {
                MagicResult::BindAndRun {
                    cwd,
                    prompt: message.to_string(),
                }
            }
        }
        MagicCommand::Reset { project, message } => {
            if project.is_empty() {
                if !message.is_empty() {
                    return MagicResult::Reject(
                        "_usage: `!reset` (clear session, keep cwd) or `!reset <project> [<message>]`_"
                            .into(),
                    );
                }
                return MagicResult::Reset {
                    cwd: None,
                    prompt: None,
                };
            }
            let registry = ProjectsRegistry::load().unwrap_or_default();
            let cwd = match registry.lookup(project) {
                Some(p) => p,
                None => {
                    return MagicResult::Reject(format!(
                        "_No project named `{}`. Try `!list` to see registered projects._",
                        project
                    ))
                }
            };
            let prompt = if message.is_empty() {
                None
            } else {
                Some(message.to_string())
            };
            MagicResult::Reset {
                cwd: Some(cwd),
                prompt,
            }
        }
    }
}

fn add_project_via_command(name: &str, path_str: &str) -> String {
    if let Err(e) = ProjectsRegistry::validate_name(name) {
        return format!("_Invalid name:_ {}", e);
    }
    let canonical = match crate::projects::canonicalize_dir(path_str) {
        Ok(p) => p,
        Err(e) => return format!("_{}_", e),
    };
    let canonical_str = canonical.to_string_lossy().to_string();
    let mut reg = match ProjectsRegistry::load() {
        Ok(r) => r,
        Err(e) => return format!("_failed to load registry: {}_", e),
    };
    let prior = reg
        .projects
        .insert(name.to_string(), canonical_str.clone());
    if let Err(e) = reg.save() {
        return format!("_failed to save registry: {}_", e);
    }
    if prior.is_some() {
        format!("[ok] updated `{}` → `{}`", name, canonical_str)
    } else {
        format!(
            "[ok] added `{}` → `{}`\nUse `!start {}` on a *new* thread to start a session there.",
            name, canonical_str, name
        )
    }
}

fn remove_project_via_command(name: &str) -> String {
    let mut reg = match ProjectsRegistry::load() {
        Ok(r) => r,
        Err(e) => return format!("_failed to load registry: {}_", e),
    };
    if reg.projects.remove(name).is_none() {
        return format!("_no project named `{}`_", name);
    }
    if let Err(e) = reg.save() {
        return format!("_failed to save registry: {}_", e);
    }
    format!("[ok] removed `{}`", name)
}

fn allow_add_via_command(user_id: &str) -> String {
    if let Err(e) = Allowlist::validate_user_id(user_id) {
        return format!("_invalid user id: {}_", e);
    }
    let mut list = match Allowlist::load() {
        Ok(l) => l,
        Err(e) => return format!("_failed to load allowlist: {}_", e),
    };
    let inserted = list.slack_user_ids.insert(user_id.to_string());
    if let Err(e) = list.save() {
        return format!("_failed to save allowlist: {}_", e);
    }
    if inserted {
        format!("[ok] allowlisted `{}`", user_id)
    } else {
        format!("[--] `{}` is already on the allowlist", user_id)
    }
}

fn allow_remove_via_command(user_id: &str) -> String {
    let mut list = match Allowlist::load() {
        Ok(l) => l,
        Err(e) => return format!("_failed to load allowlist: {}_", e),
    };
    if !list.slack_user_ids.remove(user_id) {
        return format!("_`{}` is not on the allowlist_", user_id);
    }
    if let Err(e) = list.save() {
        return format!("_failed to save allowlist: {}_", e);
    }
    format!("[ok] removed `{}`", user_id)
}

fn format_allowlist() -> String {
    let list = Allowlist::load().unwrap_or_default();
    let mut out = String::new();
    out.push_str("*slack-sessions — allowlist*\n\n");
    if list.slack_user_ids.is_empty() {
        out.push_str("_The allowlist is empty._ Bot will ignore everyone except via direct CLI access.\n");
    } else {
        out.push_str(&format!(
            "Allowlisted Slack user IDs ({}):\n",
            list.slack_user_ids.len()
        ));
        for id in &list.slack_user_ids {
            out.push_str(&format!("• `{}`\n", id));
        }
    }
    out.push_str("\n_Allowlisted users get full tools (bypassPermissions). Everyone else gets a pure-chat reply with no filesystem or network access._\n");
    out
}

fn set_default_via_command(path_str: &str) -> String {
    let canonical = match crate::projects::canonicalize_dir(path_str) {
        Ok(p) => p,
        Err(e) => return format!("_{}_", e),
    };
    let canonical_str = canonical.to_string_lossy().to_string();
    let mut reg = match ProjectsRegistry::load() {
        Ok(r) => r,
        Err(e) => return format!("_failed to load registry: {}_", e),
    };
    reg.default_dir = Some(canonical_str.clone());
    if let Err(e) = reg.save() {
        return format!("_failed to save registry: {}_", e);
    }
    format!("[ok] default working directory: `{}`", canonical_str)
}

fn default_cwd() -> PathBuf {
    ProjectsRegistry::load()
        .map(|r| r.resolved_default())
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")))
}

fn format_project_list() -> String {
    let registry = ProjectsRegistry::load().unwrap_or_default();
    let mut out = String::new();
    out.push_str("*slack-sessions — project registry*\n\n");
    out.push_str(&format!(
        "Default working directory: `{}`\n",
        registry.resolved_default().display()
    ));
    if registry.default_dir.is_none() {
        out.push_str("_(using $HOME — set with `slack-sessions project set-default <path>`)_\n");
    }
    out.push('\n');
    if registry.projects.is_empty() {
        out.push_str("_No registered projects._\n");
        out.push_str("Add one in your terminal: `slack-sessions project add <name> <path>`\n");
    } else {
        out.push_str("Registered projects:\n");
        for (name, path) in &registry.projects {
            out.push_str(&format!("• `{}` → `{}`\n", name, path));
        }
        out.push('\n');
        out.push_str("Begin a new thread with `!start <name> [<message>]` to bind the session to a project's directory.\n");
    }
    out
}

fn format_help() -> String {
    let mut out = String::new();
    out.push_str("*slack-sessions — help*\n\n");
    out.push_str("• Top-level DM → starts a new Claude session in the default working directory.\n");
    out.push_str("• Reply in the thread → resumes that session.\n");
    out.push_str("• `!start <project> [<message>]` on the *first* message of a thread → bind that thread to a registered project's directory.\n");
    out.push_str("• `!reset` → clear the thread's session and start fresh on the next message (keeps `cwd`). `!reset <project> [<message>]` to also rebind.\n");
    out.push_str("• `!silent <message>` → run silently — reactions only (:eyes: → :white_check_mark: / :x:), no streaming or final reply. Composes with `!start`: `!silent !start <project> <message>`.\n");
    out.push_str("\n*Registry commands* (allowlisted senders only, no Claude spawn):\n");
    out.push_str("• `!list` — show registered projects + default working directory\n");
    out.push_str("• `!add <name> <path>` — register a project (path can use `~`)\n");
    out.push_str("• `!remove <name>` (or `!rm <name>`) — remove a registered project\n");
    out.push_str("• `!set-default <path>` — set default working directory for unprefixed DMs\n");
    out.push_str("\n*Allowlist commands* (allowlisted senders only):\n");
    out.push_str("• `!allow list` — show allowlisted Slack user IDs\n");
    out.push_str("• `!allow add <user-id>` — grant a Slack user full-tools access\n");
    out.push_str("• `!allow remove <user-id>` — revoke access\n");
    out.push_str("\n• `!help` — show this message\n");
    out
}

/// Fetch a thread's full reply history via `conversations.replies`. Returns
/// every message in the thread including the parent.
async fn fetch_thread_replies(
    client: &SlackHyperClient,
    channel: &SlackChannelId,
    thread_ts: &SlackTs,
) -> Result<Vec<SlackHistoryMessage>, Box<dyn std::error::Error + Send + Sync>> {
    let token = BOT_TOKEN.get().ok_or("bot token not initialized")?;
    let session = client.open_session(token);
    let req = SlackApiConversationsRepliesRequest::new(channel.clone(), thread_ts.clone());
    let resp = session.conversations_replies(&req).await?;
    Ok(resp.messages)
}

/// Format prior thread messages as a text block prepended to the user's
/// current prompt, so the claude session has context for what was being
/// discussed before the bot was mentioned.
fn format_with_thread_context(history: &[&SlackHistoryMessage], current: &str) -> String {
    let mut out = String::from("[Thread context — earlier messages in this Slack thread:]\n");
    for msg in history {
        let user = msg
            .sender
            .user
            .as_ref()
            .map(|u| u.0.as_str())
            .unwrap_or("unknown");
        let text = msg.content.text.as_deref().unwrap_or("");
        out.push_str(&format!("<@{}>: {}\n", user, text));
    }
    out.push_str("[End of thread context]\n\n");
    out.push_str(current);
    out
}

async fn post_reply(
    client: &SlackHyperClient,
    channel: &SlackChannelId,
    thread_ts: &SlackTs,
    text: &str,
) -> Result<SlackTs, Box<dyn std::error::Error + Send + Sync>> {
    let token = BOT_TOKEN.get().ok_or("bot token not initialized")?;
    let session = client.open_session(token);
    let req = SlackApiChatPostMessageRequest::new(
        channel.clone(),
        SlackMessageContent::new().with_text(text.to_string()),
    )
    .with_thread_ts(thread_ts.clone());
    let resp = session.chat_post_message(&req).await?;
    Ok(resp.ts)
}

async fn post_placeholder(
    client: &SlackHyperClient,
    channel: &SlackChannelId,
    thread_ts: &SlackTs,
) -> Result<SlackTs, Box<dyn std::error::Error + Send + Sync>> {
    let token = BOT_TOKEN.get().ok_or("bot token not initialized")?;
    let session = client.open_session(token);
    let req = SlackApiChatPostMessageRequest::new(
        channel.clone(),
        SlackMessageContent::new().with_text("_thinking..._".to_string()),
    )
    .with_thread_ts(thread_ts.clone());
    let resp = session.chat_post_message(&req).await?;
    Ok(resp.ts)
}

async fn update_message(
    client: &SlackHyperClient,
    channel: &SlackChannelId,
    ts: &SlackTs,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = BOT_TOKEN.get().ok_or("bot token not initialized")?;
    let session = client.open_session(token);
    let req = SlackApiChatUpdateRequest::new(
        channel.clone(),
        SlackMessageContent::new().with_text(text.to_string()),
        ts.clone(),
    );
    session.chat_update(&req).await?;
    Ok(())
}

async fn delete_message(
    client: &SlackHyperClient,
    channel: &SlackChannelId,
    ts: &SlackTs,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = BOT_TOKEN.get().ok_or("bot token not initialized")?;
    let session = client.open_session(token);
    let req = SlackApiChatDeleteRequest::new(channel.clone(), ts.clone());
    session.chat_delete(&req).await?;
    Ok(())
}

async fn add_reaction(
    client: &SlackHyperClient,
    channel: &SlackChannelId,
    ts: &SlackTs,
    name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = BOT_TOKEN.get().ok_or("bot token not initialized")?;
    let session = client.open_session(token);
    let req = SlackApiReactionsAddRequest::new(
        channel.clone(),
        SlackReactionName(name.to_string()),
        ts.clone(),
    );
    session.reactions_add(&req).await?;
    Ok(())
}

async fn remove_reaction(
    client: &SlackHyperClient,
    channel: &SlackChannelId,
    ts: &SlackTs,
    name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = BOT_TOKEN.get().ok_or("bot token not initialized")?;
    let session = client.open_session(token);
    let req = SlackApiReactionsRemoveRequest::new(SlackReactionName(name.to_string()))
        .with_channel(channel.clone())
        .with_timestamp(ts.clone());
    session.reactions_remove(&req).await?;
    Ok(())
}

/// Split `s` into chunks each at most `SLACK_MAX_TEXT` bytes, preferring
/// paragraph (`\n\n`) breaks, then line (`\n`) breaks, then char boundaries.
/// Reserves headroom for a chunk-indicator suffix appended by the caller.
fn chunk_for_slack(s: &str) -> Vec<String> {
    const SUFFIX_BUDGET: usize = 32;
    let max = SLACK_MAX_TEXT.saturating_sub(SUFFIX_BUDGET);
    if s.len() <= max {
        return vec![s.to_string()];
    }
    let mut chunks = Vec::new();
    let mut rest = s;
    while rest.len() > max {
        let window = &rest[..max];
        let cut = window
            .rfind("\n\n")
            .map(|i| i + 2)
            .or_else(|| window.rfind('\n').map(|i| i + 1))
            .unwrap_or_else(|| {
                let mut end = max;
                while !rest.is_char_boundary(end) {
                    end -= 1;
                }
                end
            });
        chunks.push(rest[..cut].trim_end().to_string());
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        chunks.push(rest.to_string());
    }
    chunks
}

/// Returns true if `e` (or any error in its source chain) is a Slack
/// `msg_too_long` rejection. `chat.update` and `chat.postMessage` enforce a
/// limit on the rendered Block Kit payload — not raw bytes — so URL-heavy
/// mrkdwn (`<URL|label>`, channel mentions) can trip this well below
/// `SLACK_MAX_TEXT`. We treat it as a signal to split smaller and retry.
fn is_msg_too_long_error(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = current {
        if format!("{err}").contains("msg_too_long") {
            return true;
        }
        current = err.source();
    }
    false
}

/// Split `s` into two roughly-equal pieces at the best boundary near the
/// midpoint: prefer `\n\n`, then `\n`, then a char boundary. Returns None if
/// `s` is too short or no valid split point exists.
fn split_in_half(s: &str) -> Option<(String, String)> {
    if s.len() < 2 {
        return None;
    }
    let mid = s.len() / 2;
    let pick_closest = |head: Option<usize>, tail: Option<usize>| -> Option<usize> {
        match (head, tail) {
            (Some(h), Some(t)) => Some(if mid - h <= t - mid { h } else { t }),
            (Some(h), None) => Some(h),
            (None, Some(t)) => Some(t),
            (None, None) => None,
        }
    };
    let para = pick_closest(
        s[..mid].rfind("\n\n").map(|i| i + 2),
        s[mid..].find("\n\n").map(|i| mid + i + 2),
    );
    let line = pick_closest(
        s[..mid].rfind('\n').map(|i| i + 1),
        s[mid..].find('\n').map(|i| mid + i + 1),
    );
    let cut = para.or(line).unwrap_or_else(|| {
        let mut p = mid;
        while p > 0 && !s.is_char_boundary(p) {
            p -= 1;
        }
        p
    });
    if cut == 0 || cut >= s.len() {
        return None;
    }
    let head = s[..cut].trim_end().to_string();
    let tail = s[cut..].trim_start().to_string();
    if head.is_empty() || tail.is_empty() {
        return None;
    }
    Some((head, tail))
}

/// Deliver `text` to the thread, recovering from `msg_too_long` by splitting
/// the body in half and posting the halves as follow-up replies. If
/// `placeholder_ts` is `Some`, the first attempt is a `chat.update` on that
/// placeholder; otherwise it's a fresh threaded `chat.postMessage`. On a split,
/// the head goes to the placeholder (or as a new reply) and the tail is posted
/// as a follow-up — which itself recovers if it overflows again.
type SendFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<SlackTs, Box<dyn std::error::Error + Send + Sync>>>
            + Send
            + 'a,
    >,
>;

fn send_with_overflow_recovery<'a>(
    client: &'a SlackHyperClient,
    channel: &'a SlackChannelId,
    thread_ts: &'a SlackTs,
    placeholder_ts: Option<&'a SlackTs>,
    text: &'a str,
) -> SendFuture<'a> {
    Box::pin(async move {
        let first_attempt = match placeholder_ts {
            Some(ts) => update_message(client, channel, ts, text)
                .await
                .map(|_| ts.clone()),
            None => post_reply(client, channel, thread_ts, text).await,
        };
        match first_attempt {
            Ok(ts) => Ok(ts),
            Err(e) if is_msg_too_long_error(e.as_ref()) => {
                let Some((head, tail)) = split_in_half(text) else {
                    return Err(e);
                };
                warn!(
                    bytes = text.len(),
                    head_bytes = head.len(),
                    tail_bytes = tail.len(),
                    "msg_too_long; splitting and retrying"
                );
                let _ = send_with_overflow_recovery(
                    client,
                    channel,
                    thread_ts,
                    placeholder_ts,
                    &head,
                )
                .await?;
                send_with_overflow_recovery(client, channel, thread_ts, None, &tail).await
            }
            Err(e) => Err(e),
        }
    })
}

/// Post a `<@user> _done_` reply in the thread when the turn took longer than
/// `PING_THRESHOLD`. `chat.update` does not fire mention notifications, so a
/// fresh `chat.postMessage` is the only way to actually ping. Skipped for
/// quick turns to keep the thread clean. Best-effort: a failure is logged.
async fn maybe_ping_done(
    client: &SlackHyperClient,
    channel: &SlackChannelId,
    thread_ts: &SlackTs,
    user_id: &str,
    elapsed: std::time::Duration,
) {
    if elapsed < PING_THRESHOLD {
        return;
    }
    let text = format!("<@{}> _done_", user_id);
    if let Err(e) = post_reply(client, channel, thread_ts, &text).await {
        warn!(error = %e, "ping-done reply failed");
    }
}

/// Format an in-flight update: the accumulated text so far, capped to fit in a
/// Slack message, with a streaming-indicator suffix. The final post is handled
/// separately by the caller using `truncate_for_slack` once the turn completes.
fn format_interim(text: &str, parts_committed: usize) -> String {
    let suffix = if parts_committed == 0 {
        "\n\n_…streaming_".to_string()
    } else {
        format!("\n\n_(part {}, streaming…)_", parts_committed + 1)
    };
    let budget = SLACK_MAX_TEXT.saturating_sub(suffix.len());
    if text.len() <= budget {
        let mut out = String::with_capacity(text.len() + suffix.len());
        out.push_str(text);
        out.push_str(&suffix);
        return out;
    }
    let mut end = budget;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + suffix.len());
    out.push_str(&text[..end]);
    out.push_str(&suffix);
    out
}

/// What `stream_updater` reports back so the caller can land the final post
/// on the right message and continue the part numbering.
struct StreamerOutcome {
    /// The most recent placeholder still showing `_…streaming_` (or `None` if
    /// a rollover finalized perfectly at end-of-stream and no new placeholder
    /// was opened).
    current_ts: Option<SlackTs>,
    /// Number of messages the streamer already finalized as `_(part N)_`.
    parts_committed: usize,
    /// Total bytes of `claude_result.text` already delivered to those
    /// finalized messages — the caller's final post starts at this offset.
    bytes_committed: usize,
}

/// Consume text chunks from `rx`, call `chat.update` on the placeholder, and
/// roll over into a new threaded message once the in-flight body crosses
/// `STREAM_ROLLOVER`. Subsequent chunks coalesce on a 1.5 s debounce (well
/// under Slack's Tier 3 ~50/min/channel limit). Final post is the caller's
/// responsibility — this task exits when all senders drop.
async fn stream_updater(
    client: Arc<SlackHyperClient>,
    channel: SlackChannelId,
    initial_ts: SlackTs,
    thread_ts: SlackTs,
    mut rx: tokio::sync::mpsc::Receiver<String>,
) -> StreamerOutcome {
    use tokio::time::{sleep_until, Duration, Instant};
    const DEBOUNCE: Duration = Duration::from_millis(1500);

    let mut current_ts: Option<SlackTs> = Some(initial_ts);
    let mut accumulated = String::new();
    let mut parts_committed: usize = 0;
    let mut bytes_committed: usize = 0;
    let mut last_post: Option<Instant> = None;
    let mut pending = false;

    loop {
        let deadline = if pending {
            Some(match last_post {
                Some(t) => t + DEBOUNCE,
                None => Instant::now(),
            })
        } else {
            None
        };

        tokio::select! {
            maybe_chunk = rx.recv() => {
                match maybe_chunk {
                    Some(chunk) => {
                        accumulated.push_str(&chunk);
                        pending = true;
                        if current_ts.is_none() {
                            match post_placeholder(&client, &channel, &thread_ts).await {
                                Ok(ts) => current_ts = Some(ts),
                                Err(e) => warn!(error = %e, "rollover placeholder post failed"),
                            }
                        }
                    }
                    None => break,
                }
            }
            _ = async {
                match deadline {
                    Some(d) => sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                while accumulated.len() > STREAM_ROLLOVER {
                    let window = &accumulated[..STREAM_ROLLOVER];
                    let cut = window
                        .rfind("\n\n").map(|i| i + 2)
                        .or_else(|| window.rfind('\n').map(|i| i + 1))
                        .unwrap_or_else(|| {
                            let mut e = STREAM_ROLLOVER;
                            while !accumulated.is_char_boundary(e) {
                                e -= 1;
                            }
                            e
                        });
                    let part_n = parts_committed + 1;
                    let part_text = accumulated[..cut].trim_end();
                    let converted = crate::mrkdwn::to_slack_mrkdwn(part_text);
                    let final_label = format!("{}\n\n_(part {})_", converted, part_n);
                    if let Some(ref ts) = current_ts {
                        if let Err(e) = update_message(&client, &channel, ts, &final_label).await {
                            warn!(error = %e, part = part_n, "rollover finalize failed");
                        }
                    }
                    parts_committed = part_n;
                    bytes_committed += cut;
                    accumulated = accumulated[cut..].to_string();
                    if !accumulated.is_empty() {
                        match post_placeholder(&client, &channel, &thread_ts).await {
                            Ok(ts) => current_ts = Some(ts),
                            Err(e) => {
                                warn!(error = %e, "rollover placeholder post failed");
                                current_ts = None;
                                break;
                            }
                        }
                    } else {
                        current_ts = None;
                    }
                }
                if let Some(ref ts) = current_ts {
                    let converted = crate::mrkdwn::to_slack_mrkdwn(&accumulated);
                    let interim = format_interim(&converted, parts_committed);
                    if let Err(e) = update_message(&client, &channel, ts, &interim).await {
                        warn!(error = %e, "interim slack update failed");
                    }
                }
                last_post = Some(Instant::now());
                pending = false;
            }
        }
    }

    StreamerOutcome { current_ts, parts_committed, bytes_committed }
}

fn on_error(
    err: Box<dyn std::error::Error + Send + Sync>,
    _client: Arc<SlackHyperClient>,
    _state: SlackClientEventsUserState,
) -> HttpStatusCode {
    warn!(error = %err, "slack listener error");
    HttpStatusCode::OK
}

/// Spawn `caffeinate -is -w <our-pid>` as a detached child so macOS doesn't
/// sleep the daemon's WebSocket while we're running. `-i` blocks idle sleep,
/// `-s` blocks system sleep on AC; that's the minimum needed to keep Socket
/// Mode connected. We deliberately omit `-d` (display sleep) and `-m` (disk
/// idle): the daemon never touches the display, and Apple Silicon NVMe SSDs
/// have no meaningful idle state to preserve. Dropping them saves ~3-5 W
/// during user-away periods.
///
/// caffeinate exits automatically when our PID dies, so no explicit teardown
/// is needed. Best-effort: a failure is logged but doesn't stop the daemon.
fn spawn_caffeinate() {
    let pid = std::process::id().to_string();
    let result = std::process::Command::new("caffeinate")
        .args(["-is", "-w", &pid])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match result {
        Ok(_) => info!("caffeinate started; system sleep prevented while daemon is up"),
        Err(e) => warn!(error = %e, "failed to start caffeinate; system may sleep the daemon"),
    }
}

fn sessions_state_path() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    Ok(crate::config::config_dir()?.join("sessions.json"))
}

fn read_secret(
    env_var: &str,
    file_value: Option<&str>,
    label: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(t) = std::env::var(env_var) {
        if !t.is_empty() {
            info!(label, "using token from environment");
            return Ok(t);
        }
    }
    match file_value {
        Some(t) if !t.is_empty() => {
            info!(label, "using token from credentials file");
            Ok(t.to_string())
        }
        _ => Err(format!(
            "no {} token found — run `slack-sessions setup` or set {}",
            label, env_var
        )
        .into()),
    }
}
