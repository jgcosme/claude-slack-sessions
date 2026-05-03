use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::allowlist::Allowlist;
use crate::credentials::Credentials;
use crate::projects::ProjectsRegistry;

const LAUNCHD_LABEL: &str = "io.thinkingmachines.slack-sessions";

pub fn run() -> Result<()> {
    println!("slack-sessions status");
    println!();

    let mut all_ok = true;

    println!("Binaries:");
    all_ok &= section_binaries();
    println!();

    println!("Tokens:");
    all_ok &= section_tokens();
    println!();

    println!("Config:");
    all_ok &= section_config();
    println!();

    println!("Daemon:");
    all_ok &= section_daemon();
    println!();

    if all_ok {
        println!("All checks passed.");
        Ok(())
    } else {
        // Non-zero so wrappers (slash commands, scripts) surface the failure.
        std::process::exit(1);
    }
}

fn section_binaries() -> bool {
    let mut ok = true;
    let cli = std::env::current_exe().ok();
    match &cli {
        Some(p) => println!("  [ok]   slack-sessions     {}", p.display()),
        None => {
            println!("  [FAIL] slack-sessions     could not locate own binary");
            ok = false;
        }
    }

    if let Some(cli) = cli.as_ref() {
        let daemon = cli.parent().map(|p| p.join("slack-sessionsd"));
        match daemon.filter(|p| p.exists()) {
            Some(d) => println!("  [ok]   slack-sessionsd    {}", d.display()),
            None => {
                println!("  [FAIL] slack-sessionsd    not next to cli binary");
                println!("         → from the workspace root: cargo install --path daemon");
                ok = false;
            }
        }
    }

    match which("claude") {
        Some(p) => println!("  [ok]   claude             {}", p.display()),
        None => {
            println!("  [FAIL] claude             not on $PATH");
            println!("         → install Claude Code (code.claude.com/docs)");
            ok = false;
        }
    }
    ok
}

fn section_tokens() -> bool {
    let mut ok = true;
    let creds = Credentials::load().unwrap_or_default();
    match creds.app_token.as_deref() {
        Some(t) if !t.is_empty() => println!("  [ok]   app-level (xapp-)  {}", mask(t)),
        _ => {
            println!("  [FAIL] app-level (xapp-)  not stored");
            println!("         → slack-sessions setup");
            ok = false;
        }
    }
    let bot = creds.bot_token.clone();
    match bot.as_deref() {
        Some(t) if !t.is_empty() => println!("  [ok]   bot       (xoxb-)  {}", mask(t)),
        _ => {
            println!("  [FAIL] bot       (xoxb-)  not stored");
            println!("         → slack-sessions setup");
            ok = false;
        }
    }
    if let Some(token) = bot.filter(|t| !t.is_empty()) {
        match auth_test(&token) {
            Ok(team) => println!("  [ok]   auth.test          authenticated as team `{}`", team),
            Err(e) => {
                // Non-fatal: could be offline or curl missing. Surface as a warning.
                println!("  [warn] auth.test          {}", e);
            }
        }
    }
    ok
}

fn section_config() -> bool {
    let mut ok = true;
    let cfg = crate::config::config_dir().ok();
    match &cfg {
        Some(d) if d.exists() => println!("  [ok]   config dir         {}", d.display()),
        Some(d) => {
            println!(
                "  [warn] config dir         {} (created on first save)",
                d.display()
            );
        }
        None => {
            println!("  [FAIL] config dir         could not resolve home directory");
            ok = false;
        }
    }

    let allowlist = Allowlist::load().unwrap_or_default();
    let count = allowlist.slack_user_ids.len();
    if count == 0 {
        println!("  [warn] allowlist          empty — bot ignores everyone");
        println!("         → slack-sessions allow add <your-slack-user-id>");
    } else {
        println!("  [ok]   allowlist          {} user(s)", count);
    }

    let registry = ProjectsRegistry::load().unwrap_or_default();
    let project_count = registry.projects.len();
    let mut bad: Vec<(String, String)> = vec![];
    for (name, path) in &registry.projects {
        if !Path::new(path).exists() {
            bad.push((name.clone(), path.clone()));
        }
    }
    if bad.is_empty() {
        println!(
            "  [ok]   projects           {} registered, all paths exist",
            project_count
        );
    } else {
        println!(
            "  [warn] projects           {} registered, {} missing path(s):",
            project_count,
            bad.len()
        );
        for (name, path) in &bad {
            println!("           {} → {}", name, path);
        }
    }

    let default = registry.resolved_default();
    if registry.default_dir.is_some() {
        println!("  [ok]   default cwd        {}", default.display());
    } else {
        println!("  [ok]   default cwd        {} (using $HOME)", default.display());
    }
    ok
}

fn section_daemon() -> bool {
    let mut ok = true;
    let uid = match current_uid() {
        Ok(u) => u,
        Err(e) => {
            println!("  [FAIL] launchd service    could not determine uid: {}", e);
            return false;
        }
    };
    let target = format!("gui/{}/{}", uid, LAUNCHD_LABEL);
    let print_out = launchctl_print(&target);
    let pid = print_out.as_deref().ok().and_then(extract_pid);
    let last_exit = print_out.as_deref().ok().and_then(extract_last_exit);

    match (&print_out, pid) {
        (Ok(_), Some(p)) => println!("  [ok]   launchd service    loaded (pid {})", p),
        (Ok(_), None) => {
            println!("  [warn] launchd service    loaded but not running");
            if let Some(code) = last_exit {
                println!("         last exit code: {}", code);
            }
            println!("         → /slack-sessions:start");
        }
        (Err(_), _) => {
            println!("  [FAIL] launchd service    not loaded");
            println!("         → /slack-sessions:install");
            ok = false;
        }
    }

    if let Some(daemon_pid) = pid {
        match pgrep_child(daemon_pid, "caffeinate") {
            Some(cp) => println!("  [ok]   caffeinate         pid {} (system kept awake)", cp),
            None => {
                println!("  [warn] caffeinate         not found — system may sleep");
            }
        }
    }

    let log = log_dir().join("out.log");
    if log.exists() {
        println!("  [ok]   logs               {}", log.display());
    } else {
        println!(
            "  [warn] logs               {} (no logs yet — daemon may not have started)",
            log.display()
        );
    }
    ok
}

// ---------- helpers ----------

fn which(prog: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for entry in std::env::split_paths(&path) {
        let candidate = entry.join(prog);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn mask(s: &str) -> String {
    if s.len() < 12 {
        "***".into()
    } else {
        format!("{}…{}", &s[..8], &s[s.len() - 4..])
    }
}

/// Hit Slack's `auth.test` with the bot token to verify it's still valid.
/// Implemented via `curl` shell-out so the cli avoids an HTTP-client dep.
fn auth_test(bot_token: &str) -> Result<String, String> {
    let out = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "5",
            "-H",
            &format!("Authorization: Bearer {}", bot_token),
            "https://slack.com/api/auth.test",
        ])
        .output()
        .map_err(|e| format!("could not run curl: {}", e))?;
    if !out.status.success() {
        return Err(format!(
            "curl failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let body = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("non-JSON response from Slack: {} ({})", body.trim(), e))?;
    if json["ok"] == serde_json::Value::Bool(true) {
        let team = json["team"].as_str().unwrap_or("?").to_string();
        Ok(team)
    } else {
        let err = json["error"].as_str().unwrap_or("unknown error");
        Err(format!("Slack rejected the token: {}", err))
    }
}

fn current_uid() -> Result<u32, String> {
    let out = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|e| e.to_string())?;
    let s = String::from_utf8(out.stdout).map_err(|e| e.to_string())?;
    s.trim()
        .parse::<u32>()
        .map_err(|e| e.to_string())
}

fn launchctl_print(target: &str) -> Result<String, String> {
    let out = Command::new("launchctl")
        .args(["print", target])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn extract_pid(launchctl_out: &str) -> Option<u32> {
    for line in launchctl_out.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("pid =") {
            if let Ok(n) = rest.trim().parse::<u32>() {
                return Some(n);
            }
        }
    }
    None
}

fn extract_last_exit(launchctl_out: &str) -> Option<i32> {
    for line in launchctl_out.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("last exit code =") {
            if let Ok(n) = rest.trim().parse::<i32>() {
                return Some(n);
            }
        }
    }
    None
}

fn pgrep_child(parent_pid: u32, name: &str) -> Option<u32> {
    let out = Command::new("pgrep")
        .args(["-P", &parent_pid.to_string(), name])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    s.lines().next().and_then(|l| l.trim().parse::<u32>().ok())
}

fn log_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join("Library/Logs/slack-sessions"))
        .unwrap_or_default()
}
