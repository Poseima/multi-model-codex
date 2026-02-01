use std::collections::BTreeMap;

use crate::apply_patch;
use crate::apply_patch::InternalApplyPatchInvocation;
use crate::apply_patch::convert_apply_patch_to_protocol;
use crate::client_common::tools::ResponsesApiTool;
use crate::client_common::tools::ToolSpec;
use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::events::ToolEmitter;
use crate::tools::events::ToolEventCtx;
use crate::tools::handlers::parse_arguments;
use crate::tools::orchestrator::ToolOrchestrator;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use crate::tools::runtimes::apply_patch::ApplyPatchRequest;
use crate::tools::runtimes::apply_patch::ApplyPatchRuntime;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::spec::JsonSchema;
use async_trait::async_trait;
use codex_apply_patch::ApplyPatchFileChange;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;

pub struct StructuredEditHandler;

pub(crate) fn create_text_editor_tool() -> ToolSpec {
    let mut properties = BTreeMap::new();
    properties.insert(
        "command".to_string(),
        JsonSchema::String {
            description: Some(
                "The editing command to execute. One of: 'create', 'str_replace', 'delete'."
                    .to_string(),
            ),
        },
    );
    properties.insert(
        "path".to_string(),
        JsonSchema::String {
            description: Some("Relative path to the file to operate on.".to_string()),
        },
    );
    properties.insert(
        "file_text".to_string(),
        JsonSchema::String {
            description: Some(
                "Required for 'create' command. The full content of the new file.".to_string(),
            ),
        },
    );
    properties.insert(
        "old_str".to_string(),
        JsonSchema::String {
            description: Some(
                "Required for 'str_replace' command. The exact text to find in the file. Must match exactly once."
                    .to_string(),
            ),
        },
    );
    properties.insert(
        "new_str".to_string(),
        JsonSchema::String {
            description: Some(
                "Required for 'str_replace' command. The replacement text. Omit or set empty to delete old_str."
                    .to_string(),
            ),
        },
    );

    ToolSpec::Function(ResponsesApiTool {
        name: "text_editor".to_string(),
        description: r#"Edit files using structured commands.

Commands:
- **create**: Create a new file. Requires 'path' and 'file_text'.
- **str_replace**: Replace text in an existing file. Requires 'path', 'old_str', and 'new_str'. The 'old_str' must match exactly one location in the file.
- **delete**: Delete a file. Requires 'path'.

Examples:

Create a file:
  {"command": "create", "path": "hello.txt", "file_text": "Hello, world!\n"}

Replace text:
  {"command": "str_replace", "path": "src/main.rs", "old_str": "println!(\"old\")", "new_str": "println!(\"new\")"}

Delete a file:
  {"command": "delete", "path": "obsolete.txt"}
"#
        .to_string(),
        strict: false,
        parameters: JsonSchema::Object {
            properties,
            required: Some(vec!["command".to_string(), "path".to_string()]),
            additional_properties: Some(false.into()),
        },
    })
}

#[derive(Deserialize)]
struct StructuredEditArgs {
    command: String,
    path: String,
    #[serde(default)]
    file_text: Option<String>,
    #[serde(default)]
    old_str: Option<String>,
    #[serde(default)]
    new_str: Option<String>,
}

/// Number of context lines to include before and after a change in generated patches.
const CONTEXT_LINES: usize = 3;

#[async_trait]
impl ToolHandler for StructuredEditHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            tracker,
            call_id,
            tool_name,
            payload,
        } = invocation;

        let args: StructuredEditArgs = match payload {
            ToolPayload::Function { arguments } => parse_arguments(&arguments)?,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "text_editor handler received unsupported payload".to_string(),
                ));
            }
        };

        let cwd = turn.cwd.clone();
        let patch_string = match args.command.as_str() {
            "create" => {
                let file_text = args.file_text.ok_or_else(|| {
                    FunctionCallError::RespondToModel(
                        "create command requires 'file_text' parameter".to_string(),
                    )
                })?;
                generate_create_patch(&args.path, &file_text)
            }
            "str_replace" => {
                let old_str = args.old_str.ok_or_else(|| {
                    FunctionCallError::RespondToModel(
                        "str_replace command requires 'old_str' parameter".to_string(),
                    )
                })?;
                let new_str = args.new_str.unwrap_or_default();
                let file_path = cwd.join(&args.path);
                let file_content = std::fs::read_to_string(&file_path).map_err(|e| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to read file '{}': {e}",
                        args.path
                    ))
                })?;
                generate_str_replace_patch(&args.path, &old_str, &new_str, &file_content)?
            }
            "delete" => generate_delete_patch(&args.path),
            other => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "unknown command '{other}'. Expected 'create', 'str_replace', or 'delete'"
                )));
            }
        };

        // Delegate to the existing apply_patch pipeline.
        let command = vec!["apply_patch".to_string(), patch_string];
        match codex_apply_patch::maybe_parse_apply_patch_verified(&command, &cwd) {
            codex_apply_patch::MaybeApplyPatchVerified::Body(action) => {
                match apply_patch::apply_patch(turn.as_ref(), action).await {
                    InternalApplyPatchInvocation::Output(item) => {
                        let content = item?;
                        Ok(ToolOutput::Function {
                            content,
                            content_items: None,
                            success: Some(true),
                        })
                    }
                    InternalApplyPatchInvocation::DelegateToExec(apply) => {
                        let changes = convert_apply_patch_to_protocol(&apply.action);
                        let file_paths = file_paths_for_action(&apply.action);
                        let emitter =
                            ToolEmitter::apply_patch(changes.clone(), apply.auto_approved);
                        let event_ctx = ToolEventCtx::new(
                            session.as_ref(),
                            turn.as_ref(),
                            &call_id,
                            Some(&tracker),
                        );
                        emitter.begin(event_ctx).await;

                        let req = ApplyPatchRequest {
                            action: apply.action,
                            file_paths,
                            changes,
                            exec_approval_requirement: apply.exec_approval_requirement,
                            timeout_ms: None,
                            codex_exe: turn.codex_linux_sandbox_exe.clone(),
                        };

                        let mut orchestrator = ToolOrchestrator::new();
                        let mut runtime = ApplyPatchRuntime::new();
                        let tool_ctx = ToolCtx {
                            session: session.as_ref(),
                            turn: turn.as_ref(),
                            call_id: call_id.clone(),
                            tool_name: tool_name.to_string(),
                        };
                        let out = orchestrator
                            .run(&mut runtime, &req, &tool_ctx, &turn, turn.approval_policy)
                            .await;
                        let event_ctx = ToolEventCtx::new(
                            session.as_ref(),
                            turn.as_ref(),
                            &call_id,
                            Some(&tracker),
                        );
                        let content = emitter.finish(event_ctx, out).await?;
                        Ok(ToolOutput::Function {
                            content,
                            content_items: None,
                            success: Some(true),
                        })
                    }
                }
            }
            codex_apply_patch::MaybeApplyPatchVerified::CorrectnessError(err) => {
                Err(FunctionCallError::RespondToModel(format!(
                    "text_editor patch verification failed: {err}"
                )))
            }
            codex_apply_patch::MaybeApplyPatchVerified::ShellParseError(err) => {
                tracing::trace!("text_editor: failed to parse generated patch: {err:?}");
                Err(FunctionCallError::RespondToModel(
                    "text_editor: internal error generating patch".to_string(),
                ))
            }
            codex_apply_patch::MaybeApplyPatchVerified::NotApplyPatch => {
                Err(FunctionCallError::RespondToModel(
                    "text_editor: internal error – generated patch not recognized".to_string(),
                ))
            }
        }
    }
}

fn file_paths_for_action(action: &codex_apply_patch::ApplyPatchAction) -> Vec<AbsolutePathBuf> {
    let cwd = action.cwd.as_path();
    let mut keys = Vec::new();
    for (path, change) in action.changes() {
        if let Some(key) = AbsolutePathBuf::resolve_path_against_base(path, cwd).ok() {
            keys.push(key);
        }
        if let ApplyPatchFileChange::Update { move_path, .. } = change
            && let Some(dest) = move_path
            && let Some(key) = AbsolutePathBuf::resolve_path_against_base(dest, cwd).ok()
        {
            keys.push(key);
        }
    }
    keys
}

// ---------------------------------------------------------------------------
// Patch string generation
// ---------------------------------------------------------------------------

fn generate_create_patch(path: &str, file_text: &str) -> String {
    let mut patch = String::from("*** Begin Patch\n");
    patch.push_str(&format!("*** Add File: {path}\n"));
    for line in file_text.lines() {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    // Handle files that don't end with a newline – the last line still needs a +
    if !file_text.is_empty() && !file_text.ends_with('\n') {
        // The loop above already handled this via .lines()
    }
    patch.push_str("*** End Patch\n");
    patch
}

fn generate_delete_patch(path: &str) -> String {
    format!("*** Begin Patch\n*** Delete File: {path}\n*** End Patch\n")
}

fn generate_str_replace_patch(
    path: &str,
    old_str: &str,
    new_str: &str,
    file_content: &str,
) -> Result<String, FunctionCallError> {
    // Find all occurrences of old_str.
    let matches: Vec<usize> = file_content
        .match_indices(old_str)
        .map(|(idx, _)| idx)
        .collect();

    if matches.is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "old_str not found in {path}. Make sure the string matches exactly."
        )));
    }
    if matches.len() > 1 {
        return Err(FunctionCallError::RespondToModel(format!(
            "old_str appears {} times in {path}. Add more surrounding context to make it unique.",
            matches.len()
        )));
    }

    let match_start = matches[0];
    let match_end = match_start + old_str.len();

    let lines: Vec<&str> = file_content.lines().collect();

    // Find which lines the match spans.
    let mut byte_offset = 0;
    let mut start_line = 0;
    let mut end_line = 0;
    for (i, line) in lines.iter().enumerate() {
        let line_end = byte_offset + line.len() + 1; // +1 for \n
        if byte_offset <= match_start && match_start < line_end {
            start_line = i;
        }
        if byte_offset < match_end && match_end <= line_end {
            end_line = i;
            break;
        }
        byte_offset = line_end;
    }

    // Compute the new content for the affected lines by doing the replacement.
    let old_region: String = lines[start_line..=end_line].join("\n");
    let new_region = old_region.replacen(old_str, new_str, 1);
    let new_lines: Vec<&str> = new_region.lines().collect();

    // Context bounds.
    let ctx_start = start_line.saturating_sub(CONTEXT_LINES);
    let ctx_end = (end_line + CONTEXT_LINES + 1).min(lines.len());

    let mut patch = String::from("*** Begin Patch\n");
    patch.push_str(&format!("*** Update File: {path}\n"));
    patch.push_str("@@\n");

    // Pre-context lines.
    for i in ctx_start..start_line {
        patch.push(' ');
        patch.push_str(lines[i]);
        patch.push('\n');
    }

    // Old lines (removed).
    for i in start_line..=end_line {
        patch.push('-');
        patch.push_str(lines[i]);
        patch.push('\n');
    }

    // New lines (added).
    for line in &new_lines {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }

    // Post-context lines.
    for i in (end_line + 1)..ctx_end {
        patch.push(' ');
        patch.push_str(lines[i]);
        patch.push('\n');
    }

    patch.push_str("*** End Patch\n");
    Ok(patch)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use codex_apply_patch::MaybeApplyPatchVerified;
    use tempfile::TempDir;

    fn parse_patch(patch: &str, cwd: &Path) -> MaybeApplyPatchVerified {
        let argv = vec!["apply_patch".to_string(), patch.to_string()];
        codex_apply_patch::maybe_parse_apply_patch_verified(&argv, cwd)
    }

    #[test]
    fn create_patch_round_trips() {
        let tmp = TempDir::new().unwrap();
        let patch = generate_create_patch("new_file.txt", "hello\nworld\n");
        let result = parse_patch(&patch, tmp.path());
        match result {
            MaybeApplyPatchVerified::Body(action) => {
                let changes = action.changes();
                assert_eq!(changes.len(), 1);
                let (_, change) = changes.iter().next().unwrap();
                match change {
                    ApplyPatchFileChange::Add { content } => {
                        assert_eq!(content, "hello\nworld\n");
                    }
                    other => panic!("expected Add, got {other:?}"),
                }
            }
            other => panic!("expected Body, got {other:?}"),
        }
    }

    #[test]
    fn delete_patch_round_trips() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("to_delete.txt");
        std::fs::write(&file_path, "content").unwrap();

        let patch = generate_delete_patch("to_delete.txt");
        let result = parse_patch(&patch, tmp.path());
        match result {
            MaybeApplyPatchVerified::Body(action) => {
                let changes = action.changes();
                assert_eq!(changes.len(), 1);
                let (_, change) = changes.iter().next().unwrap();
                assert!(
                    matches!(change, ApplyPatchFileChange::Delete { .. }),
                    "expected Delete, got {change:?}"
                );
            }
            other => panic!("expected Body, got {other:?}"),
        }
    }

    #[test]
    fn str_replace_patch_round_trips() {
        let tmp = TempDir::new().unwrap();
        let file_content = "line one\nline two\nline three\nline four\nline five\n";
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, file_content).unwrap();

        let patch =
            generate_str_replace_patch("test.txt", "line two", "line TWO", file_content).unwrap();
        let result = parse_patch(&patch, tmp.path());
        match result {
            MaybeApplyPatchVerified::Body(action) => {
                let changes = action.changes();
                assert_eq!(changes.len(), 1);
                let (_, change) = changes.iter().next().unwrap();
                match change {
                    ApplyPatchFileChange::Update {
                        unified_diff,
                        move_path,
                        ..
                    } => {
                        assert!(unified_diff.contains("-line two"));
                        assert!(unified_diff.contains("+line TWO"));
                        assert!(move_path.is_none());
                    }
                    other => panic!("expected Update, got {other:?}"),
                }
            }
            other => panic!("expected Body, got {other:?}"),
        }
    }

    #[test]
    fn str_replace_includes_context_lines() {
        let file_content = "aaa\nbbb\nccc\nddd\nTARGET\neee\nfff\nggg\nhhh\n";
        let patch =
            generate_str_replace_patch("f.txt", "TARGET", "REPLACED", file_content).unwrap();
        // Should have 3 pre-context lines (bbb, ccc, ddd) and 3 post-context (eee, fff, ggg).
        assert!(patch.contains(" bbb\n"));
        assert!(patch.contains(" ccc\n"));
        assert!(patch.contains(" ddd\n"));
        assert!(patch.contains("-TARGET\n"));
        assert!(patch.contains("+REPLACED\n"));
        assert!(patch.contains(" eee\n"));
        assert!(patch.contains(" fff\n"));
        assert!(patch.contains(" ggg\n"));
    }

    #[test]
    fn str_replace_not_found_errors() {
        let result = generate_str_replace_patch("f.txt", "MISSING", "x", "hello\nworld\n");
        assert!(result.is_err());
    }

    #[test]
    fn str_replace_multiple_matches_errors() {
        let result = generate_str_replace_patch("f.txt", "line", "x", "line\nline\n");
        assert!(result.is_err());
    }

    #[test]
    fn str_replace_near_file_start() {
        let file_content = "first\nsecond\nthird\nfourth\n";
        let patch = generate_str_replace_patch("f.txt", "first", "FIRST", file_content).unwrap();
        // Should not have pre-context lines since match is at line 0.
        assert!(patch.contains("-first\n"));
        assert!(patch.contains("+FIRST\n"));
        assert!(patch.contains(" second\n"));
    }

    #[test]
    fn str_replace_near_file_end() {
        let file_content = "aaa\nbbb\nccc\nlast\n";
        let patch = generate_str_replace_patch("f.txt", "last", "LAST", file_content).unwrap();
        assert!(patch.contains("-last\n"));
        assert!(patch.contains("+LAST\n"));
        assert!(patch.contains(" ccc\n"));
    }
}
