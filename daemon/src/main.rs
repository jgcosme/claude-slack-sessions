use slack_morphism::prelude::*;
use std::sync::{Arc, OnceLock};
use tracing::{info, warn};

const KEYRING_SERVICE: &str = "slack-sessions";
const KEYRING_APP_TOKEN_ACCOUNT: &str = "app-token";
const KEYRING_BOT_TOKEN_ACCOUNT: &str = "bot-token";

static BOT_TOKEN: OnceLock<SlackApiToken> = OnceLock::new();

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
    if !is_im {
        return Ok(());
    }
    if msg.sender.bot_id.is_some() {
        return Ok(());
    }
    if msg.subtype.is_some() {
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

    if let Err(e) = post_echo_reply(&client, &channel, &thread_ts, &text).await {
        warn!(error = %e, "echo reply failed");
    }

    Ok(())
}

async fn post_echo_reply(
    client: &SlackHyperClient,
    channel: &SlackChannelId,
    thread_ts: &SlackTs,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = BOT_TOKEN.get().ok_or("bot token not initialized")?;
    let session = client.open_session(token);
    let request = SlackApiChatPostMessageRequest::new(
        channel.clone(),
        SlackMessageContent::new().with_text(format!("echo: {}", text)),
    )
    .with_thread_ts(thread_ts.clone());
    session.chat_post_message(&request).await?;
    Ok(())
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
