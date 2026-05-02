use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ThreadEntry {
    pub claude_session_id: Option<String>,
    pub last_active_unix: i64,
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
            let file: StoreFile = serde_json::from_str(&raw)
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
        let raw = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(parent) = self.state_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&self.state_path, raw).await?;
        Ok(())
    }
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
