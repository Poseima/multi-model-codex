//! Integration tests for the fork memory experiment system.
//!
//! These tests verify the end-to-end flow:
//! - Memory clues are injected into the system prompt when enabled
//! - The clues template references spawn_agent with memory_retriever role
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
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;

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

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

/// When memory clues exist, they should appear in the system prompt
/// (instructions field) rather than as separate developer messages.
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
    // Clues should appear in the instructions (system prompt), not developer messages.
    let instructions = request.instructions_text();
    assert!(
        instructions.contains("Project Memory"),
        "expected 'Project Memory' in instructions, got: {instructions}"
    );
    assert!(
        instructions.contains("auth, JWT"),
        "expected memory clues keywords in instructions, got: {instructions}"
    );
    assert!(
        instructions.contains("spawn_agent"),
        "expected spawn_agent instruction in instructions, got: {instructions}"
    );
    // Verify clues are NOT in developer messages (moved to system prompt).
    let developer_texts = request.message_input_texts("developer");
    let all_developer = developer_texts.join("\n");
    assert!(
        !all_developer.contains("Project Memory"),
        "memory clues should NOT be in developer messages when experiment is active"
    );

    Ok(())
}

/// When no memory clues exist, neither the system prompt nor developer
/// messages should contain "Project Memory" instructions.
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
    let instructions = request.instructions_text();
    assert!(
        !instructions.contains("Project Memory"),
        "should NOT have memory clues in instructions when experiment is not enabled"
    );
    let developer_texts = request.message_input_texts("developer");
    let all_developer = developer_texts.join("\n");
    assert!(
        !all_developer.contains("Project Memory"),
        "should NOT have memory clues in developer messages when experiment is not enabled"
    );

    Ok(())
}
