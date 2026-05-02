mod projects;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use keyring::Entry;
use std::path::PathBuf;

use crate::projects::ProjectsRegistry;

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
    /// Manage the project registry used for !<name> selection in Slack
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
}

#[derive(Subcommand)]
enum ProjectAction {
    /// Add or update a named project (path must exist)
    Add {
        name: String,
        path: PathBuf,
    },
    /// List all registered projects and the default working directory
    List,
    /// Remove a named project
    Remove {
        name: String,
    },
    /// Set the default working directory used when no !<name> prefix is given
    SetDefault {
        path: PathBuf,
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
        Command::Project { action } => match action {
            ProjectAction::Add { name, path } => project_add(&name, &path),
            ProjectAction::List => project_list(),
            ProjectAction::Remove { name } => project_remove(&name),
            ProjectAction::SetDefault { path } => project_set_default(&path),
        },
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

fn project_add(name: &str, path: &PathBuf) -> Result<()> {
    ProjectsRegistry::validate_name(name).map_err(|e| anyhow!(e))?;
    let canonical = projects::canonicalize_dir(&path.to_string_lossy()).map_err(|e| anyhow!(e))?;
    let canonical_str = canonical.to_string_lossy().to_string();
    let mut reg = ProjectsRegistry::load().context("failed to load registry")?;
    let prior = reg
        .projects
        .insert(name.to_string(), canonical_str.clone());
    reg.save().context("failed to save registry")?;
    if prior.is_some() {
        println!("[ok] updated {} -> {}", name, canonical_str);
    } else {
        println!("[ok] added {} -> {}", name, canonical_str);
    }
    Ok(())
}

fn project_list() -> Result<()> {
    let reg = ProjectsRegistry::load().context("failed to load registry")?;
    let default = reg.resolved_default();
    println!("default working directory: {}", default.display());
    if reg.default_dir.is_none() {
        println!("  (using $HOME — set with `slack-sessions project set-default <path>`)");
    }
    println!();
    if reg.projects.is_empty() {
        println!("no registered projects.");
        println!("add one with: slack-sessions project add <name> <path>");
        return Ok(());
    }
    println!("registered projects:");
    let max_name = reg.projects.keys().map(|k| k.len()).max().unwrap_or(0);
    for (name, path) in &reg.projects {
        println!("  {:width$}  {}", name, path, width = max_name);
    }
    Ok(())
}

fn project_remove(name: &str) -> Result<()> {
    let mut reg = ProjectsRegistry::load().context("failed to load registry")?;
    if reg.projects.remove(name).is_none() {
        return Err(anyhow!("no project named {:?}", name));
    }
    reg.save().context("failed to save registry")?;
    println!("[ok] removed {}", name);
    Ok(())
}

fn project_set_default(path: &PathBuf) -> Result<()> {
    let canonical = projects::canonicalize_dir(&path.to_string_lossy()).map_err(|e| anyhow!(e))?;
    let canonical_str = canonical.to_string_lossy().to_string();
    let mut reg = ProjectsRegistry::load().context("failed to load registry")?;
    reg.default_dir = Some(canonical_str.clone());
    reg.save().context("failed to save registry")?;
    println!("[ok] default working directory: {}", canonical_str);
    Ok(())
}
