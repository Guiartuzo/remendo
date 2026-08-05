//! The minibuffer: a single reusable prompt rendered on the bottom row that
//! collects input for a feature (search, go-to-line, and — later — fuzzy
//! file-find). One prompt is active at a time; the mode decides its label and
//! what its input drives. The input is global here; per-feature result state
//! (e.g. search matches) lives with the focused pane.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use crate::theme::Theme;

/// DIVERGENCE from vybim, which carried a `MiniMode` enum naming the feature
/// the prompt was driving. That existed because vybim had no application-level
/// mode to hold the answer; Remendo's `app::Mode` does, so a second copy here
/// would be a state that could disagree with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Minibuffer {
    /// The fixed label shown before the input (e.g. `Comment: `).
    prompt: String,
    /// The text the user has typed so far.
    pub input: String,
}

impl Minibuffer {
    /// A prompt labelled `prompt`, starting empty.
    ///
    /// ```
    /// # use remendo::minibuffer::Minibuffer;
    /// let mut mini = Minibuffer::new("Comment: ");
    /// mini.push('h');
    /// assert_eq!(mini.input, "h");
    /// ```
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            input: String::new(),
        }
    }

    /// A prompt pre-filled with text to edit, with the cursor past its end.
    /// Editing a drafted reply starts from the draft, not from nothing.
    pub fn editing(prompt: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            input: text.into(),
        }
    }

    pub fn push(&mut self, c: char) {
        self.input.push(c);
    }

    pub fn backspace(&mut self) {
        self.input.pop();
    }

    /// Whether anything has been typed.
    pub fn is_empty(&self) -> bool {
        self.input.trim().is_empty()
    }

    /// Draw `<prompt><input>` across `area` and return the screen column where
    /// the text cursor belongs (just past the input, clamped into the row).
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) -> u16 {
        let text = format!("{}{}", self.prompt, self.input);
        let style = Style::new().bg(theme.prompt_bg).fg(theme.text);
        frame.render_widget(Paragraph::new(text).style(style), area);
        let col = (self.prompt.chars().count() + self.input.chars().count()) as u16;
        area.x + col.min(area.width.saturating_sub(1))
    }
}
