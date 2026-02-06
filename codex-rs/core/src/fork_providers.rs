/// Fork: third-party provider definitions (OpenRouter, MiniMax).
///
/// These are isolated in a fork-only file so that `model_provider_info.rs`
/// stays close to upstream and the fork's provider surface is easy to find
/// and maintain.
use std::collections::HashMap;

use crate::model_provider_info::ModelProviderInfo;
use crate::model_provider_info::WireApi;

pub const OPENROUTER_PROVIDER_ID: &str = "openrouter";
pub const MINIMAX_PROVIDER_ID: &str = "minimax";

/// Register all fork-specific providers into the given map.
pub fn register_fork_providers(providers: &mut HashMap<String, ModelProviderInfo>) {
    providers.insert(OPENROUTER_PROVIDER_ID.into(), create_openrouter_provider());
    providers.insert(MINIMAX_PROVIDER_ID.into(), create_minimax_provider());
}

/// Create an OpenRouter provider configuration.
fn create_openrouter_provider() -> ModelProviderInfo {
    ModelProviderInfo {
        name: "OpenRouter".into(),
        base_url: Some("https://openrouter.ai/api/v1".into()),
        env_key: Some("OPENROUTER_API_KEY".into()),
        env_key_instructions: Some("Get your API key at https://openrouter.ai/keys".into()),
        experimental_bearer_token: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: Some(
            [
                (
                    "HTTP-Referer".to_string(),
                    "https://github.com/openai/codex".to_string(),
                ),
                ("X-Title".to_string(), "Codex CLI".to_string()),
            ]
            .into_iter()
            .collect(),
        ),
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        system_role: None,
    }
}

/// Create a MiniMax provider configuration.
fn create_minimax_provider() -> ModelProviderInfo {
    ModelProviderInfo {
        name: "MiniMax".into(),
        base_url: Some("https://api.minimaxi.com/v1".into()),
        env_key: Some("MINIMAX_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        wire_api: WireApi::Chat,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(4),
        stream_max_retries: Some(10),
        stream_idle_timeout_ms: Some(300_000),
        requires_openai_auth: false,
        supports_websockets: false,
        system_role: Some("user".to_string()),
    }
}
