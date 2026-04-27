use crate::config::Config;
use crate::model::{DocBlock, DocKey, TagName};
use dashmap::DashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Concurrent in-memory index of all known doc blocks for a workspace.
///
/// Held by the daemon as `Arc<Index>` and shared between the LSP server,
/// the MCP server and the SSE endpoint. Clients may use [`Index::revision`]
/// as an opaque monotonic version for if-modified-since polling.
pub struct Index {
    blocks: DashMap<DocKey, DocBlock>,
    by_path: DashMap<PathBuf, HashSet<DocKey>>,
    by_tag: DashMap<TagName, HashSet<DocKey>>,
    workspace_root: PathBuf,
    config: Arc<Config>,
    revision: AtomicU64,
}

impl Index {
    pub fn new(workspace_root: PathBuf, config: Arc<Config>) -> Self {
        Self {
            blocks: DashMap::new(),
            by_path: DashMap::new(),
            by_tag: DashMap::new(),
            workspace_root,
            config,
            revision: AtomicU64::new(0),
        }
    }

    pub const fn workspace_root(&self) -> &PathBuf {
        &self.workspace_root
    }

    pub const fn config(&self) -> &Arc<Config> {
        &self.config
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn get(&self, key: &DocKey) -> Option<DocBlock> {
        self.blocks.get(key).map(|r| r.value().clone())
    }

    pub fn keys_for_path(&self, path: &PathBuf) -> HashSet<DocKey> {
        self.by_path
            .get(path)
            .map(|r| r.value().clone())
            .unwrap_or_default()
    }

    pub fn keys_with_tag(&self, tag: &str) -> HashSet<DocKey> {
        self.by_tag
            .get(tag)
            .map(|r| r.value().clone())
            .unwrap_or_default()
    }

    /// Inserts or replaces a block, updating reverse indexes and bumping revision.
    pub fn upsert(&self, block: DocBlock) {
        let key = block.key.clone();
        let path = block.meta.path.clone();
        let tag_names: Vec<TagName> = block.tags.keys().cloned().collect();

        if let Some(prev) = self.blocks.insert(key.clone(), block) {
            self.scrub_reverse_indexes(&prev);
        }

        self.by_path.entry(path).or_default().insert(key.clone());
        for tag in tag_names {
            self.by_tag.entry(tag).or_default().insert(key.clone());
        }

        self.revision.fetch_add(1, Ordering::Release);
    }

    pub fn remove(&self, key: &DocKey) -> Option<DocBlock> {
        let removed = self.blocks.remove(key).map(|(_, v)| v);
        if let Some(block) = &removed {
            self.scrub_reverse_indexes(block);
            self.revision.fetch_add(1, Ordering::Release);
        }
        removed
    }

    fn scrub_reverse_indexes(&self, block: &DocBlock) {
        if let Some(mut entry) = self.by_path.get_mut(&block.meta.path) {
            entry.remove(&block.key);
        }
        for tag in block.tags.keys() {
            if let Some(mut entry) = self.by_tag.get_mut(tag) {
                entry.remove(&block.key);
            }
        }
    }
}
