//! Memory archiver utilities.
//!
//! The actual archive work is performed by a visible sub-Codex agent spawned
//! by [`crate::tasks::ArchiveTask`]. This module provides shared helpers used
//! by that task, most notably [`serialize_history`] which converts conversation
//! history into a human-readable transcript for the archive agent's input.

use crate::compact::content_items_to_text;
use codex_protocol::models::LocalShellAction;
use codex_protocol::models::ResponseItem;

/// Maximum characters to include from a single tool result.
/// Longer outputs are truncated with a `... [truncated]` suffix.
const TOOL_RESULT_MAX_CHARS: usize = 2000;

/// Serialize conversation history to a readable text transcript.
///
/// Includes user/assistant messages, tool calls (function + shell), and tool
/// results (truncated). Skips `developer` messages, system-injected `user`
/// messages, reasoning items, and other non-conversational metadata.
pub(crate) fn serialize_history(items: &[ResponseItem]) -> String {
    let mut transcript = String::new();
    for item in items {
        let entry = match item {
            ResponseItem::Message { role, content, .. } => {
                let Some(text) = content_items_to_text(content) else {
                    continue;
                };
                // Skip developer messages (permissions, system prompts, collaboration mode).
                if role == "developer" {
                    continue;
                }
                // Skip system-injected user messages.
                if role == "user" && is_system_message(&text) {
                    continue;
                }
                // Strip <think>...</think> blocks from assistant messages.
                // These are model reasoning artifacts that confuse archive agents
                // (e.g. MiniMax interprets them as its own chain-of-thought).
                let text = if role == "assistant" {
                    strip_think_tags(&text)
                } else {
                    text
                };
                format!("[{role}]\n{text}")
            }
            ResponseItem::FunctionCall {
                name, arguments, ..
            } => {
                format!("[tool_call: {name}]\n{arguments}")
            }
            ResponseItem::FunctionCallOutput { output, .. } => {
                let text = output.body.to_text().unwrap_or_default();
                let truncated = truncate_text(&text, TOOL_RESULT_MAX_CHARS);
                format!("[tool_result]\n{truncated}")
            }
            ResponseItem::LocalShellCall { action, .. } => {
                let cmd = match action {
                    LocalShellAction::Exec(exec) => exec.command.join(" "),
                };
                format!("[tool_call: shell]\n{cmd}")
            }
            // Skip everything else: Reasoning, CustomToolCall/Output,
            // WebSearchCall, GhostSnapshot, Compaction, Other.
            _ => continue,
        };

        if !transcript.is_empty() {
            transcript.push_str("\n\n");
        }
        transcript.push_str(&entry);
    }
    transcript
}

/// Truncate text to `max_chars`, appending `... [truncated]` if cut.
fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    // Find a safe char boundary at or before max_chars.
    let boundary = text
        .char_indices()
        .take_while(|(i, _)| *i <= max_chars)
        .last()
        .map_or(0, |(i, _)| i);
    format!("{}... [truncated]", &text[..boundary])
}

/// Remove `<think>...</think>` blocks from text.
///
/// Model reasoning tags have no archival value and actively confuse archive
/// agents that use `<think>` for their own chain-of-thought (e.g. MiniMax).
fn strip_think_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        result.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("</think>") {
            rest = &rest[start + end + "</think>".len()..];
        } else {
            // Unclosed <think> tag — drop the rest.
            return result.trim().to_string();
        }
    }
    result.push_str(rest);
    result.trim().to_string()
}

/// Detect system-injected messages by their content prefix.
///
/// These are messages injected into the conversation by the Codex runtime
/// (permissions, AGENTS.md, environment context, etc.) that carry no
/// conversational knowledge worth archiving.
fn is_system_message(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("<permissions")
        || trimmed.starts_with("# AGENTS.md")
        || trimmed.starts_with("<environment_context>")
        || trimmed.starts_with("<INSTRUCTIONS>")
        || trimmed.starts_with("<collaboration_mode>")
        || trimmed.starts_with("[System:")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_protocol::models::LocalShellExecAction;
    use codex_protocol::models::LocalShellStatus;
    use pretty_assertions::assert_eq;

    #[test]
    fn serialize_history_extracts_messages() {
        let items = vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "How does auth work?".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
            ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "Auth uses JWT tokens.".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
        ];

        let transcript = serialize_history(&items);
        assert!(transcript.contains("[user]\nHow does auth work?"));
        assert!(transcript.contains("[assistant]\nAuth uses JWT tokens."));
    }

    #[test]
    fn serialize_history_includes_tool_calls() {
        let items = vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "Hello".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
            ResponseItem::FunctionCall {
                id: None,
                call_id: "call_1".to_string(),
                name: "text_editor".to_string(),
                arguments: r#"{"command":"view","path":"/src/main.rs"}"#.to_string(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call_1".to_string(),
                output: FunctionCallOutputPayload::from_text("fn main() {}".to_string()),
            },
            ResponseItem::LocalShellCall {
                id: None,
                call_id: Some("call_2".to_string()),
                status: LocalShellStatus::Completed,
                action: LocalShellAction::Exec(LocalShellExecAction {
                    command: vec!["cargo".to_string(), "test".to_string()],
                    timeout_ms: None,
                    working_directory: None,
                    env: None,
                    user: None,
                }),
            },
        ];

        let transcript = serialize_history(&items);
        assert!(transcript.contains("[user]\nHello"));
        assert!(transcript.contains("[tool_call: text_editor]"));
        assert!(transcript.contains(r#"{"command":"view","path":"/src/main.rs"}"#));
        assert!(transcript.contains("[tool_result]\nfn main() {}"));
        assert!(transcript.contains("[tool_call: shell]\ncargo test"));
    }

    #[test]
    fn serialize_history_truncates_long_tool_results() {
        let long_output = "x".repeat(3000);
        let items = vec![ResponseItem::FunctionCallOutput {
            call_id: "call_1".to_string(),
            output: FunctionCallOutputPayload::from_text(long_output),
        }];

        let transcript = serialize_history(&items);
        assert!(transcript.contains("... [truncated]"));
        // The truncated result should be roughly TOOL_RESULT_MAX_CHARS + prefix + suffix.
        assert!(transcript.len() < 2200);
    }

    #[test]
    fn serialize_history_empty() {
        let transcript = serialize_history(&[]);
        assert_eq!(transcript, "");
    }

    #[test]
    fn serialize_history_filters_system_messages() {
        let items = vec![
            // Developer message (permissions) — should be skipped.
            ResponseItem::Message {
                id: None,
                role: "developer".to_string(),
                content: vec![ContentItem::InputText {
                    text: "You are a helpful assistant.".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
            // System-injected user message (AGENTS.md) — should be skipped.
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "# AGENTS.md\n\nCollaboration instructions...".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
            // System-injected user message (permissions) — should be skipped.
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "<permissions>\nallow all\n</permissions>".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
            // System-injected user message (environment context) — should be skipped.
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "<environment_context>\nOS: macOS\n</environment_context>".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
            // System-injected user message (archive handoff) — should be skipped.
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "[System: memory archiving complete.]".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
            // Real user message — should be kept.
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "How does auth work?".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
            // Real assistant message — should be kept.
            ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "Auth uses JWT tokens.".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
        ];

        let transcript = serialize_history(&items);
        assert_eq!(
            transcript,
            "[user]\nHow does auth work?\n\n[assistant]\nAuth uses JWT tokens."
        );
    }

    #[test]
    fn is_system_message_detects_known_prefixes() {
        assert!(is_system_message(
            "<permissions>\nallow all\n</permissions>"
        ));
        assert!(is_system_message("# AGENTS.md\ncontent"));
        assert!(is_system_message("<environment_context>\nOS: macOS"));
        assert!(is_system_message("<INSTRUCTIONS>\nsome instructions"));
        assert!(is_system_message("<collaboration_mode>\nmode"));
        assert!(is_system_message("[System: memory archiving complete.]"));
        // Leading whitespace should be tolerated.
        assert!(is_system_message("  <permissions>\nindented"));
    }

    #[test]
    fn is_system_message_allows_normal_text() {
        assert!(!is_system_message("How does auth work?"));
        assert!(!is_system_message("Hello, can you help me?"));
        assert!(!is_system_message(""));
    }

    #[test]
    fn truncate_text_short_string() {
        assert_eq!(truncate_text("hello", 10), "hello");
    }

    #[test]
    fn truncate_text_exact_limit() {
        let text = "a".repeat(100);
        assert_eq!(truncate_text(&text, 100), text);
    }

    #[test]
    fn truncate_text_over_limit() {
        let text = "a".repeat(200);
        let result = truncate_text(&text, 100);
        assert!(result.ends_with("... [truncated]"));
        assert!(result.len() < 200);
    }

    #[test]
    fn strip_think_tags_removes_blocks() {
        let input = "<think>\nLet me check the code.\n</think>\nHere is the answer.";
        assert_eq!(strip_think_tags(input), "Here is the answer.");
    }

    #[test]
    fn strip_think_tags_no_tags() {
        assert_eq!(strip_think_tags("plain text"), "plain text");
    }

    #[test]
    fn strip_think_tags_multiple_blocks() {
        let input = "<think>first</think>middle<think>second</think>end";
        assert_eq!(strip_think_tags(input), "middleend");
    }

    #[test]
    fn strip_think_tags_unclosed() {
        let input = "before<think>unclosed reasoning";
        assert_eq!(strip_think_tags(input), "before");
    }

    #[test]
    fn serialize_history_strips_think_from_assistant() {
        let items = vec![ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "<think>\nLet me reason.\n</think>\nThe answer is 42.".to_string(),
            }],
            end_turn: None,
            phase: None,
        }];

        let transcript = serialize_history(&items);
        assert_eq!(transcript, "[assistant]\nThe answer is 42.");
        assert!(!transcript.contains("<think>"));
    }
}
