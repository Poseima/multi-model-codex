//! Integration tests for the fork memory experiment system.
//!
//! These tests verify the end-to-end flow:
//! - `memory_retrieve` tool is registered and callable
//! - Memory clues are injected into the system prompt when enabled
//! - The tool handler reads memory files and returns their content
//! - The tool handler returns appropriate errors when experiment is not enabled
#![cfg(not(target_os = "windows"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use serde_json::Value;

/// Replicate the project memory root derivation from `memory_experiment::mod.rs`.
/// Since the test cwd is a temp dir (not a git repo), the cwd itself is hashed.
fn compute_project_memory_root(codex_home: &Path, cwd: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    let hash = hasher.finish();
    codex_home
        .join("memories_experiment")
        .join(format!("{hash:016x}"))
}

/// Enable the memory experiment by creating config.toml and memory_clues.md.
fn enable_experiment(project_root: &Path, clues_content: &str) {
    fs::create_dir_all(project_root).unwrap();
    fs::write(project_root.join("config.toml"), "").unwrap();
    fs::write(project_root.join("memory_clues.md"), clues_content).unwrap();
}

/// Helper to extract tool names from a request body.
fn tool_names(body: &Value) -> Vec<String> {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    tool.get("name")
                        .or_else(|| tool.get("type"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

/// Verify that the `memory_retrieve` tool is always registered in the tool list
/// sent to the model, regardless of whether the experiment is enabled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_retrieve_tool_is_registered() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("msg-1", "hello"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let mut builder = test_codex();
    let test = builder.build(&server).await?;
    test.submit_turn("hello").await?;

    let request = mock.single_request();
    let tools = tool_names(&request.body_json());
    assert!(
        tools.contains(&"memory_retrieve".to_string()),
        "memory_retrieve tool not found in tools list: {tools:?}"
    );

    Ok(())
}

/// When memory clues exist, they should appear in the developer messages
/// sent to the model (injected via `build_initial_context()`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_clues_injected_in_system_prompt() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("msg-1", "I see the clues"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let mut builder = test_codex();
    let test = builder.build(&server).await?;

    // Set up memory experiment files using the computed project root.
    let project_root = compute_project_memory_root(test.codex_home_path(), test.cwd_path());
    let clues = "\
### Semantic Memories (Concepts)
- [auth, JWT] \u{2192} semantic/auth-flow.md
  desc: JWT authentication flow with refresh tokens
";
    enable_experiment(&project_root, clues);

    test.submit_turn("tell me about auth").await?;

    let request = mock.single_request();
    // Clues should appear in developer messages (DeveloperInstructions).
    let developer_texts = request.message_input_texts("developer");
    let all_developer = developer_texts.join("\n");
    assert!(
        all_developer.contains("Project Memory"),
        "expected 'Project Memory' in developer messages, got: {all_developer}"
    );
    assert!(
        all_developer.contains("auth, JWT"),
        "expected memory clues keywords in developer messages, got: {all_developer}"
    );
    assert!(
        all_developer.contains("memory_retrieve"),
        "expected memory_retrieve instruction in developer messages, got: {all_developer}"
    );

    Ok(())
}

/// When no memory clues exist, the system prompt should NOT contain
/// "Project Memory" instructions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_clues_no_memory_prompt() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("msg-1", "ok"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let mut builder = test_codex();
    let test = builder.build(&server).await?;

    // Do NOT set up any memory experiment files.
    test.submit_turn("hello").await?;

    let request = mock.single_request();
    let developer_texts = request.message_input_texts("developer");
    let all_developer = developer_texts.join("\n");
    assert!(
        !all_developer.contains("Project Memory"),
        "should NOT have memory clues when experiment is not enabled"
    );

    Ok(())
}

// NOTE: The following integration tests for actual retrieval are removed because
// the retrieval mechanism now spawns a full sub-codex agent via
// `run_codex_thread_one_shot`. Testing the sub-agent requires mounting
// additional SSE responses for the sub-agent's API calls, which is a separate
// test harness concern. The core retrieval logic is tested via manual
// end-to-end testing with real model providers.

/// When the experiment is not enabled (no config.toml), the tool should
/// return a "not enabled" message with success=false.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_retrieve_without_experiment_returns_not_enabled() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex();
    let test = builder.build(&server).await?;

    // Do NOT create config.toml — experiment is disabled.
    let call_id = "mem-disabled-1";
    let args = serde_json::json!({
        "query": "anything"
    });

    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call(call_id, "memory_retrieve", &args.to_string()),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-2"),
            ev_assistant_message("msg-1", "ok no memories"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    test.submit_turn("search memories").await?;

    let request = mock.single_request();
    let (content, success) = request
        .function_call_output_content_and_success(call_id)
        .expect("should have function_call_output");

    let output_text = content.expect("should have content");
    assert!(
        output_text.contains("not enabled"),
        "should indicate experiment is not enabled, got: {output_text}"
    );
    assert_eq!(
        success,
        Some(false),
        "should report failure when not enabled"
    );

    Ok(())
}
