//! Fork: TUI rendering for memory experiment events.
//!
//! Follows the same `PlainHistoryCell` pattern as `collab.rs` — renders
//! `MemoryRetrieveBegin` / `MemoryRetrieveEnd` as styled history cells.

use crate::history_cell::PlainHistoryCell;
use crate::render::line_utils::prefix_lines;
use crate::text_formatting::truncate_text;
use codex_core::protocol::MemoryRetrieveBeginEvent;
use codex_core::protocol::MemoryRetrieveEndEvent;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

const QUERY_PREVIEW_GRAPHEMES: usize = 160;

pub(crate) fn retrieve_begin(ev: MemoryRetrieveBeginEvent) -> PlainHistoryCell {
    let mut details = Vec::new();
    if let Some(line) = query_line(&ev.query) {
        details.push(line);
    }
    memory_event("Retrieving memories", details)
}

pub(crate) fn retrieve_end(ev: MemoryRetrieveEndEvent) -> PlainHistoryCell {
    let status = if ev.success {
        Span::from("completed").green()
    } else {
        Span::from("failed").red()
    };
    let mut details = vec![detail_line("status", status)];
    if let Some(line) = query_line(&ev.query) {
        details.push(line);
    }
    memory_event("Memory retrieved", details)
}

fn memory_event(title: &str, details: Vec<Line<'static>>) -> PlainHistoryCell {
    let mut lines: Vec<Line<'static>> =
        vec![vec![Span::from("• ").dim(), Span::from(title.to_string()).bold()].into()];
    if !details.is_empty() {
        lines.extend(prefix_lines(details, "  └ ".dim(), "    ".into()));
    }
    PlainHistoryCell::new(lines)
}

fn detail_line(label: &str, value: impl Into<Span<'static>>) -> Line<'static> {
    vec![Span::from(format!("{label}: ")).dim(), value.into()].into()
}

fn query_line(query: &str) -> Option<Line<'static>> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(detail_line(
            "query",
            Span::from(truncate_text(trimmed, QUERY_PREVIEW_GRAPHEMES)).dim(),
        ))
    }
}
