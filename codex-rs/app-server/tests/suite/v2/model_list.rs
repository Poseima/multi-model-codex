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
use codex_app_server_protocol::ReasoningEffortOption;
use codex_app_server_protocol::RequestId;
use codex_core::test_support::all_model_presets;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;

/// Build the expected model list matching the server's merge behavior.
///
/// The test cache writes ModelInfo with `slug = preset.model`, which converts
/// back to ModelPreset with `id = model = preset.model`. The merge deduplicates
/// by `model` slug, so the cache-derived version (with `id == model`) wins over
/// builtins (which may have a different `id`).
fn expected_models_from_presets() -> Vec<Model> {
    all_model_presets()
        .iter()
        .filter(|p| p.show_in_picker)
        .map(|preset| Model {
            // After cache round-trip: id = slug = preset.model (not preset.id).
            id: preset.model.clone(),
            model: preset.model.clone(),
            upgrade: preset.upgrade.as_ref().map(|u| u.id.clone()),
            display_name: preset.display_name.clone(),
            description: preset.description.clone(),
            supported_reasoning_efforts: preset
                .supported_reasoning_efforts
                .iter()
                .map(|e| ReasoningEffortOption {
                    reasoning_effort: e.effort,
                    description: e.description.clone(),
                })
                .collect(),
            default_reasoning_effort: preset.default_reasoning_effort,
            input_modalities: preset.input_modalities.clone(),
            // Cache round-trip loses supports_personality (model_messages is not cached).
            supports_personality: false,
            is_default: preset.is_default,
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

    let expected_models = expected_models_from_presets();
    assert_eq!(items, expected_models);
    assert!(next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_models_pagination_works() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_models_cache(codex_home.path())?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let expected_models = expected_models_from_presets();
    let total = expected_models.len();
    assert!(total >= 2, "need at least 2 models for pagination test");

    // Page through all models one at a time.
    let mut collected = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let req_id = mcp
            .send_list_models_request(ModelListParams {
                limit: Some(1),
                cursor: cursor.clone(),
            })
            .await?;

        let resp: JSONRPCResponse = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(req_id)),
        )
        .await??;

        let ModelListResponse {
            data: page,
            next_cursor,
        } = to_response::<ModelListResponse>(resp)?;

        assert_eq!(page.len(), 1);
        collected.push(page.into_iter().next().unwrap());

        match next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    assert_eq!(collected, expected_models);
    Ok(())
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
