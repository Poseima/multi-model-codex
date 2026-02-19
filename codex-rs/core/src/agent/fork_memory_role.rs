/// Fork: enrich spawn_agent config when the role is "memory_retriever".
///
/// When the memory experiment is enabled, this hook injects:
/// 1. The retrieval research prompt (instructions for the sub-agent)
/// 2. The memory clues index (compact listing of available memory files)
/// 3. The memory root directory path (so the agent knows where to read)
///
/// These are prepended to `developer_instructions` so the spawned agent
/// receives them as high-priority context.
use std::path::Path;

use crate::config::Config;
use crate::memory_experiment;

/// System prompt for the retrieval research agent.
const RETRIEVAL_PROMPT: &str =
    include_str!("../../templates/memory_experiment/retrieval_prompt.md");

/// Enrich config with memory clues if this is a memory_retriever role.
///
/// Called from the `spawn_agent` handler after `apply_role_to_config()`.
/// Returns early (no-op) if the role is not "memory_retriever" or if
/// the memory experiment is not enabled.
pub(crate) async fn enrich_config_if_memory_role(
    config: &mut Config,
    role_name: Option<&str>,
    cwd: &Path,
) {
    let Some("memory_retriever") = role_name else {
        return;
    };

    if !memory_experiment::is_enabled(&config.codex_home, cwd, &config.features) {
        return;
    }

    let project_root = memory_experiment::get_project_memory_root(&config.codex_home, cwd);

    // Apply model/provider/reasoning overrides from the experiment config.
    let exp_config = memory_experiment::read_config(&config.codex_home, &project_root);
    memory_experiment::apply_model_override(
        config,
        &exp_config.retrieval_model,
        exp_config.retrieval_provider.as_deref(),
        exp_config.retrieval_reasoning_effort,
    );

    let clues = read_clues(&project_root).await;

    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");

    let retrieval_instructions = format!(
        "{RETRIEVAL_PROMPT}\n\n\
         Current time: {now}\n\
         Memory root directory: {path}\n\n\
         <memory_clues>\n{clues}\n</memory_clues>",
        path = project_root.display(),
    );

    // Prepend to existing developer_instructions so the retrieval prompt
    // sits closest to the model's generation point.
    let existing = config.developer_instructions.take().unwrap_or_default();
    config.developer_instructions = Some(format!("{retrieval_instructions}\n\n{existing}"));

    // Override cwd to the memory root so the agent's file tools operate
    // in the memory directory by default.
    config.cwd = project_root;
}

async fn read_clues(project_root: &Path) -> String {
    tokio::fs::read_to_string(project_root.join("memory_clues.md"))
        .await
        .unwrap_or_default()
}
