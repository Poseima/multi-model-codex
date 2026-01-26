use crate::auth::AuthMode;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ModelUpgrade;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use indoc::indoc;
use once_cell::sync::Lazy;

use crate::model_provider_info::OPENROUTER_PROVIDER_ID;

pub const HIDE_GPT5_1_MIGRATION_PROMPT_CONFIG: &str = "hide_gpt5_1_migration_prompt";
pub const HIDE_GPT_5_1_CODEX_MAX_MIGRATION_PROMPT_CONFIG: &str =
    "hide_gpt-5.1-codex-max_migration_prompt";

static PRESETS: Lazy<Vec<ModelPreset>> = Lazy::new(|| {
    vec![
        ModelPreset {
            id: "gpt-5.2-codex".to_string(),
            model: "gpt-5.2-codex".to_string(),
            display_name: "gpt-5.2-codex".to_string(),
            description: "Latest frontier agentic coding model.".to_string(),
            default_reasoning_effort: ReasoningEffort::Medium,
            supported_reasoning_efforts: vec![
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
                ReasoningEffortPreset {
                    effort: ReasoningEffort::XHigh,
                    description: "Extra high reasoning depth for complex problems".to_string(),
                },
            ],
            supports_personality: true,
            is_default: true,
            upgrade: None,
            show_in_picker: true,
            supported_in_api: true,
            provider_id: None,
        },
        ModelPreset {
            id: "gpt-5.1-codex-max".to_string(),
            model: "gpt-5.1-codex-max".to_string(),
            display_name: "gpt-5.1-codex-max".to_string(),
            description: "Codex-optimized flagship for deep and fast reasoning.".to_string(),
            default_reasoning_effort: ReasoningEffort::Medium,
            supported_reasoning_efforts: vec![
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
                ReasoningEffortPreset {
                    effort: ReasoningEffort::XHigh,
                    description: "Extra high reasoning depth for complex problems".to_string(),
                },
            ],
            supports_personality: false,
            is_default: false,
            upgrade: Some(gpt_52_codex_upgrade()),
            show_in_picker: true,
            supported_in_api: true,
            provider_id: None,
        },
        ModelPreset {
            id: "gpt-5.1-codex-mini".to_string(),
            model: "gpt-5.1-codex-mini".to_string(),
            display_name: "gpt-5.1-codex-mini".to_string(),
            description: "Optimized for codex. Cheaper, faster, but less capable.".to_string(),
            default_reasoning_effort: ReasoningEffort::Medium,
            supported_reasoning_efforts: vec![
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Medium,
                    description: "Dynamically adjusts reasoning based on the task".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::High,
                    description: "Maximizes reasoning depth for complex or ambiguous problems"
                        .to_string(),
                },
            ],
            supports_personality: false,
            is_default: false,
            upgrade: Some(gpt_52_codex_upgrade()),
            show_in_picker: true,
            supported_in_api: true,
            provider_id: None,
        },
        ModelPreset {
            id: "gpt-5.2".to_string(),
            model: "gpt-5.2".to_string(),
            display_name: "gpt-5.2".to_string(),
            description: "Latest frontier model with improvements across knowledge, reasoning and coding".to_string(),
            default_reasoning_effort: ReasoningEffort::Medium,
            supported_reasoning_efforts: vec![
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Low,
                    description: "Balances speed with some reasoning; useful for straightforward queries and short explanations".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Medium,
                    description: "Provides a solid balance of reasoning depth and latency for general-purpose tasks".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::High,
                    description: "Maximizes reasoning depth for complex or ambiguous problems".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::XHigh,
                    description: "Extra high reasoning depth for complex problems".to_string(),
                },
            ],
            supports_personality: false,
            is_default: false,
            upgrade: Some(gpt_52_codex_upgrade()),
            show_in_picker: true,
            supported_in_api: true,
            provider_id: None,
        },
        ModelPreset {
            id: "bengalfox".to_string(),
            model: "bengalfox".to_string(),
            display_name: "bengalfox".to_string(),
            description: "bengalfox".to_string(),
            default_reasoning_effort: ReasoningEffort::Medium,
            supported_reasoning_efforts: vec![
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
                ReasoningEffortPreset {
                    effort: ReasoningEffort::XHigh,
                    description: "Extra high reasoning depth for complex problems".to_string(),
                },
            ],
            supports_personality: true,
            is_default: false,
            upgrade: None,
            show_in_picker: false,
            supported_in_api: true,
            provider_id: None,
        },
        ModelPreset {
            id: "boomslang".to_string(),
            model: "boomslang".to_string(),
            display_name: "boomslang".to_string(),
            description: "boomslang".to_string(),
            default_reasoning_effort: ReasoningEffort::Medium,
            supported_reasoning_efforts: vec![
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Low,
                    description: "Balances speed with some reasoning; useful for straightforward queries and short explanations".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Medium,
                    description: "Provides a solid balance of reasoning depth and latency for general-purpose tasks".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::High,
                    description: "Maximizes reasoning depth for complex or ambiguous problems".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::XHigh,
                    description: "Extra high reasoning depth for complex problems".to_string(),
                },
            ],
            supports_personality: false,
            is_default: false,
            upgrade: None,
            show_in_picker: false,
            supported_in_api: true,
            provider_id: None,
        },
        // Deprecated models.
        ModelPreset {
            id: "gpt-5-codex".to_string(),
            model: "gpt-5-codex".to_string(),
            display_name: "gpt-5-codex".to_string(),
            description: "Optimized for codex.".to_string(),
            default_reasoning_effort: ReasoningEffort::Medium,
            supported_reasoning_efforts: vec![
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Low,
                    description: "Fastest responses with limited reasoning".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Medium,
                    description: "Dynamically adjusts reasoning based on the task".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::High,
                    description: "Maximizes reasoning depth for complex or ambiguous problems".to_string(),
                },
            ],
            supports_personality: false,
            is_default: false,
            upgrade: Some(gpt_52_codex_upgrade()),
            show_in_picker: false,
            supported_in_api: true,
            provider_id: None,
        },
        ModelPreset {
            id: "gpt-5-codex-mini".to_string(),
            model: "gpt-5-codex-mini".to_string(),
            display_name: "gpt-5-codex-mini".to_string(),
            description: "Optimized for codex. Cheaper, faster, but less capable.".to_string(),
            default_reasoning_effort: ReasoningEffort::Medium,
            supported_reasoning_efforts: vec![
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Medium,
                    description: "Dynamically adjusts reasoning based on the task".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::High,
                    description: "Maximizes reasoning depth for complex or ambiguous problems".to_string(),
                },
            ],
            supports_personality: false,
            is_default: false,
            upgrade: Some(gpt_52_codex_upgrade()),
            show_in_picker: false,
            supported_in_api: true,
            provider_id: None,
        },
        ModelPreset {
            id: "gpt-5.1-codex".to_string(),
            model: "gpt-5.1-codex".to_string(),
            display_name: "gpt-5.1-codex".to_string(),
            description: "Optimized for codex.".to_string(),
            default_reasoning_effort: ReasoningEffort::Medium,
            supported_reasoning_efforts: vec![
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Low,
                    description: "Fastest responses with limited reasoning".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Medium,
                    description: "Dynamically adjusts reasoning based on the task".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::High,
                    description: "Maximizes reasoning depth for complex or ambiguous problems"
                        .to_string(),
                },
            ],
            supports_personality: false,
            is_default: false,
            upgrade: Some(gpt_52_codex_upgrade()),
            show_in_picker: false,
            supported_in_api: true,
            provider_id: None,
        },
        ModelPreset {
            id: "gpt-5".to_string(),
            model: "gpt-5".to_string(),
            display_name: "gpt-5".to_string(),
            description: "Broad world knowledge with strong general reasoning.".to_string(),
            default_reasoning_effort: ReasoningEffort::Medium,
            supported_reasoning_efforts: vec![
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Minimal,
                    description: "Fastest responses with little reasoning".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Low,
                    description: "Balances speed with some reasoning; useful for straightforward queries and short explanations".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Medium,
                    description: "Provides a solid balance of reasoning depth and latency for general-purpose tasks".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::High,
                    description: "Maximizes reasoning depth for complex or ambiguous problems".to_string(),
                },
            ],
            supports_personality: false,
            is_default: false,
            upgrade: Some(gpt_52_codex_upgrade()),
            show_in_picker: false,
            supported_in_api: true,
            provider_id: None,
        },
        ModelPreset {
            id: "gpt-5.1".to_string(),
            model: "gpt-5.1".to_string(),
            display_name: "gpt-5.1".to_string(),
            description: "Broad world knowledge with strong general reasoning.".to_string(),
            default_reasoning_effort: ReasoningEffort::Medium,
            supported_reasoning_efforts: vec![
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Low,
                    description: "Balances speed with some reasoning; useful for straightforward queries and short explanations".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Medium,
                    description: "Provides a solid balance of reasoning depth and latency for general-purpose tasks".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::High,
                    description: "Maximizes reasoning depth for complex or ambiguous problems".to_string(),
                },
            ],
            supports_personality: false,
            is_default: false,
            upgrade: Some(gpt_52_codex_upgrade()),
            show_in_picker: false,
            supported_in_api: true,
            provider_id: None,
        },
        // OpenRouter models.
        ModelPreset {
            id: "openrouter/z-ai/glm-4.5-air:free".to_string(),
            model: "z-ai/glm-4.5-air:free".to_string(),
            display_name: "GLM-4.5 Air (free)".to_string(),
            description: "Zhipu AI's GLM-4.5 model via OpenRouter.".to_string(),
            default_reasoning_effort: ReasoningEffort::None,
            supported_reasoning_efforts: vec![],
            supports_personality: false,
            is_default: false,
            upgrade: None,
            show_in_picker: true,
            supported_in_api: true,
            provider_id: Some(OPENROUTER_PROVIDER_ID.to_string()),
        },
        ModelPreset {
            id: "openrouter/xiaomi/mimo-v2-flash:free".to_string(),
            model: "xiaomi/mimo-v2-flash:free".to_string(),
            display_name: "MiMo-V2-Flash (free)".to_string(),
            description: "Xiaomi's 309B MoE model. Top open-source on SWE-bench.".to_string(),
            default_reasoning_effort: ReasoningEffort::None,
            supported_reasoning_efforts: vec![],
            supports_personality: false,
            is_default: false,
            upgrade: None,
            show_in_picker: true,
            supported_in_api: true,
            provider_id: Some(OPENROUTER_PROVIDER_ID.to_string()),
        },
        ModelPreset {
            id: "openrouter/google/gemini-2.0-flash-001".to_string(),
            model: "google/gemini-2.0-flash-001".to_string(),
            display_name: "Gemini 2.0 Flash".to_string(),
            description: "Google's Gemini 2.0 Flash model via OpenRouter.".to_string(),
            default_reasoning_effort: ReasoningEffort::None,
            supported_reasoning_efforts: vec![],
            supports_personality: false,
            is_default: false,
            upgrade: None,
            show_in_picker: true,
            supported_in_api: true,
            provider_id: Some(OPENROUTER_PROVIDER_ID.to_string()),
        },
        ModelPreset {
            id: "openrouter/google/gemini-3-flash-preview".to_string(),
            model: "google/gemini-3-flash-preview".to_string(),
            display_name: "Gemini 3 Flash Preview".to_string(),
            description: "Google's Gemini 3 Flash preview model via OpenRouter.".to_string(),
            default_reasoning_effort: ReasoningEffort::None,
            supported_reasoning_efforts: vec![],
            supports_personality: false,
            is_default: false,
            upgrade: None,
            show_in_picker: true,
            supported_in_api: true,
            provider_id: Some(OPENROUTER_PROVIDER_ID.to_string()),
        },
        ModelPreset {
            id: "openrouter/openai/gpt-5.1-codex-mini".to_string(),
            model: "openai/gpt-5.1-codex-mini".to_string(),
            display_name: "GPT-5.1 Codex Mini (OpenRouter)".to_string(),
            description: "OpenAI's GPT-5.1 Codex Mini via OpenRouter.".to_string(),
            default_reasoning_effort: ReasoningEffort::None,
            supported_reasoning_efforts: vec![],
            supports_personality: false,
            is_default: false,
            upgrade: None,
            show_in_picker: true,
            supported_in_api: true,
            provider_id: Some(OPENROUTER_PROVIDER_ID.to_string()),
        },
    ]
});

fn gpt_52_codex_upgrade() -> ModelUpgrade {
    ModelUpgrade {
        id: "gpt-5.2-codex".to_string(),
        reasoning_effort_mapping: None,
        migration_config_key: "gpt-5.2-codex".to_string(),
        model_link: Some("https://openai.com/index/introducing-gpt-5-2-codex".to_string()),
        upgrade_copy: Some(
            "Codex is now powered by gpt-5.2-codex, our latest frontier agentic coding model. It is smarter and faster than its predecessors and capable of long-running project-scale work."
                .to_string(),
        ),
        migration_markdown: Some(
            indoc! {r#"
                **Codex just got an upgrade. Introducing {model_to}.**

                Codex is now powered by gpt-5.2-codex, our latest frontier agentic coding model. It is smarter and faster than its predecessors and capable of long-running project-scale work. Learn more about {model_to} at https://openai.com/index/introducing-gpt-5-2-codex

                You can continue using {model_from} if you prefer.
            "#}
            .to_string(),
        ),
    }
}

pub(super) fn builtin_model_presets(_auth_mode: Option<AuthMode>) -> Vec<ModelPreset> {
    PRESETS.iter().cloned().collect()
}

#[cfg(any(test, feature = "test-support"))]
pub fn all_model_presets() -> &'static Vec<ModelPreset> {
    &PRESETS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_default_model_is_configured() {
        let default_models = PRESETS.iter().filter(|preset| preset.is_default).count();
        assert!(default_models == 1);
    }
}
