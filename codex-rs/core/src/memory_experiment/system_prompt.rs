//! Compose base instructions with memory content for system prompt injection.
//!
//! When the memory experiment is enabled, memory clues (and optionally the
//! upstream memory summary) are appended to the base instructions string
//! rather than injected as separate developer messages. This keeps them in
//! the system prompt prefix for better prompt cache efficiency.

use crate::features::Feature;
use crate::features::Features;
use crate::memories::prompts::build_memory_tool_developer_instructions;
use crate::memory_experiment;
use std::path::Path;

/// Append memory content to base instructions when the memory experiment is active.
///
/// Memory content is appended at the END of the base instructions to minimise
/// prompt cache impact — the static prefix remains cacheable.
///
/// Returns the composed string. If the experiment is not enabled, returns
/// `base_instructions` unchanged.
pub(crate) async fn compose_base_instructions_with_memory(
    base_instructions: &str,
    codex_home: &Path,
    cwd: &Path,
    features: &Features,
) -> String {
    if !memory_experiment::is_enabled(codex_home, cwd, features) {
        return base_instructions.to_string();
    }

    let mut composed = base_instructions.to_string();

    // Append upstream memory tool summary when the MemoryTool feature is also
    // enabled alongside the experiment.
    if features.enabled(Feature::MemoryTool)
        && let Some(memory_prompt) = build_memory_tool_developer_instructions(codex_home).await
    {
        composed.push_str("\n\n");
        composed.push_str(&memory_prompt);
    }

    // Ensure clues are regenerated if memories exist but the index is missing,
    // then append the clues content.
    memory_experiment::ensure_clues_fresh(codex_home, cwd).await;
    if let Some(clues_prompt) = memory_experiment::build_clues_prompt(codex_home, cwd).await {
        composed.push_str("\n\n");
        composed.push_str(&clues_prompt);
    }

    composed
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::features::Features;
    use crate::memory_experiment::get_project_memory_root;
    use pretty_assertions::assert_eq;
    use std::path::Path;

    #[tokio::test]
    async fn compose_returns_unchanged_when_experiment_disabled() {
        let base = "original instructions";
        let features = Features::with_defaults();
        let result = compose_base_instructions_with_memory(
            base,
            Path::new("/tmp/nonexistent"),
            Path::new("/tmp/nonexistent"),
            &features,
        )
        .await;
        assert_eq!(result, base);
    }

    #[tokio::test]
    async fn compose_appends_clues_when_experiment_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let codex_home = tmp.path();
        let cwd = Path::new("/tmp/compose_test_project");
        let project_root = get_project_memory_root(codex_home, cwd);
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::write(project_root.join("config.toml"), "").unwrap();
        std::fs::write(
            project_root.join("memory_clues.md"),
            "### Semantic Memories (Concepts)\n- [test] \u{2192} semantic/test.md\n  desc: test\n",
        )
        .unwrap();

        let base = "original instructions";
        let features = Features::with_defaults();
        let result = compose_base_instructions_with_memory(base, codex_home, cwd, &features).await;
        assert!(result.starts_with(base));
        assert!(result.contains("Project Memory"));
    }

    #[tokio::test]
    async fn compose_unchanged_when_no_clues_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let codex_home = tmp.path();
        let cwd = Path::new("/tmp/compose_no_clues_project");
        let project_root = get_project_memory_root(codex_home, cwd);
        std::fs::create_dir_all(&project_root).unwrap();
        // Config exists (experiment enabled) but no clues file.
        std::fs::write(project_root.join("config.toml"), "").unwrap();

        let base = "original instructions";
        let features = Features::with_defaults();
        let result = compose_base_instructions_with_memory(base, codex_home, cwd, &features).await;
        assert_eq!(result, base);
    }
}
