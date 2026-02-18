//! Types for the memory experiment system.
//!
//! Memory files use YAML frontmatter (delimited by `---`) followed by
//! markdown content. The frontmatter is parsed into [`MemoryMetadata`].

use serde::Deserialize;
use serde::Serialize;

/// Metadata from a memory file's YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct MemoryMetadata {
    #[serde(rename = "type")]
    pub memory_type: MemoryType,
    pub keywords: Vec<String>,
    #[serde(default)]
    pub related_files: Vec<String>,
    pub summary: String,
    pub created: String,
    pub last_updated: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
}

/// The two memory types supported by the experiment.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MemoryType {
    Semantic,
    Episodic,
}

/// A lightweight clue entry for the system prompt index (no full content).
#[derive(Debug, Clone)]
pub(crate) struct MemoryClue {
    pub keywords: Vec<String>,
    pub filename: String,
    pub summary: String,
    pub memory_type: MemoryType,
    pub expires: Option<String>,
}

/// Experiment-local configuration read from `config.toml` in the project
/// memory directory. All fields are optional with sensible defaults.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ExperimentConfig {
    /// Model used for retrieval ranking (fast model).
    #[serde(default = "default_retrieval_model")]
    pub retrieval_model: String,
    /// Model used for archiving extraction.
    #[serde(default = "default_archive_model")]
    pub archive_model: String,
    /// Number of days before episodic memories expire.
    #[serde(default = "default_episodic_expiry_days")]
    pub episodic_expiry_days: u32,
}

impl Default for ExperimentConfig {
    fn default() -> Self {
        Self {
            retrieval_model: default_retrieval_model(),
            archive_model: default_archive_model(),
            episodic_expiry_days: default_episodic_expiry_days(),
        }
    }
}

fn default_retrieval_model() -> String {
    "gpt-5.3-codex-spark".to_string()
}

fn default_archive_model() -> String {
    "gpt-5.3-codex".to_string()
}

fn default_episodic_expiry_days() -> u32 {
    30
}

/// Parse YAML frontmatter from a memory markdown file.
///
/// The file must start with `---\n`, followed by YAML, then another `---\n`.
/// Returns the parsed metadata and the remaining markdown body.
pub(crate) fn parse_frontmatter(content: &str) -> Option<(MemoryMetadata, &str)> {
    let content = content.trim_start();
    let rest = content.strip_prefix("---")?;
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let end_idx = rest.find("\n---")?;
    let yaml_str = &rest[..end_idx];
    let body_start = end_idx + 4; // skip "\n---"
    let body = if body_start < rest.len() {
        rest[body_start..]
            .strip_prefix('\n')
            .unwrap_or(&rest[body_start..])
    } else {
        ""
    };
    let metadata: MemoryMetadata = serde_yaml::from_str(yaml_str).ok()?;
    Some((metadata, body))
}

/// Serialize metadata as YAML frontmatter + body into a complete memory file.
pub(crate) fn format_memory_file(metadata: &MemoryMetadata, body: &str) -> String {
    let yaml = serde_yaml::to_string(metadata).unwrap_or_default();
    format!("---\n{yaml}---\n\n{body}\n")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_frontmatter_roundtrip() {
        let input = "\
---
type: semantic
keywords: [auth, JWT]
related_files: [src/auth.rs]
summary: Auth flow
created: \"2026-02-10T14:32:00Z\"
last_updated: \"2026-02-13T09:15:00Z\"
---

# Authentication Flow
JWT-based auth.
";
        let (meta, body) = parse_frontmatter(input).unwrap();
        assert_eq!(meta.memory_type, MemoryType::Semantic);
        assert_eq!(meta.keywords, vec!["auth", "JWT"]);
        assert_eq!(meta.summary, "Auth flow");
        assert_eq!(meta.expires, None);
        assert!(body.contains("Authentication Flow"));
    }

    #[test]
    fn parse_frontmatter_episodic_with_expires() {
        let input = "\
---
type: episodic
keywords: [bug-fix, CSRF]
summary: Fixed CSRF bug
created: \"2026-02-13T00:00:00Z\"
last_updated: \"2026-02-13T00:00:00Z\"
expires: \"2026-03-15T00:00:00Z\"
---

Fixed the CSRF token validation.
";
        let (meta, body) = parse_frontmatter(input).unwrap();
        assert_eq!(meta.memory_type, MemoryType::Episodic);
        assert_eq!(meta.expires, Some("2026-03-15T00:00:00Z".to_string()));
        assert!(body.contains("CSRF token"));
    }

    #[test]
    fn parse_frontmatter_returns_none_for_missing_delimiters() {
        assert!(parse_frontmatter("no frontmatter here").is_none());
        assert!(parse_frontmatter("---\nno closing delimiter").is_none());
    }

    #[test]
    fn format_memory_file_produces_parseable_output() {
        let meta = MemoryMetadata {
            memory_type: MemoryType::Semantic,
            keywords: vec!["test".to_string()],
            related_files: vec![],
            summary: "Test memory".to_string(),
            created: "2026-02-16T00:00:00Z".to_string(),
            last_updated: "2026-02-16T00:00:00Z".to_string(),
            expires: None,
        };
        let formatted = format_memory_file(&meta, "# Content\nSome body.");
        let (parsed_meta, parsed_body) = parse_frontmatter(&formatted).unwrap();
        assert_eq!(parsed_meta, meta);
        assert!(parsed_body.contains("Some body."));
    }

    #[test]
    fn experiment_config_defaults() {
        let config = ExperimentConfig::default();
        assert_eq!(config.retrieval_model, "gpt-5.3-codex-spark");
        assert_eq!(config.archive_model, "gpt-5.3-codex");
        assert_eq!(config.episodic_expiry_days, 30);
    }

    #[test]
    fn experiment_config_deserializes_partial_toml() {
        let toml_str = "episodic_expiry_days = 14";
        let config: ExperimentConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.episodic_expiry_days, 14);
        assert_eq!(config.retrieval_model, "gpt-5.3-codex-spark");
    }
}
