//! Drawing the triage screen.
//!
//! The one module `cargo test` cannot meaningfully pin — every decision lives
//! in `mod.rs` and `keys.rs`, so what is here is layout and colour.
//!
//! ```text
//!  ┌──────────┬────────────────────────────┬──────────────────┐
//!  │ review   │ document (read-only,       │ verdict          │
//!  │ map      │  anchor highlighted)       │ justification    │
//!  │          ├────────────────────────────┤ depends_on       │
//!  │ a.rs 2/3 │ the thread, in order       │  (shared facts)  │
//!  │*/COMMIT… │ with authors               │                  │
//!  └──────────┴────────────────────────────┴──────────────────┘
//!  │ status / minibuffer / gate                               │
//!  └──────────────────────────────────────────────────────────┘
//! ```

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Borders, Paragraph, Wrap};

use super::{App, Mode};
use crate::syntax::Syntax;
use crate::theme::Theme;
use crate::triage::{Decision, Document, TriageItem};

/// Width of the review-map column.
const TREE_WIDTH: u16 = 26;

/// Width of the verdict column.
const VERDICT_WIDTH: u16 = 44;

/// Rows given to the thread beneath the document.
const THREAD_HEIGHT: u16 = 8;

/// Draw the whole screen.
pub fn draw(frame: &mut Frame, app: &App, document: &Document, theme: &Theme) {
    let [body, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());

    let tree_width = if app.tree_visible { TREE_WIDTH } else { 0 };
    let [tree, middle, verdict] = Layout::horizontal([
        Constraint::Length(tree_width),
        Constraint::Min(20),
        Constraint::Length(VERDICT_WIDTH),
    ])
    .areas(body);

    if app.tree_visible {
        draw_tree(frame, app, tree, theme);
    }
    draw_middle(frame, app, document, middle, theme);
    draw_verdict(frame, app, verdict, theme);
    draw_status(frame, app, status, theme);
}

/// The review map: where attention is still needed.
fn draw_tree(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let block = theme.block(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let entries = super::tree::entries(&app.triage);
    let selected = super::tree::selected(&app.triage, &entries);
    let lines: Vec<Line> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let style = if Some(index) == selected {
                Style::new().bg(theme.focus_bg).fg(theme.focus_fg)
            } else if entry.is_complete() {
                Style::new().fg(theme.text_muted)
            } else {
                Style::new().fg(theme.text)
            };
            Line::from(vec![
                Span::styled(entry.kind_marker().to_string(), style),
                Span::styled(truncate(&entry.path, inner.width as usize - 8), style),
                Span::styled(format!(" {}", entry.progress()), style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The document, with the thread below it.
fn draw_middle(frame: &mut Frame, app: &App, document: &Document, area: Rect, theme: &Theme) {
    let [doc_area, thread_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(THREAD_HEIGHT)]).areas(area);
    draw_document(frame, app, document, doc_area, theme);
    draw_thread(frame, app, thread_area, theme);
}

/// The anchored document, read-only, with the commented line highlighted in
/// place. Only real source files are syntax highlighted; a commit message and
/// the change overview are prose.
fn draw_document(frame: &mut Frame, app: &App, document: &Document, area: Rect, theme: &Theme) {
    let block = theme.block(Borders::ALL).title(document.title.clone());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let syntax = document
        .highlighted
        .then(|| Syntax::for_path(std::path::Path::new(&document.title)))
        .flatten();

    // Keep the anchored line on screen without letting the user's scrolling be
    // overridden: the anchor sets the floor, scrolling moves from there.
    let anchor = document.anchor_index().unwrap_or(0);
    let height = inner.height as usize;
    let base = anchor.saturating_sub(height / 3);
    let top = base + app.doc_scroll;

    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for offset in 0..height {
        let index = top + offset;
        let Some(text) = document.lines.get(index) else {
            break;
        };
        let is_anchor = document.anchor_index() == Some(index);
        lines.push(document_line(
            text,
            index,
            is_anchor,
            syntax.as_ref(),
            theme,
            width,
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// One rendered document line: number gutter, then highlighted or plain text.
fn document_line(
    text: &str,
    index: usize,
    is_anchor: bool,
    syntax: Option<&Syntax>,
    theme: &Theme,
    width: usize,
) -> Line<'static> {
    let number = Span::styled(
        format!("{:>4} ", index + 1),
        Style::new().fg(theme.text_muted),
    );
    let body = truncate(text, width.saturating_sub(5));

    if is_anchor {
        // The commented line is highlighted in place rather than pointed at
        // from elsewhere, which is what makes the pane a review surface.
        return Line::from(vec![
            number,
            Span::styled(body, Style::new().bg(theme.selection).fg(theme.text)),
        ]);
    }
    match syntax {
        Some(syntax) => {
            let mut spans = vec![number];
            spans.extend(highlight(&body, syntax));
            Line::from(spans)
        }
        None => Line::from(vec![number, Span::raw(body)]),
    }
}

/// Syntax-highlight one line into owned spans.
fn highlight(text: &str, syntax: &Syntax) -> Vec<Span<'static>> {
    let ranges = syntax.highlight_line(text);
    if ranges.is_empty() {
        return vec![Span::raw(text.to_string())];
    }
    let mut spans = Vec::new();
    let mut cursor = 0;
    for (start, end, style) in ranges {
        if start > cursor {
            spans.push(Span::raw(text[cursor..start].to_string()));
        }
        spans.push(Span::styled(text[start..end].to_string(), style));
        cursor = end;
    }
    if cursor < text.len() {
        spans.push(Span::raw(text[cursor..].to_string()));
    }
    spans
}

/// The whole exchange, in order, with authors.
///
/// The whole thread rather than its opening comment: an open thread's live ask
/// is frequently a later comment, and showing only the first asks the human to
/// decide a question the thread already moved past (design.md §13).
fn draw_thread(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let block = theme.block(Borders::ALL).title("thread");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(item) = app.triage.current() else {
        return;
    };
    let mut lines: Vec<Line> = Vec::new();
    for comment in &item.thread.comments {
        lines.push(Line::from(Span::styled(
            format!("{}:", comment.author_name()),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::raw(comment.message.clone()));
    }
    if let Some(edited) = &item.edited_prose {
        lines.push(Line::from(Span::styled(
            format!("(edited) {edited}"),
            Style::new().fg(theme.diff_add_fg),
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// The verdict, its justification, and the facts it rests on.
fn draw_verdict(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let block = theme.block(Borders::ALL).title("verdict");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(item) = app.triage.current() else {
        return;
    };
    let mut lines = verdict_header(item, theme);
    lines.extend(dependency_lines(item, theme));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// The adjudication and its justification, or an explicit unadjudicated state.
fn verdict_header(item: &TriageItem, theme: &Theme) -> Vec<Line<'static>> {
    let Some(verdict) = &item.verdict else {
        // A missing verdict must look missing, never like an empty one.
        return vec![Line::from(Span::styled(
            "UNADJUDICATED — the verdict pass produced nothing for this thread".to_string(),
            Style::new()
                .fg(theme.diff_del_fg)
                .add_modifier(Modifier::BOLD),
        ))];
    };
    let colour = match verdict.verdict {
        crate::verdict::Adjudication::Agree => theme.diff_add_fg,
        crate::verdict::Adjudication::Disagree => theme.diff_del_fg,
        crate::verdict::Adjudication::Unsure => theme.accent,
    };
    vec![
        Line::from(vec![
            Span::styled(
                verdict.verdict.label().to_uppercase(),
                Style::new().fg(colour).add_modifier(Modifier::BOLD),
            ),
            Span::styled(decision_suffix(item), Style::new().fg(theme.text_muted)),
        ]),
        Line::raw(verdict.justification.clone()),
        Line::raw(String::new()),
    ]
}

/// What the human decided, when they have.
fn decision_suffix(item: &TriageItem) -> String {
    match item.decision {
        Some(Decision::Accept) => "   → accepted".into(),
        Some(Decision::Reject) => "   → rejected".into(),
        Some(Decision::Defer) => "   → deferred".into(),
        Some(Decision::FixedByHand) => "   → fixed by hand".into(),
        None => String::new(),
    }
}

/// The out-of-code facts, with no way to run their verification.
fn dependency_lines(item: &TriageItem, theme: &Theme) -> Vec<Line<'static>> {
    let Some(verdict) = &item.verdict else {
        return Vec::new();
    };
    if verdict.is_self_contained() {
        return vec![Line::from(Span::styled(
            "rests on the code alone".to_string(),
            Style::new().fg(theme.text_muted),
        ))];
    }

    let mut lines = vec![Line::from(Span::styled(
        "RESTS ON — settle these yourself".to_string(),
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
    ))];
    for dependency in verdict.dependencies() {
        lines.push(Line::raw(format!("• {}", dependency.fact)));
        // `verify` is prose. It is rendered as text with no action attached —
        // Remendo never executes it (design.md §13).
        lines.push(Line::from(Span::styled(
            format!("  check: {}", dependency.verify),
            Style::new().fg(theme.text_muted),
        )));
        if let Some(flips_to) = dependency.flips_to {
            lines.push(Line::from(Span::styled(
                format!("  otherwise → {}", flips_to.label()),
                Style::new().fg(theme.text_muted),
            )));
        }
    }
    lines
}

/// The bottom row: the prompt, the gate, or the status.
fn draw_status(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    if let Some(mini) = &app.minibuffer {
        mini.render(frame, area, theme);
        return;
    }
    if let (Mode::Gate, Some(report)) = (app.mode, app.gate) {
        let text = if report.is_clean() {
            " End triage and begin applying? (y/n) ".to_string()
        } else {
            format!(
                " End triage? {} undecided, {} deferred, {} awaiting a reply \
                 — undecided and deferred will be left untouched. (y/n) ",
                report.undecided, report.deferred, report.unreplied
            )
        };
        let style = Style::new().bg(theme.prompt_bg).fg(theme.text);
        frame.render_widget(Paragraph::new(text).style(style), area);
        return;
    }

    let text = match &app.status {
        Some(status) => format!(" {status} "),
        None => format!(
            " {}/{}  {} undecided   a accept · r reject · d defer · h hand · e edit · \
             tab next-undecided · t tree · w worktree · F finish · q quit ",
            app.triage.cursor() + 1,
            app.triage.len(),
            app.triage.undecided()
        ),
    };
    frame.render_widget(
        Paragraph::new(truncate(&text, area.width as usize)).style(theme.list_row(true)),
        area,
    );
}

/// Cut `text` to `width` columns, counting characters rather than bytes so a
/// multi-byte line cannot be split mid-character.
fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars()
        .take(width.saturating_sub(1))
        .collect::<String>()
        + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_counts_characters_not_bytes() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello", 4), "hel…");
        // Multi-byte input must not be cut mid-character.
        assert_eq!(truncate("ãéîõü", 3), "ãé…");
    }

    #[test]
    fn truncation_survives_a_zero_width() {
        assert_eq!(truncate("hello", 0), "…");
    }
}
