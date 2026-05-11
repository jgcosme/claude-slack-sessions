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
    /// Best available descriptor for the session: Claude's
    /// auto-generated `ai-title` when present, otherwise the latest
    /// user prompt (from `last-prompt`). `None` if neither is present.
    pub title: Option<String>,
    pub mtime_unix: i64,
}

#[derive(Deserialize)]
struct CwdProbe {
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct TitleProbe {
    #[serde(rename = "aiTitle")]
    ai_title: Option<String>,
}

#[derive(Deserialize)]
struct LastPromptProbe {
    #[serde(rename = "lastPrompt")]
    last_prompt: Option<String>,
}

/// Walk `~/.claude/projects/*/*.jsonl` and return up to `limit` of the
/// most-recent **interactive** sessions, sorted by mtime descending.
/// Best-effort: returns an empty vec if HOME is missing or the projects
/// dir doesn't exist yet.
///
/// "Interactive" means the transcript contains at least one event with
/// `entrypoint == "cli"` — i.e. a human resumed it in a terminal at
/// some point. Pure `sdk-cli` transcripts (one-shot daemon turns,
/// obsidian-memory gate spawns, hook helpers, etc.) are excluded
/// because they're not things a user would meaningfully want to
/// browse, resume, or import via `!sessions resume`.
///
/// The returned `total_count` is the number of *interactive* matches
/// found across the candidate pool (so "…and N more" can be reported),
/// NOT the raw transcript count.
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
    headers.sort_by_key(|h| std::cmp::Reverse(h.2));

    // Bound scan work: on machines with thousands of sdk-cli noise
    // transcripts, walking every file just to find the few interactive
    // ones gets expensive. The most-recent 2000 covers months of
    // ordinary use; older CLI sessions you'd resume by id, not browse.
    const MAX_SCAN: usize = 2000;
    let mut out = Vec::new();
    let mut interactive_matches: usize = 0;
    for (session_id, fp, mtime_unix) in headers.into_iter().take(MAX_SCAN) {
        let (cwd, title, has_cli) = read_session_meta_from_jsonl(&fp);
        if !has_cli {
            continue;
        }
        interactive_matches += 1;
        if out.len() < limit {
            out.push(DiscoveredSession {
                session_id,
                cwd,
                title,
                mtime_unix,
            });
        }
    }
    (out, interactive_matches)
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
            let (cwd, _, _) = read_session_meta_from_jsonl(&candidate);
            return cwd.map(PathBuf::from);
        }
    }
    None
}

/// Single-pass scan of the transcript preamble for cwd, a usable
/// title, and whether the transcript contains any interactive (`cli`)
/// event. Returns `(cwd, title, has_cli_entry)`.
///
/// Title preference: `ai-title` (Claude's auto-summary) wins when
/// present; otherwise we fall back to the most recent `last-prompt`
/// record (the user's latest prompt verbatim). Short one-shot sessions
/// never get an ai-title, so the fallback is what makes the list
/// useful in practice.
///
/// `has_cli_entry` is `true` iff at least one event in the scanned
/// window has `entrypoint == "cli"`. This filters out the per-prompt
/// `sdk-cli` spawn noise (gate, daemon turns, hooks) while keeping
/// real interactive sessions AND mixed-mode sessions (Slack-started,
/// later resumed in a terminal).
///
/// Cap the scan at 200 lines so a corrupt transcript doesn't stall.
/// Short sessions fit easily; for long sessions we'll usually see
/// `ai-title` early.
fn read_session_meta_from_jsonl(
    path: &std::path::Path,
) -> (Option<String>, Option<String>, bool) {
    use std::io::{BufRead, BufReader};
    let Ok(f) = std::fs::File::open(path) else {
        return (None, None, false);
    };
    let reader = BufReader::new(f);
    let mut cwd: Option<String> = None;
    let mut ai_title: Option<String> = None;
    let mut last_prompt: Option<String> = None;
    let mut has_cli_entry = false;
    for line in reader.lines().take(200).map_while(Result::ok) {
        if cwd.is_none() && line.contains("\"cwd\"") {
            if let Ok(probe) = serde_json::from_str::<CwdProbe>(&line) {
                if let Some(c) = probe.cwd {
                    cwd = Some(c);
                }
            }
        }
        if ai_title.is_none() && line.contains("\"aiTitle\"") {
            if let Ok(probe) = serde_json::from_str::<TitleProbe>(&line) {
                if let Some(t) = probe.ai_title {
                    ai_title = Some(t);
                }
            }
        }
        // Keep overwriting last_prompt — we want the latest one in the
        // file, which reflects what the user was most recently working on.
        if line.contains("\"lastPrompt\"") {
            if let Ok(probe) = serde_json::from_str::<LastPromptProbe>(&line) {
                if let Some(p) = probe.last_prompt {
                    last_prompt = Some(p);
                }
            }
        }
        if !has_cli_entry && line.contains("\"entrypoint\":\"cli\"") {
            has_cli_entry = true;
        }
    }
    let title = ai_title.or(last_prompt).map(clean_title);
    (cwd, title, has_cli_entry)
}

/// Tidy a raw title for one-line display: strip the slack-sessions
/// "USER MESSAGE: … JSON only:" wrapper that appears in spawned
/// transcripts, collapse whitespace, and cap length.
fn clean_title(raw: String) -> String {
    let s = raw.trim();
    // Strip leading "USER MESSAGE:" if present
    let s = s.strip_prefix("USER MESSAGE:").unwrap_or(s).trim();
    // Strip trailing "JSON only:" if present
    let s = s.strip_suffix("JSON only:").unwrap_or(s).trim();
    // Collapse internal whitespace runs (incl. newlines) to single spaces
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    // Cap so the list line stays readable in Slack
    const MAX_CHARS: usize = 80;
    if collapsed.chars().count() > MAX_CHARS {
        let truncated: String = collapsed.chars().take(MAX_CHARS).collect();
        format!("{}…", truncated)
    } else {
        collapsed
    }
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
