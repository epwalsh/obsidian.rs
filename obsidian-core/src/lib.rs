pub mod common;
mod error;
pub mod health;
mod link;
mod note;
pub mod search;
mod tag;
mod vault;

pub use common::{InlineLocation, Location};
pub use error::{NoteError, SearchError, VaultError};
pub use health::{
    BrokenLink, DuplicateAlias, DuplicateId, NoteRef, StrandedNote, VaultHealthReport, backlinks_from, check_notes,
};
pub use link::{Link, LocatedLink};
pub use note::{Note, NoteBuilder, default_note_id_for_path, normalize_note_id};
pub use search::{SearchQuery, SortOrder};
pub use tag::LocatedTag;
pub use vault::{
    ExtractEdits, ExtractResult, ExtractSelection, MergePreview, RenameEdits, RenamePreview, TextSpan, Vault,
};
