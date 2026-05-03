use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const LABEL: &str = "io.thinkingmachines.slack-sessions";
const PLIST_FILENAME: &str = "io.thinkingmachines.slack-sessions.plist";
const DAEMON_BINARY: &str = "slack-sessionsd";

/// Start the daemon, registering the launchd service if it isn't already.
///
/// Idempotent across all states:
///   - plist missing                → write plist, bootstrap
///   - plist exists, not loaded     → bootstrap from existing plist
///   - plist exists, loaded         → kickstart (no-op if already running)
///
/// `bootout` of any prior version is attempted before bootstrap so a stale
/// registration doesn't make `bootstrap` fail with "service already loaded".
pub fn start() -> Result<()> {
    let uid = current_uid()?;
    let target = format!("gui/{}/{}", uid, LABEL);

    // Already loaded? Just kickstart.
    if launchctl_capture(&["print", &target]).is_ok() {
        launchctl(&["kickstart", &target])?;
        println!("[ok] daemon kicked");
        return Ok(());
    }

    // Not loaded — make sure we have a plist, then bootstrap.
    let plist = plist_path()?;
    let log_dir = log_dir()?;

    // Always (re-)write the plist on cold start. This covers two cases:
    // first install (no plist), and post-update (plist points at an
    // out-of-date daemon path because the plugin moved between cache dirs).
    std::fs::create_dir_all(&log_dir).context("create log dir")?;
    if let Some(parent) = plist.parent() {
        std::fs::create_dir_all(parent).context("create LaunchAgents dir")?;
    }
    let daemon = find_daemon_binary()?;
    std::fs::write(&plist, render_plist(&daemon, &log_dir)?).context("write plist")?;
    println!("[ok] wrote {}", plist.display());

    // Bootout any stale registration first so bootstrap doesn't fail.
    let _ = launchctl(&["bootout", &target]);

    let plist_str = plist
        .to_str()
        .ok_or_else(|| anyhow!("plist path not valid utf-8"))?;
    launchctl(&["bootstrap", &format!("gui/{}", uid), plist_str])
        .context("launchctl bootstrap")?;
    println!("[ok] daemon loaded (label: {})", LABEL);
    println!("     logs: {}/out.log", log_dir.display());
    Ok(())
}

/// Stop the daemon and remove its launchd registration.
///
/// With `purge`, also wipes log files and `~/.config/slack-sessions/`
/// (tokens and other state).
pub fn stop(purge: bool) -> Result<()> {
    let uid = current_uid()?;
    let target = format!("gui/{}/{}", uid, LABEL);

    // Bootout — ignore failure, "not loaded" is a fine starting state.
    let _ = launchctl(&["bootout", &target]);

    let plist = plist_path()?;
    if plist.exists() {
        std::fs::remove_file(&plist).context("remove plist")?;
        println!("[ok] removed {}", plist.display());
    } else {
        println!("[--] no plist at {}", plist.display());
    }

    if purge {
        let log_dir = log_dir()?;
        if log_dir.exists() {
            std::fs::remove_dir_all(&log_dir).ok();
            println!("[ok] removed logs at {}", log_dir.display());
        }
        if let Some(dir) = crate::config::config_dir().ok().filter(|d| d.exists()) {
            std::fs::remove_dir_all(&dir).ok();
            println!("[ok] removed config at {} (tokens included)", dir.display());
        }
    }
    Ok(())
}

pub fn restart() -> Result<()> {
    let uid = current_uid()?;
    let target = format!("gui/{}/{}", uid, LABEL);
    // -k kills the running process, then starts a fresh one.
    launchctl(&["kickstart", "-k", &target]).context(
        "kickstart failed — if the daemon was never registered, run `slack-sessions service start` first",
    )?;
    println!("[ok] daemon restarted");
    Ok(())
}

pub fn logs(follow: bool, lines: u32) -> Result<()> {
    let log_path = log_dir()?.join("out.log");
    if !log_path.exists() {
        return Err(anyhow!(
            "no logs yet at {}\n(daemon may not have started; check `slack-sessions status`)",
            log_path.display()
        ));
    }
    let mut cmd = Command::new("tail");
    cmd.args(["-n", &lines.to_string()]);
    if follow {
        cmd.arg("-f");
    }
    cmd.arg(&log_path);
    let status = cmd.status().context("run tail")?;
    if !status.success() {
        return Err(anyhow!("tail exited with {}", status));
    }
    Ok(())
}

// ---------- helpers ----------

fn current_uid() -> Result<u32> {
    let out = Command::new("id")
        .arg("-u")
        .output()
        .context("run `id -u`")?;
    let s = String::from_utf8(out.stdout).context("uid output not utf-8")?;
    s.trim().parse().context("parse uid")
}

fn find_daemon_binary() -> Result<PathBuf> {
    let cli = std::env::current_exe().context("locate cli binary")?;
    let parent = cli
        .parent()
        .ok_or_else(|| anyhow!("cli binary has no parent dir"))?;
    let candidate = parent.join(DAEMON_BINARY);
    if !candidate.exists() {
        return Err(anyhow!(
            "daemon binary not found at {}\nbuild it first: from the workspace root run `cargo build --release`",
            candidate.display()
        ));
    }
    std::fs::canonicalize(&candidate).context("canonicalize daemon binary")
}

fn home() -> Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow!("no home directory"))
}

fn plist_path() -> Result<PathBuf> {
    Ok(home()?.join("Library/LaunchAgents").join(PLIST_FILENAME))
}

fn log_dir() -> Result<PathBuf> {
    Ok(home()?.join("Library/Logs/slack-sessions"))
}

fn render_plist(daemon_path: &Path, log_dir: &Path) -> Result<String> {
    let path_env = std::env::var("PATH").unwrap_or_default();
    let working_dir = home()?;
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{daemon}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>WorkingDirectory</key>
    <string>{cwd}</string>
    <key>StandardOutPath</key>
    <string>{logs}/out.log</string>
    <key>StandardErrorPath</key>
    <string>{logs}/err.log</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>{path}</string>
        <key>RUST_LOG</key>
        <string>info,slack_sessionsd=debug</string>
    </dict>
</dict>
</plist>
"#,
        label = LABEL,
        daemon = xml_escape(&daemon_path.display().to_string()),
        cwd = xml_escape(&working_dir.display().to_string()),
        logs = xml_escape(&log_dir.display().to_string()),
        path = xml_escape(&path_env),
    ))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn launchctl(args: &[&str]) -> Result<()> {
    let out = Command::new("launchctl")
        .args(args)
        .output()
        .context("invoke launchctl")?;
    if !out.status.success() {
        return Err(anyhow!(
            "launchctl {} failed:\nstdout: {}\nstderr: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

fn launchctl_capture(args: &[&str]) -> Result<String> {
    let out = Command::new("launchctl")
        .args(args)
        .output()
        .context("invoke launchctl")?;
    if !out.status.success() {
        return Err(anyhow!(
            "launchctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
