/// Fork: derives provider ID from the preset ID convention.
///
/// Non-default (fork) providers encode their identity in the preset ID
/// using the format `provider/model-name`. This function extracts the
/// provider prefix, avoiding the need to add a `provider_id` field to
/// the upstream `ModelPreset` struct.
///
/// Only known fork provider prefixes are returned to avoid false positives
/// from model slugs that happen to contain slashes.
/// Fork: resolve a model slug to its fork provider ID, if any.
///
/// Returns `None` for built-in OpenAI models (no fork provider).
pub fn provider_for_model_slug(slug: &str) -> Option<String> {
    provider_for_preset(slug).map(String::from)
}

pub fn provider_for_preset(preset_id: &str) -> Option<&str> {
    if let Some(slash_pos) = preset_id.find('/') {
        let prefix = &preset_id[..slash_pos];
        return match prefix {
            "openrouter" | "minimax" | "volcengine" | "zhipu" => Some(prefix),
            _ => None,
        };
    }

    if preset_id.starts_with("codex-MiniMax") || preset_id.starts_with("MiniMax-") {
        return Some("minimax");
    }

    if preset_id.starts_with("glm-") {
        return Some("zhipu");
    }

    if preset_id.starts_with("z-ai/glm")
        || preset_id.starts_with("xiaomi/mimo")
        || preset_id.starts_with("google/gemini-2")
        || preset_id.starts_with("google/gemini-3")
    {
        return Some("openrouter");
    }

    if preset_id.starts_with("ark-") {
        return Some("volcengine");
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn openrouter_presets_return_openrouter() {
        assert_eq!(
            Some("openrouter"),
            provider_for_preset("openrouter/z-ai/glm-4.5-air:free")
        );
        assert_eq!(
            Some("openrouter"),
            provider_for_preset("openrouter/google/gemini-2.0-flash-001")
        );
    }

    #[test]
    fn minimax_presets_return_minimax() {
        assert_eq!(
            Some("minimax"),
            provider_for_preset("minimax/codex-MiniMax-M2.1")
        );
        assert_eq!(Some("minimax"), provider_for_preset("minimax/MiniMax-M2.5"));
    }

    #[test]
    fn volcengine_presets_return_volcengine() {
        assert_eq!(
            Some("volcengine"),
            provider_for_preset("volcengine/ark-code-latest")
        );
    }

    #[test]
    fn zhipu_presets_return_zhipu() {
        assert_eq!(Some("zhipu"), provider_for_preset("zhipu/glm-5"));
        assert_eq!(Some("zhipu"), provider_for_preset("zhipu/glm-4.7"));
    }

    #[test]
    fn builtin_openai_presets_return_none() {
        assert_eq!(None, provider_for_preset("gpt-5.2-codex"));
        assert_eq!(None, provider_for_preset("gpt-5.1-codex-mini"));
    }

    #[test]
    fn unknown_provider_prefix_returns_none() {
        assert_eq!(None, provider_for_preset("unknown/some-model"));
    }

    #[test]
    fn minimax_raw_slug_returns_minimax() {
        assert_eq!(Some("minimax"), provider_for_preset("MiniMax-M2.5"));
        assert_eq!(Some("minimax"), provider_for_preset("codex-MiniMax-M2.1"));
    }

    #[test]
    fn zhipu_raw_slug_returns_zhipu() {
        assert_eq!(Some("zhipu"), provider_for_preset("glm-5"));
        assert_eq!(Some("zhipu"), provider_for_preset("glm-4.7"));
    }
}
