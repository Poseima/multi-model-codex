//! Memory clues: lightweight index injected into the system prompt.
//!
//! Clues are generated from YAML frontmatter of memory files and stored as
//! `memory_clues.md` in the project memory directory. The main agent uses
//! these clues to decide when to call the `memory_retrieve` tool.

use crate::memory_experiment::get_project_memory_root;
use crate::memory_experiment::is_memory_empty;
use crate::memory_experiment::types::MemoryClue;
use crate::memory_experiment::types::MemoryType;
use crate::memory_experiment::types::parse_frontmatter;
use askama::Template;
use std::fmt::Write as _;
use std::path::Path;
use tracing::warn;

#[derive(Template)]
#[template(path = "memory_experiment/clues.md", escape = "none")]
struct ProjectMemoryCluesTemplate<'a> {
    clues_content: &'a str,
}

/// Build the memory clues prompt for system prompt injection.
///
/// Returns `None` if the experiment is not active or no clues exist yet.
/// Called from the `build_initial_context()` hook in `codex.rs`.
pub(crate) async fn build_clues_prompt(codex_home: &Path, cwd: &Path) -> Option<String> {
    let project_root = get_project_memory_root(codex_home, cwd);
    let clues_path = project_root.join("memory_clues.md");
    let clues_content = tokio::fs::read_to_string(&clues_path).await.ok()?;
    let trimmed = clues_content.trim();
    if trimmed.is_empty() {
        return None;
    }
    let template = ProjectMemoryCluesTemplate {
        clues_content: trimmed,
    };
    template.render().ok()
}

/// Regenerate `memory_clues.md` by scanning all memory files.
///
/// Called after archiving to keep the system prompt index fresh.
pub(crate) async fn regenerate_clues(project_root: &Path) -> std::io::Result<()> {
    let clues = scan_clues(project_root).await?;
    let content = format_clues(&clues);
    tokio::fs::write(project_root.join("memory_clues.md"), content).await
}

/// Ensure `memory_clues.md` exists when memory files are present.
///
/// Called at session start when the experiment is enabled. If memory files
/// exist but no clues index has been generated yet (e.g. first run after
/// enabling the feature flag), regenerate it automatically so the system
/// prompt contains clues on the very first turn.
pub(crate) async fn ensure_clues_fresh(codex_home: &Path, cwd: &Path) {
    let project_root = get_project_memory_root(codex_home, cwd);
    if is_memory_empty(&project_root).await {
        return; // No memories to index.
    }
    let clues_path = project_root.join("memory_clues.md");
    if clues_path.exists() {
        let content = tokio::fs::read_to_string(&clues_path)
            .await
            .unwrap_or_default();
        if !content.trim().is_empty() {
            return; // Clues already exist and are non-empty.
        }
    }
    // Memory files exist but no clues — regenerate.
    if let Err(e) = regenerate_clues(&project_root).await {
        warn!("auto-regenerate clues failed: {e}");
    }
}

/// Scan semantic/ and episodic/ directories for memory files and extract clues.
async fn scan_clues(project_root: &Path) -> std::io::Result<Vec<MemoryClue>> {
    let mut clues = Vec::new();
    for (subdir, expected_type) in [
        ("semantic", MemoryType::Semantic),
        ("episodic", MemoryType::Episodic),
    ] {
        let dir_path = project_root.join(subdir);
        let mut entries = match tokio::fs::read_dir(&dir_path).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !filename.ends_with(".md") {
                continue;
            }
            let content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => {
                    warn!("failed to read memory file {}: {e}", path.display());
                    continue;
                }
            };
            let Some((meta, _body)) = parse_frontmatter(&content) else {
                warn!("failed to parse frontmatter in {}", path.display());
                continue;
            };
            clues.push(MemoryClue {
                keywords: meta.keywords,
                filename: format!("{subdir}/{filename}"),
                summary: meta.summary,
                memory_type: expected_type,
                expires: meta.expires,
            });
        }
    }
    // Sort for deterministic output.
    clues.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(clues)
}

/// Format clues into the markdown content for `memory_clues.md`.
fn format_clues(clues: &[MemoryClue]) -> String {
    let mut out = String::new();

    let semantic: Vec<_> = clues
        .iter()
        .filter(|c| c.memory_type == MemoryType::Semantic)
        .collect();
    let episodic: Vec<_> = clues
        .iter()
        .filter(|c| c.memory_type == MemoryType::Episodic)
        .collect();

    if !semantic.is_empty() {
        out.push_str("### Semantic Memories (Concepts)\n");
        for clue in &semantic {
            format_clue_entry(&mut out, clue);
        }
        out.push('\n');
    }

    if !episodic.is_empty() {
        out.push_str("### Episodic Memories (Events)\n");
        for clue in &episodic {
            format_clue_entry(&mut out, clue);
        }
        out.push('\n');
    }

    if out.is_empty() {
        out.push_str("No memories yet.\n");
    }

    out
}

fn format_clue_entry(out: &mut String, clue: &MemoryClue) {
    let keywords = clue.keywords.join(", ");
    let expires_suffix = clue
        .expires
        .as_deref()
        .map(|e| format!(" (expires: {e})"))
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "- [{keywords}] \u{2192} {}{expires_suffix}",
        clue.filename
    );
    let _ = writeln!(out, "  desc: {}", clue.summary);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn format_clues_empty() {
        assert_eq!(format_clues(&[]), "No memories yet.\n");
    }

    #[test]
    fn format_clues_mixed() {
        let clues = vec![
            MemoryClue {
                keywords: vec!["auth".to_string(), "JWT".to_string()],
                filename: "semantic/auth-flow.md".to_string(),
                summary: "JWT auth flow".to_string(),
                memory_type: MemoryType::Semantic,
                expires: None,
            },
            MemoryClue {
                keywords: vec!["bug-fix".to_string()],
                filename: "episodic/csrf-fix.md".to_string(),
                summary: "Fixed CSRF".to_string(),
                memory_type: MemoryType::Episodic,
                expires: Some("2026-03-15".to_string()),
            },
        ];
        let result = format_clues(&clues);
        assert!(result.contains("### Semantic Memories"));
        assert!(result.contains("[auth, JWT] \u{2192} semantic/auth-flow.md"));
        assert!(result.contains("### Episodic Memories"));
        assert!(result.contains("(expires: 2026-03-15)"));
        assert!(result.contains("desc: Fixed CSRF"));
    }

    #[test]
    fn clue_template_renders() {
        let template = ProjectMemoryCluesTemplate {
            clues_content: "test clues",
        };
        let rendered = template.render().unwrap();
        assert!(rendered.contains("## Project Memory"));
        assert!(rendered.contains("test clues"));
        assert!(rendered.contains("memory_retrieve"));
    }

    #[tokio::test]
    async fn regenerate_clues_writes_file_from_memory_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let semantic = root.join("semantic");
        tokio::fs::create_dir_all(&semantic).await.unwrap();
        tokio::fs::write(
            semantic.join("auth.md"),
            "---\ntype: semantic\nkeywords: [auth, JWT]\nsummary: Auth flow\ncreated: \"2026-01-01T00:00:00Z\"\nlast_updated: \"2026-01-01T00:00:00Z\"\n---\n\nContent.",
        )
        .await
        .unwrap();

        regenerate_clues(root).await.unwrap();

        let clues_path = root.join("memory_clues.md");
        assert!(clues_path.exists());
        let content = tokio::fs::read_to_string(&clues_path).await.unwrap();
        assert!(content.contains("### Semantic Memories"));
        assert!(content.contains("auth, JWT"));
        assert!(content.contains("Auth flow"));
        assert!(content.contains("semantic/auth.md"));
    }

    #[tokio::test]
    async fn regenerate_clues_handles_empty_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // No semantic/ or episodic/ dirs exist.

        regenerate_clues(root).await.unwrap();

        let content = tokio::fs::read_to_string(root.join("memory_clues.md"))
            .await
            .unwrap();
        assert_eq!(content, "No memories yet.\n");
    }

    #[tokio::test]
    async fn regenerate_clues_includes_both_types() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let semantic = root.join("semantic");
        let episodic = root.join("episodic");
        tokio::fs::create_dir_all(&semantic).await.unwrap();
        tokio::fs::create_dir_all(&episodic).await.unwrap();

        tokio::fs::write(
            semantic.join("concepts.md"),
            "---\ntype: semantic\nkeywords: [design]\nsummary: Design patterns\ncreated: \"2026-01-01T00:00:00Z\"\nlast_updated: \"2026-01-01T00:00:00Z\"\n---\n\nContent.",
        )
        .await
        .unwrap();
        tokio::fs::write(
            episodic.join("event.md"),
            "---\ntype: episodic\nkeywords: [deploy]\nsummary: Deployed v2\ncreated: \"2026-02-01T00:00:00Z\"\nlast_updated: \"2026-02-01T00:00:00Z\"\nexpires: \"2026-04-01T00:00:00Z\"\n---\n\nContent.",
        )
        .await
        .unwrap();

        regenerate_clues(root).await.unwrap();

        let content = tokio::fs::read_to_string(root.join("memory_clues.md"))
            .await
            .unwrap();
        assert!(content.contains("### Semantic Memories"));
        assert!(content.contains("### Episodic Memories"));
        assert!(content.contains("design"));
        assert!(content.contains("deploy"));
        assert!(content.contains("(expires: 2026-04-01T00:00:00Z)"));
    }

    #[tokio::test]
    async fn scan_clues_skips_non_markdown_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let semantic = root.join("semantic");
        tokio::fs::create_dir_all(&semantic).await.unwrap();

        // Write a .txt file — should be skipped.
        tokio::fs::write(semantic.join("notes.txt"), "not a memory file")
            .await
            .unwrap();
        // Write a proper .md file.
        tokio::fs::write(
            semantic.join("real.md"),
            "---\ntype: semantic\nkeywords: [test]\nsummary: Real memory\ncreated: \"2026-01-01T00:00:00Z\"\nlast_updated: \"2026-01-01T00:00:00Z\"\n---\n\nContent.",
        )
        .await
        .unwrap();

        let clues = scan_clues(root).await.unwrap();
        assert_eq!(clues.len(), 1);
        assert_eq!(clues[0].filename, "semantic/real.md");
    }

    #[tokio::test]
    async fn build_clues_prompt_returns_none_when_no_clues_file() {
        let tmp = tempfile::tempdir().unwrap();
        // No memory_clues.md exists.
        let result = build_clues_prompt(tmp.path(), tmp.path()).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn build_clues_prompt_returns_none_for_empty_clues() {
        let tmp = tempfile::tempdir().unwrap();
        let root = get_project_memory_root(tmp.path(), tmp.path());
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("memory_clues.md"), "   \n  \n").unwrap();

        let result = build_clues_prompt(tmp.path(), tmp.path()).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn build_clues_prompt_renders_template_with_content() {
        let tmp = tempfile::tempdir().unwrap();
        let root = get_project_memory_root(tmp.path(), tmp.path());
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("memory_clues.md"),
            "### Semantic Memories\n- [auth] → semantic/auth.md\n  desc: Auth flow\n",
        )
        .unwrap();

        let result = build_clues_prompt(tmp.path(), tmp.path()).await;
        assert!(result.is_some());
        let rendered = result.unwrap();
        assert!(rendered.contains("## Project Memory"));
        assert!(rendered.contains("memory_retrieve"));
        assert!(rendered.contains("[auth]"));
        assert!(rendered.contains("Auth flow"));
    }

    #[tokio::test]
    async fn ensure_clues_fresh_generates_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let codex_home = tmp.path();
        let cwd = std::path::Path::new("/tmp/ensure_clues_fresh_test");
        let root = get_project_memory_root(codex_home, cwd);
        let semantic = root.join("semantic");
        tokio::fs::create_dir_all(&semantic).await.unwrap();
        tokio::fs::write(
            semantic.join("topic.md"),
            "---\ntype: semantic\nkeywords: [test]\nsummary: Test memory\ncreated: \"2026-01-01T00:00:00Z\"\nlast_updated: \"2026-01-01T00:00:00Z\"\n---\n\nContent.",
        )
        .await
        .unwrap();

        // No memory_clues.md exists yet.
        assert!(!root.join("memory_clues.md").exists());

        ensure_clues_fresh(codex_home, cwd).await;

        // Clues should now be generated.
        let content = tokio::fs::read_to_string(root.join("memory_clues.md"))
            .await
            .unwrap();
        assert!(content.contains("### Semantic Memories"));
        assert!(content.contains("test"));
        assert!(content.contains("Test memory"));
    }

    #[tokio::test]
    async fn ensure_clues_fresh_noop_when_clues_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let codex_home = tmp.path();
        let cwd = std::path::Path::new("/tmp/ensure_clues_fresh_noop_test");
        let root = get_project_memory_root(codex_home, cwd);
        let semantic = root.join("semantic");
        tokio::fs::create_dir_all(&semantic).await.unwrap();
        tokio::fs::write(
            semantic.join("topic.md"),
            "---\ntype: semantic\nkeywords: [test]\nsummary: Test\ncreated: \"2026-01-01T00:00:00Z\"\nlast_updated: \"2026-01-01T00:00:00Z\"\n---\n\nContent.",
        )
        .await
        .unwrap();

        // Pre-existing clues with custom content.
        let custom_clues = "Custom clues content";
        tokio::fs::write(root.join("memory_clues.md"), custom_clues)
            .await
            .unwrap();

        ensure_clues_fresh(codex_home, cwd).await;

        // Custom clues should be unchanged.
        let content = tokio::fs::read_to_string(root.join("memory_clues.md"))
            .await
            .unwrap();
        assert_eq!(content, custom_clues);
    }

    #[tokio::test]
    async fn ensure_clues_fresh_noop_when_no_memories() {
        let tmp = tempfile::tempdir().unwrap();
        let codex_home = tmp.path();
        let cwd = std::path::Path::new("/tmp/ensure_clues_fresh_empty_test");
        let root = get_project_memory_root(codex_home, cwd);
        tokio::fs::create_dir_all(root.join("semantic"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(root.join("episodic"))
            .await
            .unwrap();

        ensure_clues_fresh(codex_home, cwd).await;

        // No clues file should be created when there are no memories.
        assert!(!root.join("memory_clues.md").exists());
    }
}
