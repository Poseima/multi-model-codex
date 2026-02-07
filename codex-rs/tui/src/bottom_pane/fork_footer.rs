use ratatui::prelude::*;

use super::footer::CollaborationModeIndicator;
use super::footer::context_window_line;
use crate::status::format_tokens_compact;

/// Fork: enriched context window line showing "used / total (pct%)" format,
/// optionally prefixed with the collaboration mode indicator.
///
/// When a status line is active, the left side of the footer is occupied by
/// user-configured status items and the mode indicator has nowhere to render.
/// Passing `mode_indicator` here places it on the right side alongside the
/// context window info, e.g. "Plan mode · 12K / 256K (95%)".
///
/// This lives in a fork-only file so that the upstream `context_window_line()`
/// can keep its original 2-argument signature and `FooterProps` does not need
/// a `context_window_total` field (which would require ~22 test changes on
/// every upstream sync).
pub(crate) fn context_window_line_with_total(
    percent: Option<i64>,
    used_tokens: Option<i64>,
    total: Option<i64>,
    mode_indicator: Option<CollaborationModeIndicator>,
    show_cycle_hint: bool,
) -> Line<'static> {
    let context_line = context_line_spans(percent, used_tokens, total);

    match mode_indicator {
        Some(indicator) => {
            let mut spans = vec![indicator.styled_span(show_cycle_hint), " · ".dim()];
            spans.extend(context_line.spans);
            Line::from(spans)
        }
        None => context_line,
    }
}

fn context_line_spans(
    percent: Option<i64>,
    used_tokens: Option<i64>,
    total: Option<i64>,
) -> Line<'static> {
    // Best case: we have used tokens and total - show "12K / 256K (95%)"
    if let (Some(used), Some(total_tokens)) = (used_tokens, total) {
        let used_fmt = format_tokens_compact(used);
        let total_fmt = format_tokens_compact(total_tokens);
        let pct = percent.map(|p| p.clamp(0, 100)).unwrap_or(100);
        return Line::from(vec![
            Span::from(format!("{used_fmt} / {total_fmt} ({pct}%)")).dim(),
        ]);
    }

    // Fall back to upstream formatting
    context_window_line(percent, used_tokens)
}
