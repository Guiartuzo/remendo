//! Side-by-side diff of one before/after text pair.
//!
//! The diff model (`DiffRow`, `build_rows`, hunks) is computed locally with the
//! `similar` crate rather than parsed out of `git diff`'s unified output —
//! `similar` yields exactly the per-line op stream that maps onto two aligned
//! columns. Keeping `similar` behind this module is `config.yaml`'s wrap-the-
//! third-party-lib rule.
//!
//! DIVERGENCE from vybim, which this is copied from. There the view was a modal
//! over the *working tree vs `HEAD`*: it owned a changed-files list, called into
//! `crate::git` for both sides, and had two-way inner focus between list and
//! diff. Remendo needs none of that. Its confirm-diff is a single file with an
//! explicitly supplied pair of texts, because the left side is the **pre-turn
//! snapshot**, not the patchset baseline — diffing against the baseline would
//! show earlier confirmed edits and hide what the reviewer is actually
//! approving (design.md §7, specs/fix-application). A git seam here would be
//! the wrong seam.
//!
//! The confirm/reject gate that drives this view lands with tasks.md 6.3.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Borders, Clear, Paragraph};
use similar::{ChangeTag, TextDiff};

use crate::theme::Theme;

/// Rows moved per page-up / page-down.
const PAGE_ROWS: usize = 10;

/// One aligned row of a side-by-side diff. `Equal` lines sit across from each
/// other; `Delete` is left-only (removed), `Insert` is right-only (added).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffRow {
    Equal(String),
    Delete(String),
    Insert(String),
}

impl DiffRow {
    fn is_change(&self) -> bool {
        !matches!(self, DiffRow::Equal(_))
    }
}

/// A contiguous run of changed rows, as `[start, end)` row indices. Used for
/// next/previous-change navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    pub start: usize,
    pub end: usize,
}

/// A side-by-side view over one before/after pair, plus navigation state.
///
/// ```
/// # use remendo::diff_view::DiffView;
/// let view = DiffView::new("src/lib.rs", "old\n", "new\n");
/// assert_eq!(view.hunk_count(), 1);
/// ```
#[derive(Debug)]
pub struct DiffView {
    /// Shown in the header. Display only — nothing reads the filesystem here.
    path: String,
    rows: Vec<DiffRow>,
    hunks: Vec<Hunk>,
    current_hunk: usize,
    scroll: usize,
}

impl DiffView {
    /// Build the view for `path` from its `before` and `after` texts.
    ///
    /// ```
    /// # use remendo::diff_view::DiffView;
    /// let view = DiffView::new("a.rs", "keep\nold\n", "keep\nnew\n");
    /// assert!(!view.is_empty());
    /// ```
    pub fn new(path: impl Into<String>, before: &str, after: &str) -> Self {
        let rows = build_rows(before, after);
        let hunks = build_hunks(&rows);
        Self {
            path: path.into(),
            rows,
            hunks,
            current_hunk: 0,
            scroll: 0,
        }
    }

    /// True when the two texts are identical — an apply turn that changed
    /// nothing. The caller decides what that means; this view just has no rows
    /// to show.
    ///
    /// ```
    /// # use remendo::diff_view::DiffView;
    /// assert!(DiffView::new("a.rs", "same\n", "same\n").is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty()
    }

    /// Number of contiguous changed runs.
    pub fn hunk_count(&self) -> usize {
        self.hunks.len()
    }

    /// Current scroll offset, in rows.
    pub fn scroll_offset(&self) -> usize {
        self.scroll
    }

    // --- navigation --------------------------------------------------------

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        if self.scroll + 1 < self.rows.len() {
            self.scroll += 1;
        }
    }

    pub fn page_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(PAGE_ROWS);
    }

    pub fn page_down(&mut self) {
        let max = self.rows.len().saturating_sub(1);
        self.scroll = (self.scroll + PAGE_ROWS).min(max);
    }

    /// Jump to the next hunk, wrapping to the first; scrolls it into view.
    pub fn next_hunk(&mut self) {
        if self.hunks.is_empty() {
            return;
        }
        self.current_hunk = (self.current_hunk + 1) % self.hunks.len();
        self.scroll = self.hunks[self.current_hunk].start;
    }

    /// Jump to the previous hunk, wrapping to the last; scrolls it into view.
    pub fn prev_hunk(&mut self) {
        if self.hunks.is_empty() {
            return;
        }
        let n = self.hunks.len();
        self.current_hunk = (self.current_hunk + n - 1) % n;
        self.scroll = self.hunks[self.current_hunk].start;
    }

    // --- rendering ---------------------------------------------------------

    /// Draw the view over `area`, claiming it entirely.
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        frame.render_widget(Clear, area);
        let block = theme.block(Borders::ALL);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.rows.is_empty() {
            let p = Paragraph::new("No changes.").style(Style::new().fg(theme.text_muted));
            frame.render_widget(p, inner);
            return;
        }

        let [header, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
        self.render_header(frame, header, theme);
        self.render_body(frame, body, theme);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let style = Style::new().fg(theme.text).add_modifier(Modifier::BOLD);
        let header = Paragraph::new(Span::styled(self.path.clone(), style));
        frame.render_widget(header, area);
    }

    /// Two equal halves with a thin divider: the pre-turn snapshot on the left,
    /// the edit on the right.
    fn render_body(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let [left, mid, right] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(area);

        let height = area.height as usize;
        let mut left_lines: Vec<Line> = Vec::with_capacity(height);
        let mut right_lines: Vec<Line> = Vec::with_capacity(height);
        for row in self.rows.iter().skip(self.scroll).take(height) {
            let (l, r) = render_row(row, theme);
            left_lines.push(l);
            right_lines.push(r);
        }
        frame.render_widget(Paragraph::new(left_lines), left);
        frame.render_widget(theme.block(Borders::LEFT), mid);
        frame.render_widget(Paragraph::new(right_lines), right);
    }
}

/// Render one diff row into its (left, right) styled lines, gap-filling the
/// opposite side of an insert/delete.
fn render_row(row: &DiffRow, theme: &Theme) -> (Line<'static>, Line<'static>) {
    match row {
        DiffRow::Equal(t) => (Line::raw(t.clone()), Line::raw(t.clone())),
        DiffRow::Delete(t) => (
            Line::styled(
                t.clone(),
                Style::new().fg(theme.diff_del_fg).bg(theme.diff_del_bg),
            ),
            Line::styled("", Style::new().bg(theme.diff_gap_bg)),
        ),
        DiffRow::Insert(t) => (
            Line::styled("", Style::new().bg(theme.diff_gap_bg)),
            Line::styled(
                t.clone(),
                Style::new().fg(theme.diff_add_fg).bg(theme.diff_add_bg),
            ),
        ),
    }
}

/// Align `old` and `new` into a row stream via `similar`. Deleted lines become
/// left-only `Delete` rows, inserted lines right-only `Insert` rows, and equal
/// lines sit on both sides. Line endings are normalized away.
///
/// ```
/// # use remendo::diff_view::{build_rows, DiffRow};
/// let rows = build_rows("", "a\n");
/// assert_eq!(rows, vec![DiffRow::Insert("a".into())]);
/// ```
pub fn build_rows(old: &str, new: &str) -> Vec<DiffRow> {
    // Normalize line endings before diffing so CRLF/LF differences don't show
    // up as spurious whole-line changes.
    let old = old.replace("\r\n", "\n");
    let new = new.replace("\r\n", "\n");
    let diff = TextDiff::from_lines(&old, &new);
    let mut rows = Vec::new();
    for change in diff.iter_all_changes() {
        let text = change.value().trim_end_matches('\n').to_string();
        rows.push(match change.tag() {
            ChangeTag::Equal => DiffRow::Equal(text),
            ChangeTag::Delete => DiffRow::Delete(text),
            ChangeTag::Insert => DiffRow::Insert(text),
        });
    }
    rows
}

/// Group contiguous changed rows into hunks for next/previous navigation.
///
/// ```
/// # use remendo::diff_view::{build_hunks, build_rows};
/// let hunks = build_hunks(&build_rows("a\n", "b\n"));
/// assert_eq!(hunks.len(), 1);
/// ```
pub fn build_hunks(rows: &[DiffRow]) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let mut start = None;
    for (i, row) in rows.iter().enumerate() {
        match (row.is_change(), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                hunks.push(Hunk { start: s, end: i });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        hunks.push(Hunk {
            start: s,
            end: rows.len(),
        });
    }
    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_add_is_all_inserts_on_the_right() {
        let rows = build_rows("", "a\nb\n");
        assert_eq!(
            rows,
            vec![DiffRow::Insert("a".into()), DiffRow::Insert("b".into())]
        );
    }

    #[test]
    fn pure_delete_is_all_deletes_on_the_left() {
        let rows = build_rows("a\nb\n", "");
        assert_eq!(
            rows,
            vec![DiffRow::Delete("a".into()), DiffRow::Delete("b".into())]
        );
    }

    #[test]
    fn mixed_change_keeps_equal_lines_aligned() {
        let rows = build_rows("keep\nold\ntail\n", "keep\nnew\ntail\n");
        assert_eq!(
            rows,
            vec![
                DiffRow::Equal("keep".into()),
                DiffRow::Delete("old".into()),
                DiffRow::Insert("new".into()),
                DiffRow::Equal("tail".into()),
            ]
        );
    }

    #[test]
    fn crlf_does_not_create_spurious_diffs() {
        let rows = build_rows("a\r\nb\r\n", "a\nb\n");
        assert!(rows.iter().all(|r| matches!(r, DiffRow::Equal(_))));
    }

    #[test]
    fn hunks_group_contiguous_changes() {
        let rows = vec![
            DiffRow::Equal("0".into()),
            DiffRow::Delete("1".into()),
            DiffRow::Insert("2".into()),
            DiffRow::Equal("3".into()),
            DiffRow::Equal("4".into()),
            DiffRow::Insert("5".into()),
        ];
        let hunks = build_hunks(&rows);
        assert_eq!(
            hunks,
            vec![Hunk { start: 1, end: 3 }, Hunk { start: 5, end: 6 }]
        );
    }

    #[test]
    fn next_prev_hunk_wraps_and_scrolls() {
        let mut v = DiffView::new("a.rs", "0\nsame\n", "same\n2\n");
        assert_eq!(v.hunk_count(), 2);
        v.next_hunk();
        assert_eq!(v.current_hunk, 1);
        assert_eq!(v.scroll_offset(), v.hunks[1].start);
        v.next_hunk(); // wraps to first
        assert_eq!(v.current_hunk, 0);
        assert_eq!(v.scroll_offset(), v.hunks[0].start);
        v.prev_hunk(); // wraps back to last
        assert_eq!(v.current_hunk, 1);
    }

    #[test]
    fn hunk_navigation_on_an_unchanged_pair_is_inert() {
        let mut v = DiffView::new("a.rs", "same\n", "same\n");
        assert!(v.is_empty());
        v.next_hunk();
        v.prev_hunk();
        assert_eq!(v.scroll_offset(), 0);
    }

    #[test]
    fn scrolling_clamps_at_both_ends() {
        let mut v = DiffView::new("a.rs", "a\nb\nc\n", "a\nB\nc\n");
        v.scroll_up(); // already at 0
        assert_eq!(v.scroll_offset(), 0);
        v.page_down();
        assert_eq!(v.scroll_offset(), v.rows.len() - 1);
        v.scroll_down(); // clamped at the last row
        assert_eq!(v.scroll_offset(), v.rows.len() - 1);
    }

    /// The confirm-diff's left side is the pre-turn snapshot, so a second
    /// comment on the same file must show only *its* change — not the edit
    /// already confirmed for the first comment. Diffing against the patchset
    /// baseline is the bug this guards (dry-run finding #6).
    #[test]
    fn diff_is_against_the_supplied_before_not_an_original() {
        let baseline = "fn a() {}\nfn b() {}\n";
        let after_first = "fn a() { one(); }\nfn b() {}\n";
        let after_second = "fn a() { one(); }\nfn b() { two(); }\n";

        let view = DiffView::new("a.rs", after_first, after_second);
        let changed: Vec<_> = view.rows.iter().filter(|r| r.is_change()).collect();
        assert_eq!(changed.len(), 2, "only comment 2's line should differ");
        assert!(changed.iter().all(|r| match r {
            DiffRow::Delete(t) | DiffRow::Insert(t) => t.contains("fn b()"),
            DiffRow::Equal(_) => false,
        }));

        // Against the baseline instead, comment 1's confirmed work leaks into
        // the diff the reviewer is asked to approve.
        let wrong = DiffView::new("a.rs", baseline, after_second);
        let leaked = wrong.rows.iter().any(|r| match r {
            DiffRow::Delete(t) | DiffRow::Insert(t) => t.contains("one();"),
            DiffRow::Equal(_) => false,
        });
        assert!(leaked, "baseline diff contains comment 1's confirmed edit");
    }
}
