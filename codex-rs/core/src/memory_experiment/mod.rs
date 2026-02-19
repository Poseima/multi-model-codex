//! Project-scoped memory experiment.
//!
//! This module implements a file-based memory system that replaces compaction
//! when enabled. It is entirely fork-isolated: the experiment is activated by
//! the presence of a `config.toml` in the project memory directory.
//!
//! ## Storage layout
//! ```text
//! {codex_home}/memories_experiment/
//! ├── config.toml                       ← global defaults (all projects)
//! └── {project_name}/
//!     ├── config.toml                   ← per-project overrides
//!     ├── memory_clues.md               ← loaded into system prompt
//!     ├── semantic/                     ← persistent concept memories
//!     └── episodic/                     ← time-limited event memories
//! ```
//!

pub(crate) mod archiver;
pub(crate) mod clues;
pub(crate) mod expiration;
pub(crate) mod types;

use crate::config::Config;
use crate::features::Feature;
use crate::features::Features;
use crate::git_info::get_git_repo_root;
use codex_protocol::openai_models::ReasoningEffort;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use types::ExperimentConfig;
use types::ExperimentConfigRaw;

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

/// Read the experiment config with layered merging:
///
/// 1. Hardcoded defaults (MiniMax M2.5, applied via `From<ExperimentConfigRaw>`)
/// 2. Global config at `{codex_home}/memories_experiment/config.toml`
/// 3. Per-project config at `{project_root}/config.toml`
///
/// Higher-priority layers override lower ones field-by-field.
pub(crate) fn read_config(codex_home: &Path, project_root: &Path) -> ExperimentConfig {
    let global_path = codex_home.join(EXPERIMENT_DIR).join("config.toml");
    let project_path = project_root.join("config.toml");

    let global_raw = read_raw_config(&global_path);
    let project_raw = read_raw_config(&project_path);

    global_raw.merge(project_raw).into()
}

/// Read a single raw config file. Returns default (all `None`) if missing or
/// unparseable.
fn read_raw_config(path: &Path) -> ExperimentConfigRaw {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// Apply model, provider, and reasoning effort overrides to a sub-agent config.
///
/// Sets `config.model` unconditionally. If `provider_id` is given and exists
/// in `config.model_providers`, also updates `model_provider` and
/// `model_provider_id`. If `reasoning_effort` is given, sets
/// `model_reasoning_effort`.
pub(crate) fn apply_model_override(
    config: &mut Config,
    model: &str,
    provider_id: Option<&str>,
    reasoning_effort: Option<ReasoningEffort>,
) {
    config.model = Some(model.to_string());

    if let Some(pid) = provider_id
        && let Some(provider_info) = config.model_providers.get(pid)
    {
        config.model_provider = provider_info.clone();
        config.model_provider_id = pid.to_string();
    }

    if let Some(effort) = reasoning_effort {
        config.model_reasoning_effort = Some(effort);
    }
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
        let nonexistent = Path::new("/tmp/nonexistent");
        let config = read_config(nonexistent, nonexistent);
        assert_eq!(config, ExperimentConfig::default());
    }

    #[test]
    fn read_config_parses_project_values() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().join(EXPERIMENT_DIR).join("my-project");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::write(
            project_root.join("config.toml"),
            "episodic_expiry_days = 7\nretrieval_model = \"custom-model\"",
        )
        .unwrap();
        let config = read_config(tmp.path(), &project_root);
        assert_eq!(config.episodic_expiry_days, 7);
        assert_eq!(config.retrieval_model, "custom-model");
    }

    #[test]
    fn read_config_layers_global_and_project() {
        let tmp = tempfile::tempdir().unwrap();
        let codex_home = tmp.path();
        let global_dir = codex_home.join(EXPERIMENT_DIR);
        let project_root = global_dir.join("my-project");
        std::fs::create_dir_all(&project_root).unwrap();

        // Global sets retrieval_model and archive_model.
        std::fs::write(
            global_dir.join("config.toml"),
            "retrieval_model = \"global-retrieval\"\narchive_model = \"global-archive\"",
        )
        .unwrap();

        // Project overrides only retrieval_model.
        std::fs::write(
            project_root.join("config.toml"),
            "retrieval_model = \"project-retrieval\"",
        )
        .unwrap();

        let config = read_config(codex_home, &project_root);
        // Project wins for retrieval_model.
        assert_eq!(config.retrieval_model, "project-retrieval");
        // Global fills the gap for archive_model.
        assert_eq!(config.archive_model, "global-archive");
        // Hardcoded default for everything else.
        assert_eq!(config.retrieval_provider, Some("minimax".to_string()));
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

    #[test]
    fn apply_model_override_sets_model() {
        let mut config = crate::config::test_config();
        apply_model_override(&mut config, "gpt-5.3-codex-spark", None, None);
        assert_eq!(config.model, Some("gpt-5.3-codex-spark".to_string()));
    }

    #[test]
    fn apply_model_override_sets_provider_when_found() {
        let mut config = crate::config::test_config();
        // "openai" is a built-in provider that should exist in model_providers.
        let has_openai = config.model_providers.contains_key("openai");
        apply_model_override(&mut config, "gpt-5.3-codex", Some("openai"), None);
        assert_eq!(config.model, Some("gpt-5.3-codex".to_string()));
        if has_openai {
            assert_eq!(config.model_provider_id, "openai");
        }
    }

    #[test]
    fn apply_model_override_ignores_unknown_provider() {
        let mut config = crate::config::test_config();
        let original_provider_id = config.model_provider_id.clone();
        apply_model_override(
            &mut config,
            "some-model",
            Some("nonexistent-provider"),
            None,
        );
        // Provider ID should remain unchanged when the provider is not found.
        assert_eq!(config.model_provider_id, original_provider_id);
    }

    #[test]
    fn apply_model_override_sets_reasoning_effort() {
        let mut config = crate::config::test_config();
        apply_model_override(
            &mut config,
            "gpt-5.3-codex",
            None,
            Some(ReasoningEffort::Low),
        );
        assert_eq!(config.model_reasoning_effort, Some(ReasoningEffort::Low));
    }
}
