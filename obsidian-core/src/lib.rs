mod link;
mod note;
pub mod search;
mod vault;
pub use link::{Link, LocatedLink, Location};
pub use note::Note;
pub use search::{SearchError, SearchQuery};
pub use vault::Vault;
