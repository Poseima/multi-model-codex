use std::time::Duration;

use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::to_response;
use app_test_support::write_models_cache;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::Model;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::ModelUpgradeInfo;
use codex_app_server_protocol::ReasoningEffortOption;
use codex_app_server_protocol::RequestId;
use codex_protocol::openai_models::ModelPreset;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;

fn model_from_preset(preset: &ModelPreset) -> Model {
    Model {
        id: preset.id.clone(),
        model: preset.model.clone(),
        upgrade: preset.upgrade.as_ref().map(|upgrade| upgrade.id.clone()),
        upgrade_info: preset.upgrade.as_ref().map(|upgrade| ModelUpgradeInfo {
            model: upgrade.id.clone(),
            upgrade_copy: upgrade.upgrade_copy.clone(),
            model_link: upgrade.model_link.clone(),
            migration_markdown: upgrade.migration_markdown.clone(),
        }),
        availability_nux: preset.availability_nux.clone().map(Into::into),
        display_name: preset.display_name.clone(),
        description: preset.description.clone(),
        hidden: !preset.show_in_picker,
        supported_reasoning_efforts: preset
            .supported_reasoning_efforts
            .iter()
            .map(|preset| ReasoningEffortOption {
                reasoning_effort: preset.effort,
                description: preset.description.clone(),
            })
            .collect(),
        default_reasoning_effort: preset.default_reasoning_effort,
        input_modalities: preset.input_modalities.clone(),
        // `write_models_cache()` round-trips through a simplified ModelInfo fixture that does not
        // preserve personality placeholders in base instructions, so app-server list results from
        // cache report `supports_personality = false`.
        // todo(sayan): fix, maybe make roundtrip use ModelInfo only
        supports_personality: false,
        is_default: preset.is_default,
    }
}

fn expected_visible_models() -> Vec<Model> {
    // Filter by supported_in_api to support testing with both ChatGPT and non-ChatGPT auth modes.
    let mut presets =
        ModelPreset::filter_by_auth(codex_core::test_support::all_model_presets().clone(), false);

    // Mirror `ModelsManager::build_available_models()` default selection after auth filtering.
    ModelPreset::mark_default_by_picker_visibility(&mut presets);

    let mut models: Vec<Model> = presets
        .iter()
        .filter(|preset| preset.show_in_picker)
        .map(model_from_preset)
        .collect();

    // Fork catalog models are merged at runtime by `merge_with_fork_models()` in ModelsManager.
    // They have higher priority numbers so they appear after upstream models.
    models.extend(fork_catalog_expected_models());

    models
}

/// Expected Model entries for fork catalog models (MiniMax, Zhipu).
///
/// These are added by `merge_with_fork_models()` at runtime and are not in the
/// bundled `models.json`, so we construct them manually for test assertions.
fn fork_catalog_expected_models() -> Vec<Model> {
    let fork_reasoning_efforts = vec![
        ReasoningEffortOption {
            reasoning_effort: codex_protocol::openai_models::ReasoningEffort::Low,
            description: "Fast responses with lighter reasoning".to_string(),
        },
        ReasoningEffortOption {
            reasoning_effort: codex_protocol::openai_models::ReasoningEffort::Medium,
            description: "Balances speed and reasoning depth for everyday tasks".to_string(),
        },
        ReasoningEffortOption {
            reasoning_effort: codex_protocol::openai_models::ReasoningEffort::High,
            description: "Greater reasoning depth for complex problems".to_string(),
        },
    ];

    [
        ("codex-MiniMax-M2.1", "MiniMax coding model"),
        ("MiniMax-M2.5", "MiniMax coding model"),
        ("glm-5", "Zhipu GLM coding model"),
        ("glm-4.7", "Zhipu GLM coding model"),
    ]
    .into_iter()
    .map(|(slug, description)| Model {
        id: slug.to_string(),
        model: slug.to_string(),
        upgrade: None,
        upgrade_info: None,
        availability_nux: None,
        display_name: slug.to_string(),
        description: description.to_string(),
        hidden: false,
        supported_reasoning_efforts: fork_reasoning_efforts.clone(),
        default_reasoning_effort: codex_protocol::openai_models::ReasoningEffort::Medium,
        input_modalities: vec![codex_protocol::openai_models::InputModality::Text],
        supports_personality: false,
        is_default: false,
    })
    .collect()
}

#[tokio::test]
async fn list_models_returns_all_models_with_large_limit() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_models_cache(codex_home.path())?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: Some(100),
            cursor: None,
            include_hidden: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelListResponse {
        data: items,
        next_cursor,
    } = to_response::<ModelListResponse>(response)?;

    let expected_models = expected_visible_models();

    assert_eq!(items, expected_models);
    assert!(next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_models_includes_hidden_models() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_models_cache(codex_home.path())?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: Some(100),
            cursor: None,
            include_hidden: Some(true),
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelListResponse {
        data: items,
        next_cursor,
    } = to_response::<ModelListResponse>(response)?;

    assert!(items.iter().any(|item| item.hidden));
    assert!(next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_models_pagination_works() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_models_cache(codex_home.path())?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let expected_models = expected_visible_models();
    let mut cursor = None;
    let mut items = Vec::new();

    for _ in 0..expected_models.len() {
        let request_id = mcp
            .send_list_models_request(ModelListParams {
                limit: Some(1),
                cursor: cursor.clone(),
                include_hidden: None,
            })
            .await?;

        let response: JSONRPCResponse = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
        )
        .await??;

        let ModelListResponse {
            data: page_items,
            next_cursor,
        } = to_response::<ModelListResponse>(response)?;

        assert_eq!(page_items.len(), 1);
        items.extend(page_items);

        if let Some(next_cursor) = next_cursor {
            cursor = Some(next_cursor);
        } else {
            assert_eq!(items, expected_models);
            return Ok(());
        }
    }

    panic!(
        "model pagination did not terminate after {} pages",
        expected_models.len()
    );
}

#[tokio::test]
async fn list_models_rejects_invalid_cursor() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_models_cache(codex_home.path())?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: None,
            cursor: Some("invalid".to_string()),
            include_hidden: None,
        })
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(error.error.message, "invalid cursor: invalid");
    Ok(())
}
