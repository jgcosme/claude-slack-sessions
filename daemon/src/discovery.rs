use serde::Deserialize;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// One claude session found on disk under `~/.claude/projects/`.
pub struct DiscoveredSession {
    pub session_id: String,
    /// True cwd recovered from the JSONL transcript's first event that
    /// records one. `None` if the file is empty or contains only events
    /// without a `cwd` field.
    pub cwd: Option<String>,
    pub mtime_unix: i64,
}

#[derive(Deserialize)]
struct CwdProbe {
    cwd: Option<String>,
}

/// Walk `~/.claude/projects/*/*.jsonl` and return up to `limit` of the
/// most-recent sessions, sorted by mtime descending. Best-effort:
/// returns an empty vec if HOME is missing or the projects dir doesn't
/// exist yet.
///
/// `total_count` in the result lets callers report "showing N of M".
/// Cwd is only resolved for the entries we actually return — reading
/// the JSONL preamble for every file on disk would cost seconds on
/// machines with thousands of historic sessions.
pub fn enumerate_recent_sessions(limit: usize) -> (Vec<DiscoveredSession>, usize) {
    let mut headers: Vec<(String, std::path::PathBuf, i64)> = Vec::new();
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
    headers.sort_by(|a, b| b.2.cmp(&a.2));
    headers.truncate(limit);
    let out = headers
        .into_iter()
        .map(|(session_id, fp, mtime_unix)| DiscoveredSession {
            session_id,
            cwd: read_cwd_from_jsonl(&fp),
            mtime_unix,
        })
        .collect();
    (out, total)
}

/// Look up a session by id and return its cwd if found. Walks the
/// projects dir until it hits the matching JSONL.
pub fn find_session_cwd(session_id: &str) -> Option<PathBuf> {
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

fn read_cwd_from_jsonl(path: &std::path::Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path).ok()?;
    // The cwd is reliably present on the first user/assistant line, but
    // bookkeeping events (queue-operation etc.) without `cwd` come
    // first. Cap the scan so a corrupt transcript doesn't stall the
    // listing — 200 lines comfortably reaches the first user event.
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

/// Format a unix timestamp delta from now as a short human-readable
/// "Xm/Xh/Xd ago" string. Used for the !session list output.
pub fn relative_age(mtime_unix: i64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_age_seconds() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(relative_age(now - 5).ends_with("s ago"));
        assert!(relative_age(now - 120).ends_with("m ago"));
        assert!(relative_age(now - 7200).ends_with("h ago"));
        assert!(relative_age(now - 200_000).ends_with("d ago"));
    }

    #[test]
    fn enumerate_does_not_panic_on_missing_root() {
        // We can't easily isolate HOME, but the function must at least
        // return cleanly if the user has no ~/.claude/projects.
        let _ = enumerate_recent_sessions(30);
    }
}
