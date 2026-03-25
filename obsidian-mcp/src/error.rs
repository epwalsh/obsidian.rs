use obsidian_core::{NoteError, SearchError, VaultError};
use rmcp::model::ErrorCode;

fn make_err(msg: String) -> rmcp::ErrorData {
    rmcp::ErrorData {
        code: ErrorCode::INTERNAL_ERROR,
        message: msg.into(),
        data: None,
    }
}

pub fn vault_err(e: VaultError) -> rmcp::ErrorData {
    make_err(e.to_string())
}

pub fn note_err(e: NoteError) -> rmcp::ErrorData {
    make_err(e.to_string())
}

pub fn search_err(e: SearchError) -> rmcp::ErrorData {
    make_err(e.to_string())
}

pub fn other_err(msg: impl Into<String>) -> rmcp::ErrorData {
    make_err(msg.into())
}
