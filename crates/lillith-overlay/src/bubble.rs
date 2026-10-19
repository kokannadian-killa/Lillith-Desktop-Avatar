//! Speech bubbles above her head. TODO(M3): shape text with `cosmic-text`
//! (feature `text`), mood-tinted rounded rect, auto-hide after a few seconds.

/// A visible speech bubble.
#[derive(Debug, Clone, PartialEq)]
pub struct Bubble {
    pub text: String,
    pub remaining_s: f32,
}

impl Bubble {
    /// Bubble lifetime grows with text length (min 2.5 s).
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let remaining_s = (text.chars().count() as f32 * 0.06).max(2.5);
        Self { text, remaining_s }
    }
}
