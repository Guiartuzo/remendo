//! The document pane: a read-only, syntax-highlighted view onto a [`Buffer`],
//! with its own cursor, scroll offset, and incremental search.
//!
//! DIVERGENCE from vybim's `pane.rs`, which this is the read-only slice of.
//! Roughly two thirds of that module is deliberately left behind — multi-caret
//! editing, insert/delete/backspace, undo/redo, word motion, and completion —
//! because v0 has **no in-application code editor** (proposal.md, design.md
//! §11). A fix that needs hand-writing is made in the reviewer's own editor
//! against the worktree, and the pane re-reads it.
//!
//! Two smaller divergences follow from that:
//!
//! * Motion methods drop vybim's `extend: bool` selection parameter. Selection
//!   survives only because *search* highlights its current match with it; with
//!   no copy and no edit, a user-driven selection has nothing to act on.
//! * The pane holds no `buffer_id`. vybim routed through a central buffer store
//!   so two panes could share one buffer; here the buffer is handed in per call.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::{layout::Position, style::Style};

use crate::buffer::Buffer;
use crate::search::Search;
use crate::syntax::Syntax;
use crate::theme::Theme;

/// Cursor position within the buffer. `target_col` remembers the column the
/// user "wants" so vertical movement across short lines doesn't lose it.
#[derive(Debug, Default, Clone, Copy)]
pub struct Cursor {
    pub line: usize,
    pub col: usize,
    pub target_col: usize,
}

/// A read-only viewport onto a buffer.
///
/// ```
/// # use remendo::{buffer::Buffer, pane::DocumentPane};
/// let buffer = Buffer::from_text("one\ntwo\nthree\n");
/// let mut pane = DocumentPane::new();
/// pane.goto_line(&buffer, 2);
/// assert_eq!(pane.cursor_line_col(), (1, 0));
/// ```
#[derive(Debug, Default)]
pub struct DocumentPane {
    pub cursor: Cursor,
    /// Selection anchor `(line, col)`. The selection spans from here to the
    /// cursor; `None` means none. Set by search to highlight its current match.
    anchor: Option<(usize, usize)>,
    scroll_row: usize,
    scroll_col: usize,
    /// Height of the content region at the last render, used by page movement
    /// (which needs the viewport size, only known at render time).
    last_height: usize,
    search: Option<Search>,
}

impl DocumentPane {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset all view state — used when the pane switches to another document.
    /// The document is polymorphic: a source file, the commit message, or a
    /// synthetic change overview (specs/comment-triage).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    // --- queries -----------------------------------------------------------

    pub fn has_selection(&self) -> bool {
        self.anchor.is_some()
    }

    /// The cursor's current `(line, col)`.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        (self.cursor.line, self.cursor.col)
    }

    /// Current vertical scroll offset, in lines.
    pub fn scroll_row(&self) -> usize {
        self.scroll_row
    }

    fn last_line(&self, buffer: &Buffer) -> usize {
        buffer.line_count() - 1
    }

    /// The selection as an ordered `(start, end)` pair, if any.
    fn ordered_selection(&self) -> Option<((usize, usize), (usize, usize))> {
        let anchor = self.anchor?;
        let cursor = (self.cursor.line, self.cursor.col);
        Some(if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        })
    }

    // --- motion ------------------------------------------------------------

    pub fn move_left(&mut self, buffer: &Buffer) {
        self.anchor = None;
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        } else if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.col = buffer.line_len_chars(self.cursor.line);
        }
        self.cursor.target_col = self.cursor.col;
    }

    pub fn move_right(&mut self, buffer: &Buffer) {
        self.anchor = None;
        if self.cursor.col < buffer.line_len_chars(self.cursor.line) {
            self.cursor.col += 1;
        } else if self.cursor.line < self.last_line(buffer) {
            self.cursor.line += 1;
            self.cursor.col = 0;
        }
        self.cursor.target_col = self.cursor.col;
    }

    pub fn move_up(&mut self, buffer: &Buffer) {
        self.anchor = None;
        if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.clamp_col_to_target(buffer);
        }
    }

    pub fn move_down(&mut self, buffer: &Buffer) {
        self.anchor = None;
        if self.cursor.line < self.last_line(buffer) {
            self.cursor.line += 1;
            self.clamp_col_to_target(buffer);
        }
    }

    /// Move to column zero of the current line (Home).
    pub fn move_line_start(&mut self) {
        self.anchor = None;
        self.cursor.col = 0;
        self.cursor.target_col = 0;
    }

    /// Move to the end of the current line (End).
    pub fn move_line_end(&mut self, buffer: &Buffer) {
        self.anchor = None;
        self.cursor.col = buffer.line_len_chars(self.cursor.line);
        self.cursor.target_col = self.cursor.col;
    }

    /// Move up by roughly one viewport height, keeping the target column.
    pub fn page_up(&mut self, buffer: &Buffer) {
        self.anchor = None;
        self.cursor.line = self.cursor.line.saturating_sub(self.page_rows());
        self.clamp_col_to_target(buffer);
    }

    /// Move down by roughly one viewport height, keeping the target column.
    pub fn page_down(&mut self, buffer: &Buffer) {
        self.anchor = None;
        self.cursor.line = (self.cursor.line + self.page_rows()).min(self.last_line(buffer));
        self.clamp_col_to_target(buffer);
    }

    /// Rows to jump for a page movement: the last-rendered content height, or a
    /// sane default before the first render.
    fn page_rows(&self) -> usize {
        self.last_height.max(1)
    }

    /// Put the cursor at its remembered target column, clamped to the current
    /// line — the "wanted column" behaviour that survives short lines.
    fn clamp_col_to_target(&mut self, buffer: &Buffer) {
        self.cursor.col = self
            .cursor
            .target_col
            .min(buffer.line_len_chars(self.cursor.line));
    }

    /// Move the cursor to the start of 1-based line `n`, clamped to the last
    /// line, clearing any selection.
    pub fn goto_line(&mut self, buffer: &Buffer, n: usize) {
        let line = n.saturating_sub(1).min(self.last_line(buffer));
        self.cursor.line = line;
        self.cursor.col = 0;
        self.cursor.target_col = 0;
        self.anchor = None;
    }

    /// Move the cursor to `(line, col)`, clamped into `buffer`, clearing any
    /// selection. The next render reveals it via the same scroll-into-view path
    /// go-to-line and search use.
    pub fn set_cursor(&mut self, buffer: &Buffer, line: usize, col: usize) {
        let line = line.min(self.last_line(buffer));
        let col = col.min(buffer.line_len_chars(line));
        self.cursor.line = line;
        self.cursor.col = col;
        self.cursor.target_col = col;
        self.anchor = None;
    }

    // --- search ------------------------------------------------------------

    /// Begin a search: remember the current cursor so a cancel can restore it.
    pub fn search_begin(&mut self) {
        self.search = Some(Search::begin(self.cursor));
    }

    /// Recompute matches for `query` and jump to the nearest one at or after the
    /// search origin (wrapping to the first). An empty query or no matches puts
    /// the cursor back at the origin with no selection.
    pub fn search_update(&mut self, buffer: &Buffer, query: &str) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        search.update(buffer, query);
        self.show_current_match();
    }

    /// Move to the next match, wrapping past the last to the first.
    pub fn search_next(&mut self) {
        if let Some(search) = self.search.as_mut() {
            search.next();
        }
        self.show_current_match();
    }

    /// Move to the previous match, wrapping past the first to the last.
    pub fn search_prev(&mut self) {
        if let Some(search) = self.search.as_mut() {
            search.prev();
        }
        self.show_current_match();
    }

    /// Confirm the search: drop the search state but leave the match selected.
    pub fn search_commit(&mut self) {
        self.search = None;
    }

    /// Abandon the search: restore the origin cursor and clear the selection.
    pub fn search_cancel(&mut self) {
        if let Some(search) = self.search.take() {
            let origin = search.origin();
            self.cursor = origin;
            self.cursor.target_col = origin.col;
            self.anchor = None;
        }
    }

    /// Move the cursor onto the search's current match and span it with the
    /// selection so the highlight draws it. With no match, fall back to the
    /// origin — where the cursor sat before the search began.
    fn show_current_match(&mut self) {
        let Some(search) = self.search.as_ref() else {
            return;
        };
        match search.current_match() {
            Some(((line, col), len)) => {
                self.anchor = Some((line, col));
                self.cursor.line = line;
                self.cursor.col = col + len;
            }
            None => {
                self.cursor = search.origin();
                self.anchor = None;
            }
        }
        self.cursor.target_col = self.cursor.col;
    }

    // --- rendering ---------------------------------------------------------

    /// Render the pane into `area`: the visible region plus a one-row status bar.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &RenderCtx) {
        let [content, status] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        self.render_content(frame, content, ctx);
        self.render_status(frame, status, ctx);
    }

    fn render_content(&mut self, frame: &mut Frame, area: Rect, ctx: &RenderCtx) {
        let &RenderCtx {
            buffer,
            focused,
            theme,
            ..
        } = ctx;
        // Reserve a left gutter for line numbers; text fills the rest.
        let num_w = gutter_num_width(buffer.line_count());
        let [gutter, content] =
            Layout::horizontal([Constraint::Length((num_w + 1) as u16), Constraint::Min(0)])
                .areas(area);

        let height = content.height as usize;
        self.last_height = height;
        self.scroll_row = scroll_to_show(self.scroll_row, self.cursor.line, height);
        self.scroll_col = scroll_to_show(self.scroll_col, self.cursor.col, content.width as usize);

        self.render_text(frame, gutter, content, ctx, num_w);
        self.paint_current_line(frame, area, content, theme, focused);
        if let Some((start, end)) = self.ordered_selection() {
            self.paint_selection(frame, content, buffer, theme, start, end);
        }
        if focused {
            let cx = content.x + (self.cursor.col.saturating_sub(self.scroll_col)) as u16;
            let cy = content.y + (self.cursor.line.saturating_sub(self.scroll_row)) as u16;
            frame.set_cursor_position(Position::new(cx, cy));
        }
    }

    /// Draw the line-number gutter and the highlighted text of the visible rows.
    fn render_text(
        &self,
        frame: &mut Frame,
        gutter: Rect,
        content: Rect,
        ctx: &RenderCtx,
        num_w: usize,
    ) {
        let height = content.height as usize;
        let mut numbers: Vec<Line> = Vec::with_capacity(height);
        let mut lines: Vec<Line> = Vec::with_capacity(height);
        for row in 0..height {
            let line_idx = self.scroll_row + row;
            if line_idx >= ctx.buffer.line_count() {
                break;
            }
            let is_current = line_idx == self.cursor.line;
            let num_style = if is_current && ctx.focused {
                Style::new().fg(ctx.theme.text)
            } else {
                Style::new().fg(ctx.theme.text_muted)
            };
            numbers.push(Line::styled(
                format!("{:>width$} ", line_idx + 1, width = num_w),
                num_style,
            ));

            let text = ctx.buffer.line_text(line_idx);
            let visible: String = text.chars().skip(self.scroll_col).collect();
            lines.push(highlight_line(&visible, ctx.syntax));
        }
        frame.render_widget(Paragraph::new(Text::from(numbers)), gutter);
        frame.render_widget(Paragraph::new(Text::from(lines)), content);
    }

    /// Subtle current-line tint across the whole row (gutter + text), so empty
    /// cells are covered too.
    fn paint_current_line(
        &self,
        frame: &mut Frame,
        area: Rect,
        content: Rect,
        theme: &Theme,
        focused: bool,
    ) {
        if !focused || self.cursor.line < self.scroll_row {
            return;
        }
        let row = (self.cursor.line - self.scroll_row) as u16;
        if row < content.height {
            let row_rect = Rect::new(area.x, content.y + row, area.width, 1);
            frame
                .buffer_mut()
                .set_style(row_rect, Style::new().bg(theme.cursor_line));
        }
    }

    /// Paint the selection over the text, after the current-line tint so the
    /// selected span wins where they overlap.
    fn paint_selection(
        &self,
        frame: &mut Frame,
        content: Rect,
        buffer: &Buffer,
        theme: &Theme,
        start: (usize, usize),
        end: (usize, usize),
    ) {
        for row in 0..content.height as usize {
            let line_idx = self.scroll_row + row;
            if line_idx < start.0 || line_idx > end.0 {
                continue;
            }
            let start_col = if line_idx == start.0 { start.1 } else { 0 };
            // Lines fully inside the selection extend one cell past the end of
            // line, so the selected newline reads as highlighted.
            let end_col = if line_idx == end.0 {
                end.1
            } else {
                buffer.line_len_chars(line_idx) + 1
            };
            let vis_start = start_col.max(self.scroll_col);
            if end_col <= vis_start {
                continue;
            }
            let sx = content.x + (vis_start - self.scroll_col) as u16;
            let avail = content.width.saturating_sub(sx - content.x);
            let w = ((end_col - vis_start) as u16).min(avail);
            if w == 0 {
                continue;
            }
            let rect = Rect::new(sx, content.y + row as u16, w, 1);
            frame
                .buffer_mut()
                .set_style(rect, Style::new().bg(theme.selection));
        }
    }

    fn render_status(&self, frame: &mut Frame, area: Rect, ctx: &RenderCtx) {
        let name = ctx
            .buffer
            .path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "[No Name]".to_string());
        // No dirty marker: the pane is read-only, so it can never be the source
        // of an unsaved change.
        let text = format!(
            " {name}    Ln {}, Col {} ",
            self.cursor.line + 1,
            self.cursor.col + 1
        );
        frame.render_widget(
            Paragraph::new(text).style(ctx.theme.list_row(ctx.focused)),
            area,
        );
    }
}

/// Per-frame inputs the pane needs to draw itself. `syntax` is `Some` when the
/// document's language has a bundled grammar.
#[derive(Clone, Copy)]
pub struct RenderCtx<'a> {
    pub buffer: &'a Buffer,
    pub syntax: Option<&'a Syntax>,
    pub focused: bool,
    pub theme: &'a Theme,
}

/// Build a styled line from `text`, applying syntax highlight spans (byte
/// ranges) where a grammar is available, and leaving gaps in the default style.
/// The returned line owns its text, so it does not borrow `text`.
fn highlight_line(text: &str, syntax: Option<&Syntax>) -> Line<'static> {
    let Some(syntax) = syntax else {
        return Line::raw(text.to_string());
    };
    let spans = syntax.highlight_line(text);
    if spans.is_empty() {
        return Line::raw(text.to_string());
    }

    let mut out: Vec<Span> = Vec::new();
    let mut cursor = 0;
    for (start, end, style) in spans {
        // Spans are ordered and non-overlapping; fill any gap before this run.
        if start > cursor {
            out.push(Span::raw(text[cursor..start].to_string()));
        }
        out.push(Span::styled(text[start..end].to_string(), style));
        cursor = end;
    }
    if cursor < text.len() {
        out.push(Span::raw(text[cursor..].to_string()));
    }
    Line::from(out)
}

/// Width (in digits) reserved for line numbers, given the line count. A floor
/// of 3 keeps the gutter from jittering on small files.
fn gutter_num_width(line_count: usize) -> usize {
    line_count.to_string().len().max(3)
}

/// Given the current scroll offset, the cursor index, and the viewport size on
/// one axis, return the scroll offset that keeps the cursor visible while
/// moving as little as possible.
fn scroll_to_show(scroll: usize, cursor: usize, viewport: usize) -> usize {
    if viewport == 0 {
        return scroll;
    }
    if cursor < scroll {
        cursor
    } else if cursor >= scroll + viewport {
        cursor + 1 - viewport
    } else {
        scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pane over a fresh in-memory buffer.
    fn setup(text: &str) -> (DocumentPane, Buffer) {
        (DocumentPane::new(), Buffer::from_text(text))
    }

    // --- viewport (ported unchanged from vybim) ----------------------------

    #[test]
    fn cursor_inside_viewport_does_not_scroll() {
        assert_eq!(scroll_to_show(10, 12, 20), 10);
    }

    #[test]
    fn cursor_above_viewport_scrolls_up_to_cursor() {
        assert_eq!(scroll_to_show(10, 4, 20), 4);
    }

    #[test]
    fn cursor_below_viewport_scrolls_just_enough() {
        assert_eq!(scroll_to_show(10, 35, 20), 16);
    }

    #[test]
    fn zero_viewport_is_a_noop() {
        assert_eq!(scroll_to_show(7, 100, 0), 7);
    }

    #[test]
    fn gutter_has_a_minimum_width() {
        assert_eq!(gutter_num_width(5), 3);
        assert_eq!(gutter_num_width(12345), 5);
    }

    // --- motion ------------------------------------------------------------

    #[test]
    fn vertical_move_preserves_target_column() {
        let (mut p, b) = setup("abcde\nxy\nlongerline");
        for _ in 0..4 {
            p.move_right(&b);
        }
        assert_eq!(p.cursor_line_col(), (0, 4));
        p.move_down(&b); // "xy" len 2 -> clamp to 2, target stays 4
        assert_eq!(p.cursor_line_col(), (1, 2));
        assert_eq!(p.cursor.target_col, 4);
        p.move_down(&b); // "longerline" -> col back to 4
        assert_eq!(p.cursor_line_col(), (2, 4));
    }

    #[test]
    fn move_left_wraps_to_previous_line_end() {
        let (mut p, b) = setup("ab\ncd");
        p.move_down(&b);
        assert_eq!(p.cursor_line_col(), (1, 0));
        p.move_left(&b);
        assert_eq!(p.cursor_line_col(), (0, 2));
    }

    #[test]
    fn motion_clamps_at_both_ends() {
        let (mut p, b) = setup("ab\ncd");
        p.move_up(&b); // already on the first line
        assert_eq!(p.cursor_line_col(), (0, 0));
        p.move_left(&b); // already at the start
        assert_eq!(p.cursor_line_col(), (0, 0));
        p.page_down(&b);
        p.move_down(&b); // already on the last line
        assert_eq!(p.cursor.line, 1);
    }

    // --- goto_line ---------------------------------------------------------

    #[test]
    fn goto_line_is_one_based_and_clamps() {
        let (mut p, b) = setup("one\ntwo\nthree\n");
        p.goto_line(&b, 2);
        assert_eq!(p.cursor_line_col(), (1, 0));
        p.goto_line(&b, 9999);
        assert_eq!(p.cursor.line, b.line_count() - 1);
        p.goto_line(&b, 0); // saturates rather than underflowing
        assert_eq!(p.cursor.line, 0);
    }

    // --- search ------------------------------------------------------------

    #[test]
    fn search_selects_the_first_match_at_or_after_the_origin() {
        let (mut p, b) = setup("alpha\nbeta\nalpha\n");
        p.goto_line(&b, 2);
        p.search_begin();
        p.search_update(&b, "alpha");
        // Origin is line 1, so the match on line 2 is the nearest at-or-after.
        assert_eq!(p.cursor.line, 2);
        assert!(p.has_selection());
    }

    #[test]
    fn search_is_case_insensitive_and_wraps() {
        let (mut p, b) = setup("Alpha\nbeta\nALPHA\n");
        p.search_begin();
        p.search_update(&b, "alpha");
        assert_eq!(p.cursor.line, 0);
        p.search_next();
        assert_eq!(p.cursor.line, 2);
        p.search_next(); // wraps to the first
        assert_eq!(p.cursor.line, 0);
        p.search_prev(); // wraps back to the last
        assert_eq!(p.cursor.line, 2);
    }

    #[test]
    fn search_cancel_restores_the_origin_cursor() {
        let (mut p, b) = setup("alpha\nbeta\nalpha\n");
        p.goto_line(&b, 2);
        let origin = p.cursor_line_col();
        p.search_begin();
        p.search_update(&b, "alpha");
        assert_ne!(p.cursor_line_col(), origin);
        p.search_cancel();
        assert_eq!(p.cursor_line_col(), origin);
        assert!(!p.has_selection());
    }

    #[test]
    fn search_commit_keeps_the_match_selected() {
        let (mut p, b) = setup("alpha\nbeta\n");
        p.search_begin();
        p.search_update(&b, "beta");
        p.search_commit();
        assert_eq!(p.cursor.line, 1);
        assert!(p.has_selection());
    }

    #[test]
    fn a_query_with_no_match_returns_to_the_origin() {
        let (mut p, b) = setup("alpha\nbeta\n");
        p.goto_line(&b, 2);
        let origin = p.cursor_line_col();
        p.search_begin();
        p.search_update(&b, "zzz");
        assert_eq!(p.cursor_line_col(), origin);
        assert!(!p.has_selection());
    }

    #[test]
    fn search_update_without_begin_is_inert() {
        let (mut p, b) = setup("alpha\n");
        p.search_update(&b, "alpha");
        assert_eq!(p.cursor_line_col(), (0, 0));
        assert!(!p.has_selection());
    }
}
