//! Incremental search over a [`Buffer`]: which positions match, and which one
//! is current.
//!
//! Split out of `pane.rs` on `config.yaml`'s one-responsibility rule. The
//! division is that [`Search`] knows *what matches and which is selected*,
//! while the pane knows *where the cursor and selection sit* — so this module
//! never touches a cursor, and the pane never scans for text.

use crate::buffer::Buffer;
use crate::pane::Cursor;

/// A match: its `(line, col)` start and its length in characters.
pub type Match = ((usize, usize), usize);

/// Active incremental-search state: the match positions, which one is current,
/// the cursor to restore on cancel, and the query's char length.
///
/// ```
/// # use remendo::{buffer::Buffer, pane::Cursor, search::Search};
/// let buffer = Buffer::from_text("alpha\nbeta\nalpha\n");
/// let mut search = Search::begin(Cursor::default());
/// search.update(&buffer, "alpha");
/// assert_eq!(search.current_match(), Some(((0, 0), 5)));
/// ```
#[derive(Debug)]
pub struct Search {
    /// `(line, col)` start of each match, in buffer order.
    matches: Vec<(usize, usize)>,
    current: usize,
    origin: Cursor,
    len: usize,
}

impl Search {
    /// Start a search from `origin`, the cursor a cancel restores.
    pub fn begin(origin: Cursor) -> Self {
        Self {
            matches: Vec::new(),
            current: 0,
            origin,
            len: 0,
        }
    }

    /// The cursor the search started from.
    pub fn origin(&self) -> Cursor {
        self.origin
    }

    /// Recompute matches for `query` and select the nearest one at or after the
    /// origin, wrapping to the first. An empty query clears the match set.
    pub fn update(&mut self, buffer: &Buffer, query: &str) {
        self.matches = if query.is_empty() {
            Vec::new()
        } else {
            find_matches(buffer, query)
        };
        self.len = query.chars().count();
        let origin = (self.origin.line, self.origin.col);
        self.current = nearest_at_or_after(&self.matches, origin).unwrap_or(0);
    }

    /// Advance to the next match, wrapping past the last to the first.
    pub fn next(&mut self) {
        if !self.matches.is_empty() {
            self.current = (self.current + 1) % self.matches.len();
        }
    }

    /// Step back to the previous match, wrapping past the first to the last.
    pub fn prev(&mut self) {
        if !self.matches.is_empty() {
            let n = self.matches.len();
            self.current = (self.current + n - 1) % n;
        }
    }

    /// The currently selected match, or `None` when nothing matches.
    ///
    /// ```
    /// # use remendo::{buffer::Buffer, pane::Cursor, search::Search};
    /// let buffer = Buffer::from_text("alpha\n");
    /// let mut search = Search::begin(Cursor::default());
    /// search.update(&buffer, "zzz");
    /// assert_eq!(search.current_match(), None);
    /// ```
    pub fn current_match(&self) -> Option<Match> {
        Some((*self.matches.get(self.current)?, self.len))
    }
}

/// All `(line, col)` starts where `query` matches case-insensitively (ASCII
/// case folding). Matching works in character columns so highlight positions
/// line up with the char-based cursor. A query never spans a newline, so every
/// match is contained within a single line.
fn find_matches(buffer: &Buffer, query: &str) -> Vec<(usize, usize)> {
    let needle: Vec<char> = query.chars().collect();
    let nlen = needle.len();
    if nlen == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in 0..buffer.line_count() {
        let hay: Vec<char> = buffer.line_text(line).chars().collect();
        if hay.len() < nlen {
            continue;
        }
        for start in 0..=(hay.len() - nlen) {
            if hay[start..start + nlen]
                .iter()
                .zip(&needle)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
            {
                out.push((line, start));
            }
        }
    }
    out
}

/// Index of the first match at or after `pos` in a buffer-ordered match list,
/// or `None` if every match precedes `pos`.
fn nearest_at_or_after(matches: &[(usize, usize)], pos: (usize, usize)) -> Option<usize> {
    matches.iter().position(|&m| m >= pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search_from(line: usize, col: usize) -> Search {
        Search::begin(Cursor {
            line,
            col,
            target_col: col,
        })
    }

    #[test]
    fn matching_is_case_insensitive() {
        let b = Buffer::from_text("Alpha\nbeta\nALPHA\n");
        assert_eq!(find_matches(&b, "alpha"), vec![(0, 0), (2, 0)]);
    }

    #[test]
    fn a_match_shorter_than_the_line_is_found_at_its_column() {
        let b = Buffer::from_text("xx needle xx\n");
        assert_eq!(find_matches(&b, "needle"), vec![(0, 3)]);
    }

    #[test]
    fn an_empty_query_matches_nothing() {
        let b = Buffer::from_text("alpha\n");
        assert!(find_matches(&b, "").is_empty());
    }

    #[test]
    fn nearest_picks_the_first_match_at_or_after() {
        let matches = vec![(0, 0), (2, 0), (4, 0)];
        assert_eq!(nearest_at_or_after(&matches, (1, 0)), Some(1));
        assert_eq!(nearest_at_or_after(&matches, (2, 0)), Some(1));
        assert_eq!(nearest_at_or_after(&matches, (9, 0)), None);
    }

    #[test]
    fn update_selects_the_match_at_or_after_the_origin() {
        let b = Buffer::from_text("alpha\nbeta\nalpha\n");
        let mut s = search_from(1, 0);
        s.update(&b, "alpha");
        assert_eq!(s.current_match(), Some(((2, 0), 5)));
    }

    #[test]
    fn next_and_prev_wrap_in_both_directions() {
        let b = Buffer::from_text("alpha\nbeta\nalpha\n");
        let mut s = search_from(0, 0);
        s.update(&b, "alpha");
        assert_eq!(s.current_match(), Some(((0, 0), 5)));
        s.next();
        assert_eq!(s.current_match(), Some(((2, 0), 5)));
        s.next(); // wraps to the first
        assert_eq!(s.current_match(), Some(((0, 0), 5)));
        s.prev(); // wraps back to the last
        assert_eq!(s.current_match(), Some(((2, 0), 5)));
    }

    #[test]
    fn navigation_with_no_matches_is_inert() {
        let b = Buffer::from_text("alpha\n");
        let mut s = search_from(0, 0);
        s.update(&b, "zzz");
        s.next();
        s.prev();
        assert_eq!(s.current_match(), None);
    }

    #[test]
    fn re_updating_to_an_empty_query_clears_the_matches() {
        let b = Buffer::from_text("alpha\n");
        let mut s = search_from(0, 0);
        s.update(&b, "alpha");
        assert!(s.current_match().is_some());
        s.update(&b, "");
        assert_eq!(s.current_match(), None);
    }
}
