use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use keyring::Entry;

const SERVICE: &str = "slack-sessions";
const APP_TOKEN_ACCOUNT: &str = "app-token";
const BOT_TOKEN_ACCOUNT: &str = "bot-token";

#[derive(Parser)]
#[command(
    name = "slack-sessions",
    version,
    about = "Drive Claude Code from Slack — one session per thread"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Store Slack tokens in the OS secret store
    Setup {
        /// Verify stored tokens, don't prompt
        #[arg(long)]
        check: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Setup { check } => {
            if check {
                setup_check()
            } else {
                setup_interactive()
            }
        }
    }
}

fn setup_interactive() -> Result<()> {
    println!("slack-sessions setup");
    println!();
    println!("Two tokens are needed (find both at https://api.slack.com/apps -> your app):");
    println!("  app-level (xapp-1-...)  Basic Information -> App-Level Tokens");
    println!("                          scope: connections:write");
    println!("  bot       (xoxb-...)    OAuth & Permissions -> Bot User OAuth Token");
    println!("                          scopes: chat:write, im:history, im:read");
    println!();
    println!("Press Enter at a prompt to keep an existing stored value.");
    println!();

    prompt_and_store("app-level token", "xapp-", APP_TOKEN_ACCOUNT)?;
    prompt_and_store("bot token", "xoxb-", BOT_TOKEN_ACCOUNT)?;

    println!();
    println!("[ok] verify with: slack-sessions setup --check");
    Ok(())
}

fn prompt_and_store(label: &str, prefix: &str, account: &str) -> Result<()> {
    let entry = Entry::new(SERVICE, account).context("failed to open keyring entry")?;
    let existing = entry.get_password().ok();

    let prompt = if existing.is_some() {
        format!("{} (Enter to keep existing): ", label)
    } else {
        format!("{} (input hidden): ", label)
    };

    let token = rpassword::prompt_password(&prompt).context("failed to read from terminal")?;
    let token = token.trim();

    if token.is_empty() {
        if existing.is_some() {
            println!("[ok] kept existing {}", label);
            return Ok(());
        }
        return Err(anyhow!("no {} provided", label));
    }

    if !token.starts_with(prefix) {
        return Err(anyhow!(
            "{} does not look right (expected `{}...` prefix)",
            label,
            prefix
        ));
    }
    if token.len() < 20 {
        return Err(anyhow!("{} too short to be valid", label));
    }

    entry
        .set_password(token)
        .context("failed to write token to OS secret store")?;
    println!("[ok] stored {}", label);
    Ok(())
}

fn setup_check() -> Result<()> {
    let mut all_present = true;
    for (label, account) in [
        ("app-level (xapp-)", APP_TOKEN_ACCOUNT),
        ("bot       (xoxb-)", BOT_TOKEN_ACCOUNT),
    ] {
        let entry = Entry::new(SERVICE, account).context("failed to open keyring entry")?;
        match entry.get_password() {
            Ok(t) => println!("[ok] {}: {}", label, mask(&t)),
            Err(keyring::Error::NoEntry) => {
                println!("[--] {}: not stored", label);
                all_present = false;
            }
            Err(e) => return Err(e).context("failed to read keyring entry"),
        }
    }
    if !all_present {
        return Err(anyhow!("missing tokens — run `slack-sessions setup`"));
    }
    Ok(())
}

fn mask(s: &str) -> String {
    if s.len() < 12 {
        "***".into()
    } else {
        format!("{}...{}", &s[..8], &s[s.len() - 4..])
    }
}
