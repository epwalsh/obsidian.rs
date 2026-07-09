use super::*;

#[derive(Debug)]
pub enum ExtractNoteSelection {
    Section(String),
    Range(Range),
}

#[derive(Debug)]
pub struct ExtractNoteRequest {
    pub(in crate::state) snapshot: StateSnapshot,
    pub(in crate::state) path: PathBuf,
    pub(in crate::state) selection: ExtractNoteSelection,
    pub(in crate::state) new_path: PathBuf,
    pub(in crate::state) new_id: Option<String>,
    pub(in crate::state) replace_with: Option<String>,
}

impl ExtractNoteRequest {
    pub fn compute(self) -> Result<ExtractEdits, StateError> {
        let source_text = self.snapshot.text_for_path(&self.path)?;
        let vault = self.snapshot.build_vault()?;
        let selection = match self.selection {
            ExtractNoteSelection::Section(section) => ExtractSelection::Section(section),
            ExtractNoteSelection::Range(range) => {
                let span =
                    text_span_from_lsp_range(&source_text, range).ok_or_else(|| StateError::InvalidExtractTarget {
                        path: self.path.clone(),
                        new_path: self.new_path.display().to_string(),
                    })?;
                ExtractSelection::Span(span)
            }
        };
        Ok(vault.extract_to_note_edits_from_text(
            &self.path,
            &source_text,
            &selection,
            &self.new_path,
            self.new_id.as_deref(),
            self.replace_with.as_deref(),
        )?)
    }
}

fn text_span_from_lsp_range(text: &str, range: Range) -> Option<TextSpan> {
    let start_line = range.start.line as usize;
    let end_line = range.end.line as usize;
    let start_text = text.lines().nth(start_line)?;
    let end_text = text.lines().nth(end_line)?;
    let start_byte = lsp_character_to_byte_index(start_text, range.start.character);
    let end_byte = lsp_character_to_byte_index(end_text, range.end.character);

    Some(TextSpan {
        start_line: start_line + 1,
        start_col: start_text[..start_byte].chars().count(),
        end_line: end_line + 1,
        end_col: end_text[..end_byte].chars().count(),
    })
}
