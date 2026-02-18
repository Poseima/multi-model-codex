//! Project-scoped memory experiment.
//!
//! This module implements a file-based memory system that replaces compaction
//! when enabled. It is entirely fork-isolated: the experiment is activated by
//! the presence of a `config.toml` in the project memory directory.
//!
//! ## Storage layout
//! ```text
//! {codex_home}/memories_experiment/{project_name}/
//! ├── config.toml          ← presence = experiment enabled
//! ├── memory_clues.md      ← loaded into system prompt
//! ├── semantic/             ← persistent concept memories
//! └── episodic/             ← time-limited event memories
//! ```
//!

pub(crate) mod archiver;
pub(crate) mod clues;
pub(crate) mod expiration;
pub(crate) mod retrieval;
pub(crate) mod types;

use crate::features::Feature;
use crate::features::Features;
use crate::git_info::get_git_repo_root;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use types::ExperimentConfig;

// Re-export the public API items used by upstream hooks.
pub(crate) use clues::build_clues_prompt;
pub(crate) use clues::ensure_clues_fresh;

/// Subdirectory name under codex_home for experiment storage.
const EXPERIMENT_DIR: &str = "memories_experiment";

/// Check whether the memory experiment is enabled for the project at `cwd`.
///
/// The experiment is active when EITHER:
/// - The `memory_experiment` feature flag is enabled in global config, OR
/// - A per-project `{project_memory_root}/config.toml` exists (backward compat).
pub(crate) fn is_enabled(codex_home: &Path, cwd: &Path, features: &Features) -> bool {
    features.enabled(Feature::MemoryExperiment)
        || get_project_memory_root(codex_home, cwd)
            .join("config.toml")
            .exists()
}

/// Derive the project-scoped memory root directory.
///
/// Uses the last path component of the git repo root (or cwd) as a
/// human-readable project identifier. Falls back to a hash when the path
/// has no meaningful last component (e.g. filesystem root `/`).
pub(crate) fn get_project_memory_root(codex_home: &Path, cwd: &Path) -> PathBuf {
    let repo_root = get_git_repo_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let dir_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{:016x}", hash_path(&repo_root)));
    codex_home.join(EXPERIMENT_DIR).join(dir_name)
}

/// Read the experiment config from the project memory directory.
/// Returns defaults if the file is missing or unparseable.
pub(crate) fn read_config(project_root: &Path) -> ExperimentConfig {
    let config_path = project_root.join("config.toml");
    std::fs::read_to_string(config_path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// Ensure the project memory directory structure exists.
pub(crate) async fn ensure_layout(project_root: &Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(project_root.join("semantic")).await?;
    tokio::fs::create_dir_all(project_root.join("episodic")).await?;
    Ok(())
}

/// Check whether the memory directories contain any `.md` files.
///
/// Returns `true` when both `semantic/` and `episodic/` are empty (or missing),
/// indicating a fresh memory directory where Phase 1 (Retrieval) can be skipped.
pub(crate) async fn is_memory_empty(project_root: &Path) -> bool {
    dir_has_no_md_files(&project_root.join("semantic")).await
        && dir_has_no_md_files(&project_root.join("episodic")).await
}

/// Returns `true` if the directory does not exist or contains no `.md` files.
async fn dir_has_no_md_files(dir: &Path) -> bool {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return true;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.path().extension().is_some_and(|ext| ext == "md") {
            return false;
        }
    }
    true
}

fn hash_path(path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::features::Features;
    use pretty_assertions::assert_eq;
    use std::path::Path;

    #[test]
    fn project_memory_root_is_deterministic() {
        let home = Path::new("/tmp/codex_home");
        let cwd = Path::new("/tmp/no_git_repo_here");
        let root1 = get_project_memory_root(home, cwd);
        let root2 = get_project_memory_root(home, cwd);
        assert_eq!(root1, root2);
        assert!(root1.starts_with(home.join(EXPERIMENT_DIR)));
    }

    #[test]
    fn project_memory_root_uses_folder_name() {
        let home = Path::new("/tmp/codex_home");
        // No git repo, so cwd's last component is used as the dir name.
        let cwd = Path::new("/tmp/my-project");
        let root = get_project_memory_root(home, cwd);
        assert_eq!(root, home.join(EXPERIMENT_DIR).join("my-project"));
    }

    #[test]
    fn different_cwds_produce_different_roots() {
        let home = Path::new("/tmp/codex_home");
        let root_a = get_project_memory_root(home, Path::new("/project/a"));
        let root_b = get_project_memory_root(home, Path::new("/project/b"));
        assert_ne!(root_a, root_b);
    }

    #[test]
    fn is_enabled_returns_false_when_no_config_and_no_flag() {
        let home = Path::new("/tmp/nonexistent_codex_home");
        let cwd = Path::new("/tmp/nonexistent_cwd");
        assert!(!is_enabled(home, cwd, &Features::with_defaults()));
    }

    #[test]
    fn is_enabled_returns_true_when_config_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let cwd = Path::new("/tmp/some_project");
        let root = get_project_memory_root(home, cwd);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config.toml"), "").unwrap();
        assert!(is_enabled(home, cwd, &Features::with_defaults()));
    }

    #[test]
    fn is_enabled_returns_true_when_feature_flag_enabled() {
        let home = Path::new("/tmp/nonexistent_codex_home");
        let cwd = Path::new("/tmp/nonexistent_cwd");
        let mut features = Features::with_defaults();
        features.enable(Feature::MemoryExperiment);
        assert!(is_enabled(home, cwd, &features));
    }

    #[test]
    fn read_config_returns_defaults_when_missing() {
        let config = read_config(Path::new("/tmp/nonexistent"));
        assert_eq!(config.episodic_expiry_days, 30);
    }

    #[test]
    fn read_config_parses_custom_values() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            "episodic_expiry_days = 7\nretrieval_model = \"custom-model\"",
        )
        .unwrap();
        let config = read_config(tmp.path());
        assert_eq!(config.episodic_expiry_days, 7);
        assert_eq!(config.retrieval_model, "custom-model");
    }

    #[tokio::test]
    async fn ensure_layout_creates_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        ensure_layout(&root).await.unwrap();
        assert!(root.join("semantic").is_dir());
        assert!(root.join("episodic").is_dir());
    }

    #[tokio::test]
    async fn ensure_layout_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        ensure_layout(&root).await.unwrap();
        // Call again — should not fail.
        ensure_layout(&root).await.unwrap();
        assert!(root.join("semantic").is_dir());
        assert!(root.join("episodic").is_dir());
    }

    #[tokio::test]
    async fn is_memory_empty_true_for_fresh_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        ensure_layout(&root).await.unwrap();
        assert!(is_memory_empty(&root).await);
    }

    #[tokio::test]
    async fn is_memory_empty_true_for_missing_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("nonexistent");
        assert!(is_memory_empty(&root).await);
    }

    #[tokio::test]
    async fn is_memory_empty_false_when_semantic_has_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        ensure_layout(&root).await.unwrap();
        std::fs::write(root.join("semantic/topic.md"), "# Topic").unwrap();
        assert!(!is_memory_empty(&root).await);
    }

    #[tokio::test]
    async fn is_memory_empty_false_when_episodic_has_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        ensure_layout(&root).await.unwrap();
        std::fs::write(root.join("episodic/2026-02.md"), "event").unwrap();
        assert!(!is_memory_empty(&root).await);
    }

    #[tokio::test]
    async fn is_memory_empty_ignores_non_md_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        ensure_layout(&root).await.unwrap();
        std::fs::write(root.join("semantic/.gitkeep"), "").unwrap();
        assert!(is_memory_empty(&root).await);
    }
}
