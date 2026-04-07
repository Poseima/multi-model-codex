use super::*;
use crate::ModelsManagerConfig;
use pretty_assertions::assert_eq;

fn test_config() -> ModelsManagerConfig {
    ModelsManagerConfig::default()
}

#[test]
fn reasoning_summaries_override_true_enables_support() {
    let model = model_info_from_slug("unknown-model");
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(true),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.supports_reasoning_summaries = true;

    assert_eq!(updated, expected);
}

#[test]
fn reasoning_summaries_override_false_does_not_disable_support() {
    let mut model = model_info_from_slug("unknown-model");
    model.supports_reasoning_summaries = true;
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(false),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn reasoning_summaries_override_false_is_noop_when_model_is_false() {
    let model = model_info_from_slug("unknown-model");
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(false),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn model_context_window_override_clamps_to_max_context_window() {
    let mut model = model_info_from_slug("unknown-model");
    model.context_window = Some(273_000);
    model.max_context_window = Some(400_000);
    let config = ModelsManagerConfig {
        model_context_window: Some(500_000),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.context_window = Some(400_000);

    assert_eq!(updated, expected);
}

#[test]
fn model_context_window_uses_model_value_without_override() {
    let mut model = model_info_from_slug("unknown-model");
    model.context_window = Some(273_000);
    model.max_context_window = Some(400_000);
    let config = ModelsManagerConfig::default();

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn per_model_auto_compact_limit_overrides_global_default() {
    let model = model_info_from_slug("gpt-5.3-codex");
    let mut config = ModelsManagerConfig {
        personality_enabled: true,
        ..Default::default()
    };
    config.model_auto_compact_token_limit = Some(111);
    config
        .model_auto_compact_token_limits
        .insert("gpt-5.3-codex".to_string(), 222);

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.auto_compact_token_limit = Some(222);

    assert_eq!(updated, expected);
}

#[test]
fn global_auto_compact_limit_used_when_per_model_is_missing() {
    let model = model_info_from_slug("gpt-5.3-codex");
    let mut config = ModelsManagerConfig {
        personality_enabled: true,
        ..Default::default()
    };
    config.model_auto_compact_token_limit = Some(111);
    config
        .model_auto_compact_token_limits
        .insert("gpt-5.1-codex".to_string(), 222);

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.auto_compact_token_limit = Some(111);

    assert_eq!(updated, expected);
}

#[test]
fn model_auto_compact_limit_is_preserved_when_no_config_override_exists() {
    let mut model = model_info_from_slug("gpt-5.3-codex");
    model.auto_compact_token_limit = Some(333);
    let config = ModelsManagerConfig {
        personality_enabled: true,
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn per_model_auto_compact_limit_requires_exact_slug_match() {
    let model = model_info_from_slug("gpt-5.3-codex");
    let mut config = ModelsManagerConfig {
        personality_enabled: true,
        ..Default::default()
    };
    config.model_auto_compact_token_limit = Some(111);
    config
        .model_auto_compact_token_limits
        .insert("gpt-5.3".to_string(), 222);

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.auto_compact_token_limit = Some(111);

    assert_eq!(updated, expected);
}
