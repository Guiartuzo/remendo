//! The verdict cache: the expensive half of a review session, persisted.
//!
//! Re-running the verdict pass on a relaunch is not merely a repeated charge,
//! it is **non-deterministic** — a second pass returns different verdicts over a
//! worktree that already contains round-one fixes, so Claude adjudicates
//! comments whose fix is already applied (design.md §13).
//!
//! What is deliberately **not** cached: the human's triage decisions and the
//! drafted replies. Those are cheap to redo and expensive to get subtly stale.
//!
//! The key is `(change id, revision)`, never the change id alone. A new patchset
//! means the cached verdicts describe code that no longer exists; keying on the
//! revision turns that from a correctness bug into a cache miss.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::WorkspaceError;

/// A change's cached verdict pass.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VerdictCache {
    pub change_id: String,
    /// The revision the verdicts were produced against. A mismatch is a miss.
    pub revision: String,
    /// Accumulated `total_cost_usd` across every turn of this session. This is
    /// what settles `open-decisions.md` Tier 5 — one real run prices a change.
    #[serde(default)]
    pub total_cost_usd: f64,
    /// The verdict payloads, keyed by comment id.
    ///
    /// Held as opaque JSON on purpose: the verdict schema belongs to §4, which
    /// is gated behind the `claude` re-probe (task 4.11). Typing it here would
    /// be implementing that section ahead of its gate. §4 replaces this with its
    /// own type; the cache format is versioned so that swap is a migration
    /// rather than a silent misread.
    #[serde(default)]
    pub verdicts: serde_json::Map<String, serde_json::Value>,
    /// Format version, so a later shape change can be detected rather than
    /// mis-parsed into plausible nonsense.
    #[serde(default = "current_version")]
    pub version: u32,
}

/// The cache format version this build writes.
const CACHE_VERSION: u32 = 1;

fn current_version() -> u32 {
    CACHE_VERSION
}

impl VerdictCache {
    /// An empty cache for a change at a revision.
    pub fn new(change_id: &str, revision: &str) -> Self {
        Self {
            change_id: change_id.to_string(),
            revision: revision.to_string(),
            total_cost_usd: 0.0,
            verdicts: serde_json::Map::new(),
            version: CACHE_VERSION,
        }
    }

    /// Whether this cache describes `revision` of `change_id`.
    ///
    /// ```
    /// # use remendo::workspace::VerdictCache;
    /// let cache = VerdictCache::new("12345", "sha3");
    /// assert!(cache.matches("12345", "sha3"));
    /// assert!(!cache.matches("12345", "sha4"), "a new patchset is a miss");
    /// ```
    pub fn matches(&self, change_id: &str, revision: &str) -> bool {
        self.version == CACHE_VERSION && self.change_id == change_id && self.revision == revision
    }

    /// Record a turn's cost.
    pub fn add_cost(&mut self, usd: f64) {
        self.total_cost_usd += usd;
    }

    /// Store one comment's verdict payload.
    pub fn put(&mut self, comment_id: &str, verdict: serde_json::Value) {
        self.verdicts.insert(comment_id.to_string(), verdict);
    }

    /// Read a comment's cached verdict.
    pub fn get(&self, comment_id: &str) -> Option<&serde_json::Value> {
        self.verdicts.get(comment_id)
    }

    /// Load a cache from `path`, returning `None` when it is absent or does not
    /// describe this `(change, revision)`.
    ///
    /// A corrupt or stale file is a miss, never an error: the worst case is
    /// paying for the pass again, and failing the launch over an unreadable
    /// cache would make a convenience into a liability.
    pub fn load(path: &Path, change_id: &str, revision: &str) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let cache: Self = serde_json::from_str(&text).ok()?;
        cache.matches(change_id, revision).then_some(cache)
    }

    /// Write the cache to `path`, creating its directory if needed.
    pub fn save(&self, path: &Path) -> Result<(), WorkspaceError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| WorkspaceError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let text = serde_json::to_string_pretty(self).expect("the cache is serializable");
        std::fs::write(path, text).map_err(|source| WorkspaceError::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("remendo-cache-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("verdicts.json")
    }

    fn verdict(text: &str) -> serde_json::Value {
        serde_json::json!({"verdict": text, "justification": "…", "depends_on": null})
    }

    #[test]
    fn a_fresh_cache_is_empty_and_free() {
        let cache = VerdictCache::new("12345", "sha3");
        assert_eq!(cache.total_cost_usd, 0.0);
        assert!(cache.verdicts.is_empty());
    }

    #[test]
    fn verdicts_and_cost_accumulate() {
        let mut cache = VerdictCache::new("12345", "sha3");
        cache.put("c1", verdict("agree"));
        cache.add_cost(0.14);
        cache.add_cost(0.06);
        assert_eq!(cache.get("c1").unwrap()["verdict"], "agree");
        assert!((cache.total_cost_usd - 0.20).abs() < 1e-9);
    }

    #[test]
    fn a_round_trip_through_disk_preserves_everything() {
        let path = temp_path("roundtrip");
        let mut cache = VerdictCache::new("12345", "sha3");
        cache.put("c1", verdict("disagree"));
        cache.add_cost(0.42);
        cache.save(&path).unwrap();

        let loaded = VerdictCache::load(&path, "12345", "sha3").expect("a hit");
        assert_eq!(loaded, cache);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The rule that makes the cache correct rather than a bug: a new patchset
    /// describes code that no longer exists.
    #[test]
    fn a_different_revision_is_a_miss() {
        let path = temp_path("revision-miss");
        VerdictCache::new("12345", "sha3").save(&path).unwrap();
        assert!(VerdictCache::load(&path, "12345", "sha3").is_some());
        assert!(VerdictCache::load(&path, "12345", "sha4").is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_different_change_is_a_miss() {
        let path = temp_path("change-miss");
        VerdictCache::new("12345", "sha3").save(&path).unwrap();
        assert!(VerdictCache::load(&path, "99999", "sha3").is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn an_absent_file_is_a_miss_not_an_error() {
        assert!(VerdictCache::load(Path::new("/nonexistent/verdicts.json"), "1", "s").is_none());
    }

    /// Failing a launch over an unreadable cache would turn a convenience into
    /// a liability; paying for the pass again is the right worst case.
    #[test]
    fn a_corrupt_file_is_a_miss_not_an_error() {
        let path = temp_path("corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(VerdictCache::load(&path, "12345", "sha3").is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_future_format_version_is_a_miss() {
        let path = temp_path("version");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"change_id":"12345","revision":"sha3","version":999}"#,
        )
        .unwrap();
        assert!(
            VerdictCache::load(&path, "12345", "sha3").is_none(),
            "a shape we do not understand must not be read as if we did"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn saving_creates_the_session_directory() {
        let path = temp_path("mkdir");
        assert!(!path.parent().unwrap().exists());
        VerdictCache::new("1", "s").save(&path).unwrap();
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
