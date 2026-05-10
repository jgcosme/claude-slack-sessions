use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// Mirror of daemon/src/discovery.rs scoped to what the CLI needs. The two are
// independent on purpose: the daemon walk is hot-path (every `!sessions list`)
// and gets micro-optimized; the CLI walk is admin-time and prioritizes
// readable tabular output. Schema (cwd in the JSONL preamble, encoded dirs
// under `~/.claude/projects/`) is set by Claude Code, so divergence between
// daemon and CLI is bounded by upstream stability.

struct Discovered {
    session_id: String,
    cwd: Option<String>,
    mtime_unix: i64,
}

#[derive(Deserialize)]
struct CwdProbe {
    cwd: Option<String>,
}

fn enumerate_recent(limit: usize) -> (Vec<Discovered>, usize) {
    let mut headers: Vec<(String, PathBuf, i64)> = Vec::new();
    let root = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("projects"),
        None => return (Vec::new(), 0),
    };
    let project_dirs = match std::fs::read_dir(&root) {
        Ok(rd) => rd,
        Err(_) => return (Vec::new(), 0),
    };
    for entry in project_dirs.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let files = match std::fs::read_dir(&path) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for f in files.flatten() {
            let fp = f.path();
            if fp.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let session_id = match fp.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let mtime_unix = std::fs::metadata(&fp)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            headers.push((session_id, fp, mtime_unix));
        }
    }
    let total = headers.len();
    headers.sort_by_key(|h| std::cmp::Reverse(h.2));
    headers.truncate(limit);
    let out = headers
        .into_iter()
        .map(|(session_id, fp, mtime_unix)| Discovered {
            session_id,
            cwd: read_cwd_from_jsonl(&fp),
            mtime_unix,
        })
        .collect();
    (out, total)
}

fn find_session_cwd(session_id: &str) -> Option<PathBuf> {
    let root = dirs::home_dir()?.join(".claude").join("projects");
    let project_dirs = std::fs::read_dir(&root).ok()?;
    for entry in project_dirs.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let candidate = dir.join(format!("{}.jsonl", session_id));
        if candidate.exists() {
            return read_cwd_from_jsonl(&candidate).map(PathBuf::from);
        }
    }
    None
}

fn read_cwd_from_jsonl(path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(f);
    for line in reader.lines().take(200).map_while(Result::ok) {
        if !line.contains("\"cwd\"") {
            continue;
        }
        if let Ok(probe) = serde_json::from_str::<CwdProbe>(&line) {
            if let Some(c) = probe.cwd {
                return Some(c);
            }
        }
    }
    None
}

fn relative_age(mtime_unix: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let delta = (now - mtime_unix).max(0);
    if delta < 60 {
        format!("{}s ago", delta)
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}

pub fn list(limit: usize) -> Result<()> {
    let (sessions, total) = enumerate_recent(limit);
    if sessions.is_empty() {
        println!("[--] no claude sessions found under ~/.claude/projects/");
        return Ok(());
    }
    let max_id = sessions.iter().map(|s| s.session_id.len()).max().unwrap_or(36);
    let max_age = sessions
        .iter()
        .map(|s| relative_age(s.mtime_unix).len())
        .max()
        .unwrap_or(8);
    println!(
        "{:<id$}  {:<age$}  cwd",
        "session-id",
        "age",
        id = max_id,
        age = max_age
    );
    for s in &sessions {
        let cwd = s.cwd.as_deref().unwrap_or("(unknown)");
        println!(
            "{:<id$}  {:<age$}  {}",
            s.session_id,
            relative_age(s.mtime_unix),
            cwd,
            id = max_id,
            age = max_age,
        );
    }
    if total > sessions.len() {
        println!();
        println!("(showing {} of {}; pass --limit to widen)", sessions.len(), total);
    }
    println!();
    println!("resume with: slack-sessions sessions resume <session-id>");
    Ok(())
}

/// Resolve a session-id to its on-disk cwd, then `exec`'s
/// `claude --resume <session-id>` in that cwd — replacing this CLI process so
/// the user lands directly in claude with no extra wrapper between them.
///
/// Guarded by a TTY check: when invoked through Claude Code's `Bash` tool
/// (e.g., the `/slack-sessions:sessions` slash command), stdout is not a
/// terminal — exec'ing the interactive `claude` TUI in that environment
/// hangs at best, and at worst spawns a second claude that fights the
/// daemon's session lock (issue #3). When non-tty, print the resolved cwd
/// and a copy-paste command instead, then exit cleanly.
///
/// On non-unix platforms the exec falls back to spawn + wait.
pub fn resume(session_id: &str) -> Result<()> {
    let cwd = find_session_cwd(session_id).ok_or_else(|| {
        anyhow!(
            "no session `{}` found under ~/.claude/projects/ — run `slack-sessions sessions list` to see candidates",
            session_id
        )
    })?;
    let claude = which_claude().context("`claude` binary not found on PATH")?;

    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        println!("[ok] found session {}", session_id);
        println!("     cwd: {}", cwd.display());
        println!();
        println!("This command needs a real terminal — `claude --resume` is an interactive TUI.");
        println!("Open a terminal in that cwd and run:");
        println!();
        println!("    cd {} && claude --resume {}", cwd.display(), session_id);
        println!();
        println!("(or just: slack-sessions sessions resume {})", session_id);
        return Ok(());
    }

    eprintln!("[ok] resuming session {}", session_id);
    eprintln!("     cwd: {}", cwd.display());
    eprintln!("     exec: {} --resume {}", claude.display(), session_id);
    eprintln!();

    let mut cmd = std::process::Command::new(&claude);
    cmd.current_dir(&cwd).arg("--resume").arg(session_id);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        Err(anyhow!("exec failed: {}", err))
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().context("failed to spawn claude")?;
        if !status.success() {
            return Err(anyhow!("claude exited with {}", status));
        }
        Ok(())
    }
}

fn which_claude() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("claude");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
