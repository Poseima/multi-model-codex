//! Memory retrieval via a full sub-codex research agent.
//!
//! Spawns a sub-codex agent (via `run_codex_thread_one_shot`) that receives the
//! query + memory clues, reads memory files using its tools, and returns a
//! synthesized research result. Mirrors the archive agent pattern in
//! `tasks/archive.rs`.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::codex::Session;
use crate::codex::TurnContext;
use crate::codex_delegate::run_codex_thread_one_shot;
use crate::config::Constrained;
use crate::memory_experiment;
use crate::protocol::SubAgentSource;

/// System prompt for the retrieval research agent.
const RETRIEVAL_PROMPT: &str =
    include_str!("../../templates/memory_experiment/retrieval_prompt.md");

/// Retrieve memory content by spawning a research sub-agent.
///
/// The agent receives the query + memory clues, reads relevant files using its
/// tools, and returns a comprehensive synthesized research result.
pub(crate) async fn retrieve(
    project_root: &Path,
    query: &str,
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
) -> Result<String, String> {
    if memory_experiment::is_memory_empty(project_root).await {
        return Ok("No memories found for this project.".to_string());
    }

    // Read memory clues — the compact index the agent uses to decide what to read.
    let clues_content = tokio::fs::read_to_string(project_root.join("memory_clues.md"))
        .await
        .unwrap_or_default();

    let cancellation_token = CancellationToken::new();

    let receiver = match start_retrieval_conversation(
        project_root,
        query,
        &clues_content,
        Arc::clone(session),
        Arc::clone(turn),
        cancellation_token,
    )
    .await
    {
        Some(rx) => rx,
        None => return Ok("No relevant memories found for your query.".to_string()),
    };

    match process_retrieval_events(receiver).await {
        Some(result) => Ok(result),
        None => Ok("Memory retrieval was interrupted.".to_string()),
    }
}

/// Spawn a sub-codex research agent in the memory directory.
///
/// Follows the archive agent pattern: clone parent config, override sandbox /
/// approval / cwd, build two-message input (data first, instructions last for
/// recency), then call `run_codex_thread_one_shot`.
async fn start_retrieval_conversation(
    project_root: &Path,
    query: &str,
    clues_content: &str,
    session: Arc<Session>,
    ctx: Arc<TurnContext>,
    cancellation_token: CancellationToken,
) -> Option<async_channel::Receiver<Event>> {
    let config = ctx.config.clone();
    let mut sub_agent_config = config.as_ref().clone();

    // Clear system instructions — the retrieval prompt is injected as the LAST
    // user message so it sits closest to the model's generation point.
    sub_agent_config.base_instructions = None;

    // Auto-approve all tool calls (the subagent only reads files in the memory dir).
    sub_agent_config.permissions.approval_policy = Constrained::allow_only(AskForApproval::Never);

    // Allow the agent to operate in the memory directory. WorkspaceWrite gives
    // full tool access; the agent only reads but needs shell/text_editor tools.
    sub_agent_config.permissions.sandbox_policy =
        Constrained::allow_only(SandboxPolicy::new_workspace_write_policy());

    // No MCP servers needed — the retrieval agent only reads memory files.
    sub_agent_config.mcp_servers = Constrained::allow_only(std::collections::HashMap::new());

    // Set the working directory to the project memory root.
    sub_agent_config.cwd = project_root.to_path_buf();

    // Two-message structure (instructions last for recency):
    // 1. Query + memory clues (data)
    // 2. Research instructions (closest to model generation)
    let initial_input = vec![
        UserInput::Text {
            text: format!(
                "<query>\n{query}\n</query>\n\n\
                 <memory_clues>\n{clues_content}\n</memory_clues>"
            ),
            text_elements: Vec::new(),
        },
        UserInput::Text {
            text: format!(
                "The above contains a QUERY from the main agent and MEMORY CLUES \
                 listing available memory files.\n\
                 Your ONLY task is to research the memory files and produce a \
                 synthesis that answers the query.\n\n\
                 {RETRIEVAL_PROMPT}"
            ),
            text_elements: Vec::new(),
        },
    ];

    let auth_manager = Arc::clone(&session.services.auth_manager);
    let models_manager = Arc::clone(&session.services.models_manager);

    (run_codex_thread_one_shot(
        sub_agent_config,
        auth_manager,
        models_manager,
        initial_input,
        session,
        ctx,
        cancellation_token,
        None,
        SubAgentSource::MemoryRetrieval,
    )
    .await)
        .ok()
        .map(|io| io.rx_event)
}

/// Consume retrieval sub-agent events and capture the research result.
///
/// Unlike the archive agent (which forwards events to the TUI), retrieval
/// suppresses all events — only the final `last_agent_message` is captured
/// and returned to the tool handler.
async fn process_retrieval_events(receiver: async_channel::Receiver<Event>) -> Option<String> {
    while let Ok(event) = receiver.recv().await {
        match event.msg {
            EventMsg::TurnComplete(task_complete) => {
                return task_complete.last_agent_message;
            }
            EventMsg::TurnAborted(_) => {
                return None;
            }
            // Suppress all other events — retrieval runs silently inside a tool call.
            _ => {}
        }
    }
    None
}

/// Read specified memory files and return their content concatenated.
pub(crate) async fn read_memory_files(
    project_root: &Path,
    filenames: &[String],
) -> Result<String, String> {
    let mut output = String::new();
    let mut found = 0;

    for filename in filenames {
        let path = project_root.join(filename);
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => {
                if found > 0 {
                    output.push_str("\n---\n\n");
                }
                let _ = writeln!(output, "## File: {filename}");
                output.push_str(content.trim());
                output.push('\n');
                found += 1;
            }
            Err(e) => {
                warn!("memory_retrieve: could not read {filename}: {e}");
                let _ = writeln!(output, "\n[File not found: {filename}]");
            }
        }
    }

    if found == 0 {
        return Ok("No matching memory files found.".to_string());
    }

    Ok(output)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_memory_files_returns_content() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let semantic = root.join("semantic");
        tokio::fs::create_dir_all(&semantic).await.unwrap();
        tokio::fs::write(
            semantic.join("test.md"),
            "---\ntype: semantic\nkeywords: [test]\nsummary: Test\ncreated: \"2026-01-01T00:00:00Z\"\nlast_updated: \"2026-01-01T00:00:00Z\"\n---\n\n# Test\nBody.",
        )
        .await
        .unwrap();

        let result = read_memory_files(root, &["semantic/test.md".to_string()])
            .await
            .unwrap();
        assert!(result.contains("## File: semantic/test.md"));
        assert!(result.contains("# Test"));
        assert!(result.contains("Body."));
    }

    #[tokio::test]
    async fn read_memory_files_handles_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let result = read_memory_files(tmp.path(), &["nonexistent.md".to_string()])
            .await
            .unwrap();
        assert!(result.contains("No matching memory files found"));
    }

    #[tokio::test]
    async fn read_memory_files_returns_multiple_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let semantic = root.join("semantic");
        tokio::fs::create_dir_all(&semantic).await.unwrap();
        tokio::fs::write(
            semantic.join("auth.md"),
            "---\ntype: semantic\nkeywords: [auth]\nsummary: Auth\ncreated: \"2026-01-01T00:00:00Z\"\nlast_updated: \"2026-01-01T00:00:00Z\"\n---\n\nAuth content.",
        )
        .await
        .unwrap();
        tokio::fs::write(
            semantic.join("db.md"),
            "---\ntype: semantic\nkeywords: [db]\nsummary: Database\ncreated: \"2026-01-01T00:00:00Z\"\nlast_updated: \"2026-01-01T00:00:00Z\"\n---\n\nDB content.",
        )
        .await
        .unwrap();

        let result = read_memory_files(
            root,
            &["semantic/auth.md".to_string(), "semantic/db.md".to_string()],
        )
        .await
        .unwrap();
        assert!(result.contains("## File: semantic/auth.md"));
        assert!(result.contains("Auth content."));
        assert!(result.contains("## File: semantic/db.md"));
        assert!(result.contains("DB content."));
        // Files should be separated by a delimiter.
        assert!(result.contains("---"));
    }

    #[tokio::test]
    async fn read_memory_files_mixed_found_and_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let semantic = root.join("semantic");
        tokio::fs::create_dir_all(&semantic).await.unwrap();
        tokio::fs::write(semantic.join("real.md"), "Real content.")
            .await
            .unwrap();

        let result = read_memory_files(
            root,
            &[
                "semantic/real.md".to_string(),
                "semantic/missing.md".to_string(),
            ],
        )
        .await
        .unwrap();
        assert!(result.contains("## File: semantic/real.md"));
        assert!(result.contains("Real content."));
        assert!(result.contains("[File not found: semantic/missing.md]"));
    }

    #[tokio::test]
    async fn retrieve_returns_empty_when_no_memories() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Create empty semantic and episodic directories.
        tokio::fs::create_dir_all(root.join("semantic"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(root.join("episodic"))
            .await
            .unwrap();

        // We can't call retrieve() without a full session, but we can test
        // the is_memory_empty check directly.
        assert!(memory_experiment::is_memory_empty(root).await);
    }
}
