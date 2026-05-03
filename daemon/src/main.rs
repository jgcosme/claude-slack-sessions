mod allowlist;
mod claude;
mod config;
mod projects;
mod session;

use slack_morphism::prelude::*;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tracing::{info, warn};

use crate::allowlist::Allowlist;
use crate::claude::ToolMode;
use crate::projects::ProjectsRegistry;
use crate::session::{now_unix, SessionStore};

const KEYRING_SERVICE: &str = "slack-sessions";
const KEYRING_APP_TOKEN_ACCOUNT: &str = "app-token";
const KEYRING_BOT_TOKEN_ACCOUNT: &str = "bot-token";
const SLACK_MAX_TEXT: usize = 38_000;
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

    let app_token_str = read_secret("SLACK_APP_TOKEN", KEYRING_APP_TOKEN_ACCOUNT, "app-level")?;
    let bot_token_str = read_secret("SLACK_BOT_TOKEN", KEYRING_BOT_TOKEN_ACCOUNT, "bot")?;
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
        tier = if is_allowlisted { "full" } else { "no-tools" },
        surface = "dm",
        channel = %channel.0,
        ts = %ts.0,
        thread_ts = %thread_ts.0,
        text = %text,
        "event"
    );

    if is_allowlisted {
        tokio::spawn(async move {
            if let Err(e) =
                handle_full_session(client, channel, thread_ts, text, user_id, Surface::Dm).await
            {
                warn!(error = %e, "DM handling failed");
            }
        });
    } else {
        tokio::spawn(async move {
            if let Err(e) = handle_no_tools_reply(client, channel, thread_ts, text, user_id).await {
                warn!(error = %e, "no-tools reply failed");
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
        tier = if is_allowlisted { "full" } else { "no-tools" },
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
            if let Err(e) = handle_no_tools_reply(client, channel, thread_ts, text, user_id).await {
                warn!(error = %e, "no-tools reply failed");
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

/// Handle a DM from a non-allowlisted Slack user. Spawns claude with
/// `--tools ""` (no filesystem, no Bash, no MCP, no network) and posts a
/// one-shot reply. No session resume, no thread state — every message is
/// answered fresh.
async fn handle_no_tools_reply(
    client: Arc<SlackHyperClient>,
    channel: SlackChannelId,
    thread_ts: SlackTs,
    text: String,
    user_id: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let started = std::time::Instant::now();
    let placeholder_ts = post_placeholder(&client, &channel, &thread_ts).await?;
    let cwd = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
    let updater = tokio::spawn(stream_updater(
        client.clone(),
        channel.clone(),
        placeholder_ts.clone(),
        rx,
    ));
    let result = match crate::claude::run_turn(&text, None, &cwd, ToolMode::None, Some(tx)).await {
        Ok(r) => r,
        Err(e) => {
            let _ = updater.await;
            let err_text = format!("_claude failed:_ {}", e);
            let _ = update_message(&client, &channel, &placeholder_ts, &err_text).await;
            return Err(e);
        }
    };
    let _ = updater.await;
    let display_text = truncate_for_slack(&result.text);
    update_message(&client, &channel, &placeholder_ts, &display_text).await?;
    maybe_ping_done(&client, &channel, &thread_ts, &user_id, started.elapsed()).await;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    Dm,
    ChannelMention,
}

async fn handle_full_session(
    client: Arc<SlackHyperClient>,
    channel: SlackChannelId,
    thread_ts: SlackTs,
    text: String,
    user_id: String,
    surface: Surface,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let started = std::time::Instant::now();
    let store = SESSION_STORE
        .get()
        .ok_or("session store not initialized")?
        .clone();
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

    // On first contact in a channel thread, fetch thread history so claude has
    // context for what's being discussed before the mention. Skipped for DMs
    // (no prior context to fetch) and for resumed sessions (claude already has
    // it from the previous turn).
    let prompt_text = if surface == Surface::ChannelMention
        && entry.claude_session_id.is_none()
    {
        match fetch_thread_replies(&client, &channel, &thread_ts).await {
            Ok(history) if history.len() > 1 => format_with_thread_context(&history, &prompt_text),
            Ok(_) => prompt_text,
            Err(e) => {
                warn!(error = %e, "failed to fetch thread context; proceeding without it");
                prompt_text
            }
        }
    } else {
        prompt_text
    };

    let placeholder_ts = post_placeholder(&client, &channel, &thread_ts).await?;

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
    let updater = tokio::spawn(stream_updater(
        client.clone(),
        channel.clone(),
        placeholder_ts.clone(),
        rx,
    ));
    let claude_result = match crate::claude::run_turn(
        &prompt_text,
        entry.claude_session_id.as_deref(),
        &resolved_cwd,
        ToolMode::Full,
        Some(tx),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            let _ = updater.await;
            let err_text = format!("_claude failed:_ {}", e);
            let _ = update_message(&client, &channel, &placeholder_ts, &err_text).await;
            return Err(e);
        }
    };
    let _ = updater.await;

    let display_text = truncate_for_slack(&claude_result.text);
    update_message(&client, &channel, &placeholder_ts, &display_text).await?;
    maybe_ping_done(&client, &channel, &thread_ts, &user_id, started.elapsed()).await;

    if entry.claude_session_id.is_none() {
        entry.claude_session_id = claude_result.session_id;
    }
    if is_first_turn {
        entry.cwd = Some(resolved_cwd.to_string_lossy().to_string());
    }
    entry.last_active_unix = now_unix();
    drop(entry);

    if let Err(e) = store.persist().await {
        warn!(error = %e, "failed to persist session store");
    }
    Ok(())
}

enum MagicCommand<'a> {
    List,
    Help,
    Add { name: &'a str, path: &'a str },
    Remove { name: &'a str },
    SetDefault { path: &'a str },
    Start { name: &'a str, message: &'a str },
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
fn format_with_thread_context(history: &[SlackHistoryMessage], current: &str) -> String {
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
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = BOT_TOKEN.get().ok_or("bot token not initialized")?;
    let session = client.open_session(token);
    let req = SlackApiChatPostMessageRequest::new(
        channel.clone(),
        SlackMessageContent::new().with_text(text.to_string()),
    )
    .with_thread_ts(thread_ts.clone());
    session.chat_post_message(&req).await?;
    Ok(())
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

fn truncate_for_slack(s: &str) -> String {
    if s.len() <= SLACK_MAX_TEXT {
        return s.to_string();
    }
    let mut end = SLACK_MAX_TEXT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut t = String::with_capacity(end + 32);
    t.push_str(&s[..end]);
    t.push_str("\n\n_[output truncated]_");
    t
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
fn format_interim(text: &str) -> String {
    const SUFFIX: &str = "\n\n_…streaming_";
    let budget = SLACK_MAX_TEXT.saturating_sub(SUFFIX.len());
    if text.len() <= budget {
        let mut out = String::with_capacity(text.len() + SUFFIX.len());
        out.push_str(text);
        out.push_str(SUFFIX);
        return out;
    }
    let mut end = budget;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + SUFFIX.len());
    out.push_str(&text[..end]);
    out.push_str(SUFFIX);
    out
}

/// Consume text chunks from `rx` and call `chat.update` on the placeholder
/// message with the accumulated text so far. The first chunk posts immediately
/// (replacing `_thinking..._`); subsequent chunks coalesce on a 1.5 s debounce
/// to stay well under Slack's Tier 3 rate limit (~50/min/channel). The final
/// post is the caller's responsibility — this task exits when all senders drop.
async fn stream_updater(
    client: Arc<SlackHyperClient>,
    channel: SlackChannelId,
    ts: SlackTs,
    mut rx: tokio::sync::mpsc::Receiver<String>,
) {
    use tokio::time::{sleep_until, Duration, Instant};
    const DEBOUNCE: Duration = Duration::from_millis(1500);

    let mut accumulated = String::new();
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
                let interim = format_interim(&accumulated);
                if let Err(e) = update_message(&client, &channel, &ts, &interim).await {
                    warn!(error = %e, "interim slack update failed");
                }
                last_post = Some(Instant::now());
                pending = false;
            }
        }
    }
}

fn on_error(
    err: Box<dyn std::error::Error + Send + Sync>,
    _client: Arc<SlackHyperClient>,
    _state: SlackClientEventsUserState,
) -> HttpStatusCode {
    warn!(error = %err, "slack listener error");
    HttpStatusCode::OK
}

/// Spawn `caffeinate -dimsu -w <our-pid>` as a detached child so macOS doesn't
/// sleep while the daemon is running. caffeinate exits automatically when our
/// PID dies, so no explicit teardown is needed. Best-effort: a failure is
/// logged but doesn't stop the daemon.
fn spawn_caffeinate() {
    let pid = std::process::id().to_string();
    let result = std::process::Command::new("caffeinate")
        .args(["-dimsu", "-w", &pid])
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
    keyring_account: &str,
    label: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(t) = std::env::var(env_var) {
        if !t.is_empty() {
            info!(label, "using token from environment");
            return Ok(t);
        }
    }
    let entry = keyring::Entry::new(KEYRING_SERVICE, keyring_account)?;
    match entry.get_password() {
        Ok(t) => {
            info!(label, "using token from OS secret store");
            Ok(t)
        }
        Err(keyring::Error::NoEntry) => Err(format!(
            "no {} token found — run `slack-sessions setup` or set {}",
            label, env_var
        )
        .into()),
        Err(e) => Err(e.into()),
    }
}
