use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

const HEADER: &str = r#"// slack-sessions — thread state
//
// Per-thread session state, keyed by Slack thread_ts. Each entry holds:
//   • claude_session_id: passed to `claude --resume` on subsequent turns
//     so the model resumes the same conversation. Captured from claude's
//     stream-json output on the first turn.
//   • cwd: working directory the thread is bound to. Set on first turn
//     based on `!start <project>` / default registry; reused on replies.
//   • last_active_unix: epoch seconds of the most recent turn.
//
// Mutated by the daemon on every DM/mention; manual edits are safe but
// will likely be overwritten quickly. Removing an entry effectively
// starts a new claude session the next time someone posts in that
// thread (since claude_session_id will be missing). Deleting the file
// forgets all bound threads but does not affect projects.json or
// allowlist.json.
//
// Comments above are restored automatically on each write.
"#;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ThreadEntry {
    pub claude_session_id: Option<String>,
    pub last_active_unix: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct StoreFile {
    sessions: HashMap<String, ThreadEntry>,
}

pub struct SessionStore {
    threads: Mutex<HashMap<String, Arc<Mutex<ThreadEntry>>>>,
    state_path: PathBuf,
}

impl SessionStore {
    pub async fn load(state_path: PathBuf) -> std::io::Result<Self> {
        let exists = tokio::fs::try_exists(&state_path).await.unwrap_or(false);
        let threads = if exists {
            let raw = tokio::fs::read_to_string(&state_path).await?;
            let file: StoreFile = json5::from_str(&raw)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            file.sessions
                .into_iter()
                .map(|(k, v)| (k, Arc::new(Mutex::new(v))))
                .collect()
        } else {
            HashMap::new()
        };
        Ok(Self {
            threads: Mutex::new(threads),
            state_path,
        })
    }

    pub async fn get_or_create(&self, thread_ts: &str) -> Arc<Mutex<ThreadEntry>> {
        let mut map = self.threads.lock().await;
        map.entry(thread_ts.to_string())
            .or_insert_with(|| {
                Arc::new(Mutex::new(ThreadEntry {
                    claude_session_id: None,
                    last_active_unix: now_unix(),
                    cwd: None,
                }))
            })
            .clone()
    }

    pub async fn persist(&self) -> std::io::Result<()> {
        let snapshot = {
            let map = self.threads.lock().await;
            let mut file = StoreFile::default();
            for (k, entry_arc) in map.iter() {
                let entry = entry_arc.lock().await;
                file.sessions.insert(k.clone(), entry.clone());
            }
            file
        };
        let json = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(parent) = self.state_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let combined = format!("{}\n{}\n", HEADER.trim_end(), json);
        tokio::fs::write(&self.state_path, combined).await?;
        Ok(())
    }
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
