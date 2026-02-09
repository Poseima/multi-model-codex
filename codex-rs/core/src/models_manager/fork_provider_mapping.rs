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
/// Looks up the slug in the built-in presets and extracts the provider
/// prefix from the matching preset ID. Returns `None` for built-in
/// OpenAI models (no fork provider).
pub fn provider_for_model_slug(slug: &str) -> Option<String> {
    super::model_presets::builtin_model_presets(None)
        .iter()
        .find(|p| p.model == slug)
        .and_then(|p| provider_for_preset(&p.id).map(String::from))
}

pub fn provider_for_preset(preset_id: &str) -> Option<&str> {
    let slash_pos = preset_id.find('/')?;
    let prefix = &preset_id[..slash_pos];
    match prefix {
        "openrouter" | "minimax" | "volcengine" => Some(prefix),
        _ => None,
    }
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
    }

    #[test]
    fn volcengine_presets_return_volcengine() {
        assert_eq!(
            Some("volcengine"),
            provider_for_preset("volcengine/ark-code-latest")
        );
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
}
