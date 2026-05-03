use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const LABEL: &str = "io.thinkingmachines.slack-sessions";
const PLIST_FILENAME: &str = "io.thinkingmachines.slack-sessions.plist";
const DAEMON_BINARY: &str = "slack-sessionsd";

pub fn install() -> Result<()> {
    let daemon_path = find_daemon_binary()?;
    let log_dir = log_dir()?;
    std::fs::create_dir_all(&log_dir).context("create log dir")?;

    let plist_path = plist_path()?;
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent).context("create LaunchAgents dir")?;
    }

    let plist_contents = render_plist(&daemon_path, &log_dir)?;
    std::fs::write(&plist_path, plist_contents).context("write plist")?;
    println!("[ok] wrote {}", plist_path.display());

    // bootstrap: load and start. If already loaded, error — bootout first.
    let uid = current_uid()?;
    let domain = format!("gui/{}", uid);
    let plist_str = plist_path
        .to_str()
        .ok_or_else(|| anyhow!("plist path not valid utf-8"))?;
    // If a previous version is loaded, bootout first so bootstrap doesn't fail.
    let _ = launchctl(&["bootout", &format!("{}/{}", domain, LABEL)]);
    launchctl(&["bootstrap", &domain, plist_str]).context("launchctl bootstrap")?;
    println!("[ok] daemon loaded (label: {})", LABEL);
    println!("     logs: {}/out.log", log_dir.display());
    println!("     status: slack-sessions service status");
    Ok(())
}

pub fn start() -> Result<()> {
    let uid = current_uid()?;
    let target = format!("gui/{}/{}", uid, LABEL);
    if launchctl_capture(&["print", &target]).is_ok() {
        // Already loaded — kickstart (no-op if running)
        launchctl(&["kickstart", &target])?;
        println!("[ok] daemon kicked");
        return Ok(());
    }
    // Not loaded — bootstrap from plist
    let plist_path = plist_path()?;
    if !plist_path.exists() {
        return Err(anyhow!(
            "not installed — run `slack-sessions service install` first"
        ));
    }
    let plist_str = plist_path
        .to_str()
        .ok_or_else(|| anyhow!("plist path not valid utf-8"))?;
    launchctl(&["bootstrap", &format!("gui/{}", uid), plist_str])?;
    println!("[ok] daemon loaded");
    Ok(())
}

pub fn stop() -> Result<()> {
    let uid = current_uid()?;
    let target = format!("gui/{}/{}", uid, LABEL);
    launchctl(&["bootout", &target]).context("launchctl bootout")?;
    println!("[ok] daemon stopped");
    Ok(())
}

pub fn restart() -> Result<()> {
    let uid = current_uid()?;
    let target = format!("gui/{}/{}", uid, LABEL);
    // -k kills the running process, then starts a fresh one.
    launchctl(&["kickstart", "-k", &target])?;
    println!("[ok] daemon restarted");
    Ok(())
}

pub fn status() -> Result<()> {
    let uid = current_uid()?;
    let target = format!("gui/{}/{}", uid, LABEL);
    match launchctl_capture(&["print", &target]) {
        Ok(out) => {
            let pid_line = out.lines().find(|l| l.trim().starts_with("pid ="));
            let state_line = out.lines().find(|l| l.trim().starts_with("state ="));
            let last_exit = out
                .lines()
                .find(|l| l.trim().starts_with("last exit code"));
            println!("[ok] daemon: loaded");
            if let Some(s) = state_line {
                println!("     {}", s.trim());
            }
            if let Some(p) = pid_line {
                println!("     {}", p.trim());
            } else {
                println!("     (no pid — not running)");
            }
            if let Some(e) = last_exit {
                println!("     {}", e.trim());
            }
            println!("     logs: {}", log_dir()?.join("out.log").display());
        }
        Err(_) => {
            println!("[--] daemon: not loaded");
            println!("     install with: slack-sessions service install");
        }
    }
    Ok(())
}

pub fn uninstall(purge: bool) -> Result<()> {
    let uid = current_uid()?;
    let target = format!("gui/{}/{}", uid, LABEL);
    // Ignore bootout failure (not loaded is fine).
    let _ = launchctl(&["bootout", &target]);
    let plist_path = plist_path()?;
    if plist_path.exists() {
        std::fs::remove_file(&plist_path).context("remove plist")?;
        println!("[ok] removed {}", plist_path.display());
    } else {
        println!("[--] no plist at {}", plist_path.display());
    }
    if purge {
        let log_dir = log_dir()?;
        if log_dir.exists() {
            std::fs::remove_dir_all(&log_dir).ok();
            println!("[ok] removed logs at {}", log_dir.display());
        }
        let config_dir = crate::config::config_dir().ok();
        if let Some(dir) = config_dir.filter(|d| d.exists()) {
            std::fs::remove_dir_all(&dir).ok();
            println!("[ok] removed config at {}", dir.display());
        }
        println!(
            "     (keyring tokens preserved; clear manually if desired:\n      security delete-generic-password -s slack-sessions -a app-token\n      security delete-generic-password -s slack-sessions -a bot-token)"
        );
    }
    Ok(())
}

pub fn logs(follow: bool, lines: u32) -> Result<()> {
    let log_path = log_dir()?.join("out.log");
    if !log_path.exists() {
        return Err(anyhow!(
            "no logs yet at {}\n(daemon may not have started; check `slack-sessions service status`)",
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
