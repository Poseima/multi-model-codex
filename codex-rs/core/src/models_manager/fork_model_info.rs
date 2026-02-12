/// Fork: model metadata overrides for provider-specific models (OpenRouter, MiniMax).
///
/// Since upstream moved all model metadata to remote `models.json` and removed
/// hardcoded definitions from `model_info.rs`, fork-specific models that are not
/// in the remote metadata need a local fallback. This file provides `ModelInfo`
/// for those models, keeping the fork diff to a 2-line hook in upstream code.
use codex_protocol::openai_models::ApplyPatchToolType;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::TruncationPolicyConfig;
use codex_protocol::openai_models::default_input_modalities;

use super::model_info::BASE_INSTRUCTIONS;

const BASE_INSTRUCTIONS_WITH_TEXT_EDITOR: &str =
    include_str!("../../prompt_with_text_editor_instructions.md");

const CONTEXT_WINDOW_272K: i64 = 272_000;

macro_rules! fork_model_info {
    (
        $slug:expr $(, $key:ident : $value:expr )* $(,)?
    ) => {{
        #[allow(unused_mut)]
        let mut model = ModelInfo {
            slug: $slug.to_string(),
            display_name: $slug.to_string(),
            description: None,
            default_reasoning_level: None,
            supported_reasoning_levels: Vec::new(),
            shell_type: ConfigShellToolType::Default,
            visibility: ModelVisibility::None,
            supported_in_api: true,
            priority: 99,
            upgrade: None,
            base_instructions: BASE_INSTRUCTIONS.to_string(),
            model_messages: None,
            supports_reasoning_summaries: false,
            support_verbosity: false,
            default_verbosity: None,
            apply_patch_tool_type: None,
            truncation_policy: TruncationPolicyConfig::bytes(10_000),
            supports_parallel_tool_calls: false,
            context_window: Some(CONTEXT_WINDOW_272K),
            auto_compact_token_limit: None,
            effective_context_window_percent: 95,
            experimental_supported_tools: Vec::new(),
            input_modalities: default_input_modalities(),
            prefer_websockets: false,
        };

        $(
            model.$key = $value;
        )*
        model
    }};
}

/// Returns `Some(ModelInfo)` for fork provider models, `None` for everything else.
pub(crate) fn fork_model_info_for_slug(slug: &str) -> Option<ModelInfo> {
    // MiniMax models
    if slug.starts_with("codex-MiniMax") {
        return Some(fork_model_info!(
            slug,
            base_instructions: BASE_INSTRUCTIONS_WITH_TEXT_EDITOR.to_string(),
            apply_patch_tool_type: Some(ApplyPatchToolType::Structured),
            shell_type: ConfigShellToolType::ShellCommand,
            supports_reasoning_summaries: false,
            context_window: Some(200_000),
        ));
    }

    // Zhipu models
    if slug.starts_with("glm-") {
        let ctx = if slug.starts_with("glm-5") {
            200_000
        } else {
            128_000
        };
        return Some(fork_model_info!(
            slug,
            base_instructions: BASE_INSTRUCTIONS_WITH_TEXT_EDITOR.to_string(),
            apply_patch_tool_type: Some(ApplyPatchToolType::Structured),
            shell_type: ConfigShellToolType::ShellCommand,
            context_window: Some(ctx),
        ));
    }

    // OpenRouter models
    if slug.starts_with("xiaomi/mimo") {
        return Some(fork_model_info!(
            slug,
            context_window: Some(256_000),
        ));
    }
    if slug.starts_with("z-ai/glm") {
        return Some(fork_model_info!(
            slug,
            context_window: Some(128_000),
        ));
    }
    if slug.starts_with("google/gemini-2") || slug.starts_with("google/gemini-3") {
        return Some(fork_model_info!(
            slug,
            context_window: Some(1_000_000),
        ));
    }
    if slug.starts_with("openai/gpt-5") {
        return Some(fork_model_info!(
            slug,
            context_window: Some(CONTEXT_WINDOW_272K),
        ));
    }

    None
}
