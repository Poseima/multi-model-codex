//! Fork-only catalog additions for third-party providers.
//!
//! Upstream switched model picker entries to come from `core/models.json`.
//! To keep fork provider models visible in `/model` without forking that large
//! JSON file, we merge a small list of fork models at runtime.

use codex_protocol::openai_models::ApplyPatchToolType;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::openai_models::TruncationPolicyConfig;
use codex_protocol::openai_models::default_input_modalities;

const BASE_INSTRUCTIONS_WITH_TEXT_EDITOR: &str =
    include_str!("../../prompt_with_text_editor_instructions.md");

pub(crate) fn merge_with_fork_models(mut models: Vec<ModelInfo>) -> Vec<ModelInfo> {
    for model in fork_models() {
        if !models.iter().any(|existing| existing.slug == model.slug) {
            models.push(model);
        }
    }
    models
}

fn fork_models() -> Vec<ModelInfo> {
    vec![
        minimax_model("codex-MiniMax-M2.1", 200_000, 200),
        minimax_model("MiniMax-M2.5", 200_000, 201),
        zhipu_model("glm-5", 200_000, 210),
        zhipu_model("glm-4.7", 128_000, 211),
    ]
}

fn reasoning_presets() -> Vec<ReasoningEffortPreset> {
    vec![
        ReasoningEffortPreset {
            effort: ReasoningEffort::Low,
            description: "Fast responses with lighter reasoning".to_string(),
        },
        ReasoningEffortPreset {
            effort: ReasoningEffort::Medium,
            description: "Balances speed and reasoning depth for everyday tasks".to_string(),
        },
        ReasoningEffortPreset {
            effort: ReasoningEffort::High,
            description: "Greater reasoning depth for complex problems".to_string(),
        },
    ]
}

fn minimax_model(slug: &str, context_window: i64, priority: i32) -> ModelInfo {
    ModelInfo {
        slug: slug.to_string(),
        display_name: slug.to_string(),
        description: Some("MiniMax coding model".to_string()),
        default_reasoning_level: Some(ReasoningEffort::Medium),
        supported_reasoning_levels: reasoning_presets(),
        shell_type: ConfigShellToolType::ShellCommand,
        visibility: ModelVisibility::List,
        supported_in_api: true,
        priority,
        upgrade: None,
        base_instructions: BASE_INSTRUCTIONS_WITH_TEXT_EDITOR.to_string(),
        model_messages: None,
        supports_reasoning_summaries: false,
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: Some(ApplyPatchToolType::Structured),
        truncation_policy: TruncationPolicyConfig::bytes(10_000),
        supports_parallel_tool_calls: false,
        context_window: Some(context_window),
        auto_compact_token_limit: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
        input_modalities: default_input_modalities(),
        prefer_websockets: false,
        used_fallback_model_metadata: false,
    }
}

fn zhipu_model(slug: &str, context_window: i64, priority: i32) -> ModelInfo {
    ModelInfo {
        slug: slug.to_string(),
        display_name: slug.to_string(),
        description: Some("Zhipu GLM coding model".to_string()),
        default_reasoning_level: Some(ReasoningEffort::Medium),
        supported_reasoning_levels: reasoning_presets(),
        shell_type: ConfigShellToolType::ShellCommand,
        visibility: ModelVisibility::List,
        supported_in_api: true,
        priority,
        upgrade: None,
        base_instructions: BASE_INSTRUCTIONS_WITH_TEXT_EDITOR.to_string(),
        model_messages: None,
        supports_reasoning_summaries: false,
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: Some(ApplyPatchToolType::Structured),
        truncation_policy: TruncationPolicyConfig::bytes(10_000),
        supports_parallel_tool_calls: false,
        context_window: Some(context_window),
        auto_compact_token_limit: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
        input_modalities: default_input_modalities(),
        prefer_websockets: false,
        used_fallback_model_metadata: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn merge_adds_missing_fork_models() {
        let merged = merge_with_fork_models(Vec::new());
        let slugs: Vec<&str> = merged.iter().map(|model| model.slug.as_str()).collect();
        assert!(slugs.contains(&"codex-MiniMax-M2.1"));
        assert!(slugs.contains(&"MiniMax-M2.5"));
        assert!(slugs.contains(&"glm-5"));
        assert!(slugs.contains(&"glm-4.7"));
    }

    #[test]
    fn merge_does_not_duplicate_existing_models() {
        let existing = vec![minimax_model("MiniMax-M2.5", 200_000, 201)];
        let merged = merge_with_fork_models(existing);
        let count = merged
            .iter()
            .filter(|model| model.slug == "MiniMax-M2.5")
            .count();
        assert_eq!(count, 1);
    }
}
