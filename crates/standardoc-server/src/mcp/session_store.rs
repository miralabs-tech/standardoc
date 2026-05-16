use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::transport::streamable_http_server::session::store::{
    SessionState, SessionStore, SessionStoreError,
};
use tokio::sync::RwLock;

/// Persisted MCP session store backed by a JSON sidecar file at
/// `<workspace>/.standardoc/mcp-sessions.json`.
///
/// rmcp 1.7's `StreamableHttpServerConfig::session_store` calls into
/// `load(session_id)` whenever a request lands on an instance with no
/// in-memory session. By persisting the `initialize` handshake state to
/// disk, we let the next daemon process (post-restart, post-rebuild,
/// post-migration) transparently restore Claude Code's session — no
/// manual reconnect dance needed.
///
/// The store is `Send + Sync + 'static` via interior mutability
/// (`tokio::sync::RwLock` over a `HashMap`). Every mutating call
/// (`store`, `delete`) flushes the cache to disk atomically (`.tmp`
/// rename) so a daemon crash mid-write leaves the previous file intact.
///
/// Total state size is tiny — a few KB max even with many concurrent
/// clients — so the JSON-file backend is overkill-free. A SQLite
/// sidecar would be possible later if we need richer querying / TTL.
pub(crate) struct FileSessionStore {
    path: PathBuf,
    cache: Arc<RwLock<HashMap<String, SessionState>>>,
}

impl FileSessionStore {
    /// Build a store rooted at `<workspace_root>/.standardoc/mcp-sessions.json`.
    /// If the file already exists and parses cleanly, its contents prime
    /// the in-memory cache (post-restart recovery). A missing / corrupt
    /// file starts empty and is best-effort overwritten on next mutation.
    pub(crate) fn new(workspace_root: &Path) -> Self {
        let path = workspace_root
            .join(".standardoc")
            .join("mcp-sessions.json");
        let cache = load_from_disk(&path).unwrap_or_default();
        Self {
            path,
            cache: Arc::new(RwLock::new(cache)),
        }
    }

    /// Atomic write of the cache to disk : serialize to JSON, write to a
    /// `.tmp` sibling, then `rename` over the canonical path. Best-effort
    /// — caller swallows errors and lets the daemon continue. Runs the
    /// blocking IO under `spawn_blocking` so the runtime stays responsive
    /// (we already depend on tokio without the `fs` feature).
    async fn flush(&self) -> Result<(), SessionStoreError> {
        let snapshot: HashMap<String, SessionState> = self.cache.read().await.clone();
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<(), SessionStoreError> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let json = serde_json::to_vec_pretty(&snapshot)?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, json)?;
            std::fs::rename(&tmp, &path)?;
            Ok(())
        })
        .await
        .map_err(|e| Box::new(e) as SessionStoreError)??;
        Ok(())
    }
}

fn load_from_disk(path: &Path) -> Option<HashMap<String, SessionState>> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[async_trait]
impl SessionStore for FileSessionStore {
    async fn load(&self, session_id: &str) -> Result<Option<SessionState>, SessionStoreError> {
        Ok(self.cache.read().await.get(session_id).cloned())
    }

    async fn store(
        &self,
        session_id: &str,
        state: &SessionState,
    ) -> Result<(), SessionStoreError> {
        {
            let mut g = self.cache.write().await;
            g.insert(session_id.to_string(), state.clone());
        }
        self.flush().await
    }

    async fn delete(&self, session_id: &str) -> Result<(), SessionStoreError> {
        {
            let mut g = self.cache.write().await;
            g.remove(session_id);
        }
        self.flush().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{ClientCapabilities, Implementation, InitializeRequestParams};
    use tempfile::TempDir;

    fn sample_state(name: &str) -> SessionState {
        let mut implementation = Implementation::default();
        implementation.name = name.to_string();
        SessionState::new(InitializeRequestParams::new(
            ClientCapabilities::default(),
            implementation,
        ))
    }

    #[tokio::test]
    async fn store_and_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new(tmp.path());
        let state = sample_state("test-client");
        store.store("sess-1", &state).await.unwrap();
        let got = store.load("sess-1").await.unwrap().expect("present");
        assert_eq!(got.initialize_params.client_info.name, "test-client");
    }

    #[tokio::test]
    async fn delete_removes_entry() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new(tmp.path());
        let state = sample_state("c");
        store.store("sess-x", &state).await.unwrap();
        store.delete("sess-x").await.unwrap();
        assert!(store.load("sess-x").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn persisted_state_survives_new_instance() {
        let tmp = TempDir::new().unwrap();
        {
            let store = FileSessionStore::new(tmp.path());
            store.store("persisted", &sample_state("cc")).await.unwrap();
        }
        // Simulate daemon restart — fresh instance reads the same file.
        let reborn = FileSessionStore::new(tmp.path());
        let got = reborn
            .load("persisted")
            .await
            .unwrap()
            .expect("must survive process death");
        assert_eq!(got.initialize_params.client_info.name, "cc");
    }

    #[tokio::test]
    async fn missing_session_is_none() {
        let tmp = TempDir::new().unwrap();
        let store = FileSessionStore::new(tmp.path());
        assert!(store.load("never-seen").await.unwrap().is_none());
    }
}
