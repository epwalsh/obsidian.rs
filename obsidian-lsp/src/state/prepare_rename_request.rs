use super::*;

#[derive(Debug)]
pub struct PrepareRenameRequest {
    pub(in crate::state) snapshot: StateSnapshot,
    pub(in crate::state) path: PathBuf,
    pub(in crate::state) position: Position,
}

impl PrepareRenameRequest {
    pub fn compute(self) -> Result<Option<PrepareRenameResponse>, StateError> {
        let source_note = self.snapshot.note_for_path(&self.path)?;
        if find_link_at_position(&source_note, self.position).is_none()
            && let Some(selected_tag) = find_tag_at_position(&self.snapshot, &source_note, self.position)?
        {
            return Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                range: selected_tag.rename_range,
                placeholder: selected_tag.placeholder,
            }));
        }

        let Some(target) = rename_target(&self.snapshot, &self.path, self.position)? else {
            return Ok(None);
        };

        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: target.range,
            placeholder: target.placeholder,
        }))
    }
}
