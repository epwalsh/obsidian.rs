#[derive(Clone, Debug, PartialEq)]
pub enum Location {
    Frontmatter,
    Inline(InlineLocation),
}

/// Position of an inline element within a text.
///
/// Lines are 1-indexed; columns are 0-indexed and character-based (not byte-based).
/// `col_end` is exclusive (past-the-end).
#[derive(Clone, Debug, PartialEq)]
pub struct InlineLocation {
    pub line: usize,
    pub col_start: usize,
    pub col_end: usize,
}
