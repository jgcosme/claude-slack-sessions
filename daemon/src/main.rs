mod claude;
mod session;

use slack_morphism::prelude::*;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tracing::{info, warn};

use crate::session::{now_unix, SessionStore};

const KEYRING_SERVICE: &str = "slack-sessions";
const KEYRING_APP_TOKEN_ACCOUNT: &str = "app-token";
const KEYRING_BOT_TOKEN_ACCOUNT: &str = "bot-token";
const STATE_PATH: &str = ".runtime/sessions.json";
const SLACK_MAX_TEXT: usize = 38_000;

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

    let store = Arc::new(SessionStore::load(PathBuf::from(STATE_PATH)).await?);
    info!(path = STATE_PATH, "session store loaded");
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
    let SlackEventCallbackBody::Message(msg) = event.event else {
        return Ok(());
    };

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

    info!(
        channel = %channel.0,
        ts = %ts.0,
        thread_ts = %thread_ts.0,
        text = %text,
        "DM received"
    );

    tokio::spawn(async move {
        if let Err(e) = handle_dm(client, channel, thread_ts, text).await {
            warn!(error = %e, "DM handling failed");
        }
    });

    Ok(())
}

async fn handle_dm(
    client: Arc<SlackHyperClient>,
    channel: SlackChannelId,
    thread_ts: SlackTs,
    text: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let store = SESSION_STORE
        .get()
        .ok_or("session store not initialized")?
        .clone();
    let entry_arc = store.get_or_create(&thread_ts.0).await;
    let mut entry = entry_arc.lock().await;

    let placeholder_ts = post_placeholder(&client, &channel, &thread_ts).await?;

    let claude_result =
        match crate::claude::run_turn(&text, entry.claude_session_id.as_deref()).await {
            Ok(r) => r,
            Err(e) => {
                let err_text = format!("_claude failed:_ {}", e);
                let _ = update_message(&client, &channel, &placeholder_ts, &err_text).await;
                return Err(e);
            }
        };

    let display_text = truncate_for_slack(&claude_result.text);
    update_message(&client, &channel, &placeholder_ts, &display_text).await?;

    if entry.claude_session_id.is_none() {
        entry.claude_session_id = claude_result.session_id;
    }
    entry.last_active_unix = now_unix();
    drop(entry);

    if let Err(e) = store.persist().await {
        warn!(error = %e, "failed to persist session store");
    }
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

fn on_error(
    err: Box<dyn std::error::Error + Send + Sync>,
    _client: Arc<SlackHyperClient>,
    _state: SlackClientEventsUserState,
) -> HttpStatusCode {
    warn!(error = %err, "slack listener error");
    HttpStatusCode::OK
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
