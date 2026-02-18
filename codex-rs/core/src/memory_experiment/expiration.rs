//! Episodic memory expiration.
//!
//! Scans the `episodic/` directory and removes files whose `expires` timestamp
//! has passed.

use crate::memory_experiment::types::parse_frontmatter;
use chrono::Utc;
use std::path::Path;
use tracing::info;
use tracing::warn;

/// Remove episodic memory files past their expiry date.
///
/// Returns the number of files pruned.
pub(crate) async fn prune_expired(project_root: &Path) -> std::io::Result<usize> {
    let episodic_dir = project_root.join("episodic");
    let mut entries = match tokio::fs::read_dir(&episodic_dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };

    let now = Utc::now();
    let mut pruned = 0;

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
                warn!("failed to read episodic file {}: {e}", path.display());
                continue;
            }
        };

        let Some((meta, _)) = parse_frontmatter(&content) else {
            continue;
        };

        let Some(expires_str) = meta.expires.as_deref() else {
            continue;
        };

        let Ok(expires) = chrono::DateTime::parse_from_rfc3339(expires_str) else {
            // Also try date-only format.
            if let Ok(date) = chrono::NaiveDate::parse_from_str(expires_str, "%Y-%m-%d") {
                let expires_dt = date.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc());
                if let Some(expires_dt) = expires_dt
                    && expires_dt < now
                {
                    info!("pruning expired episodic memory: {filename}");
                    if let Err(e) = tokio::fs::remove_file(&path).await {
                        warn!("failed to remove expired memory {}: {e}", path.display());
                    } else {
                        pruned += 1;
                    }
                }
                continue;
            }
            warn!("unparseable expires timestamp in {filename}: {expires_str}");
            continue;
        };

        if expires.with_timezone(&Utc) < now {
            info!("pruning expired episodic memory: {filename}");
            if let Err(e) = tokio::fs::remove_file(&path).await {
                warn!("failed to remove expired memory {}: {e}", path.display());
            } else {
                pruned += 1;
            }
        }
    }

    Ok(pruned)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn prune_expired_removes_old_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let episodic = root.join("episodic");
        tokio::fs::create_dir_all(&episodic).await.unwrap();

        // Expired file.
        let expired_content = "\
---
type: episodic
keywords: [old]
summary: Old event
created: \"2025-01-01T00:00:00Z\"
last_updated: \"2025-01-01T00:00:00Z\"
expires: \"2025-06-01T00:00:00Z\"
---

Old event content.
";
        tokio::fs::write(episodic.join("old-event.md"), expired_content)
            .await
            .unwrap();

        // Not-yet-expired file.
        let fresh_content = "\
---
type: episodic
keywords: [fresh]
summary: Fresh event
created: \"2026-02-15T00:00:00Z\"
last_updated: \"2026-02-15T00:00:00Z\"
expires: \"2027-02-15T00:00:00Z\"
---

Fresh event content.
";
        tokio::fs::write(episodic.join("fresh-event.md"), fresh_content)
            .await
            .unwrap();

        let pruned = prune_expired(root).await.unwrap();
        assert_eq!(pruned, 1);
        assert!(!episodic.join("old-event.md").exists());
        assert!(episodic.join("fresh-event.md").exists());
    }

    #[tokio::test]
    async fn prune_expired_handles_missing_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let pruned = prune_expired(tmp.path()).await.unwrap();
        assert_eq!(pruned, 0);
    }

    #[tokio::test]
    async fn prune_expired_handles_date_only_format() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let episodic = root.join("episodic");
        tokio::fs::create_dir_all(&episodic).await.unwrap();

        // Expired with date-only format.
        let content = "\
---
type: episodic
keywords: [old]
summary: Old event
created: \"2025-01-01T00:00:00Z\"
last_updated: \"2025-01-01T00:00:00Z\"
expires: \"2025-06-01\"
---

Old event.
";
        tokio::fs::write(episodic.join("date-only.md"), content)
            .await
            .unwrap();

        let pruned = prune_expired(root).await.unwrap();
        assert_eq!(pruned, 1);
        assert!(!episodic.join("date-only.md").exists());
    }

    #[tokio::test]
    async fn prune_expired_preserves_files_without_expires_field() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let episodic = root.join("episodic");
        tokio::fs::create_dir_all(&episodic).await.unwrap();

        // No expires field — should not be pruned.
        let content = "\
---
type: episodic
keywords: [no-expiry]
summary: Event without expiry
created: \"2025-01-01T00:00:00Z\"
last_updated: \"2025-01-01T00:00:00Z\"
---

Permanent episodic event.
";
        tokio::fs::write(episodic.join("no-expiry.md"), content)
            .await
            .unwrap();

        let pruned = prune_expired(root).await.unwrap();
        assert_eq!(pruned, 0);
        assert!(episodic.join("no-expiry.md").exists());
    }

    #[tokio::test]
    async fn prune_expired_skips_non_markdown_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let episodic = root.join("episodic");
        tokio::fs::create_dir_all(&episodic).await.unwrap();

        tokio::fs::write(episodic.join("notes.txt"), "not a memory file")
            .await
            .unwrap();

        let pruned = prune_expired(root).await.unwrap();
        assert_eq!(pruned, 0);
        assert!(episodic.join("notes.txt").exists());
    }
}
