use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use obsidian_core::{Location, Note, Vault};
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde_json::json;

use crate::error::{note_err, other_err, search_err, vault_err};
use crate::tools::{
    ListNotesParams, ListTagsParams, PatchNoteParams, ReadNoteParams, RenameNoteParams, SearchByTagParams,
    SearchNotesParams, UpdateNoteParams, WriteNoteParams,
};

pub struct VaultServer {
    vault: Arc<Mutex<Vault>>,
    #[allow(dead_code)] // Used by the macro expansion.
    tool_router: ToolRouter<Self>,
}

impl VaultServer {
    pub fn new(vault: Vault) -> Self {
        Self {
            vault: Arc::new(Mutex::new(vault)),
            tool_router: Self::tool_router(),
        }
    }
}

fn vault_rel_path(path: &Path, vault_path: &Path) -> String {
    path.strip_prefix(vault_path).unwrap_or(path).display().to_string()
}

fn note_to_json(note: &Note, vault_path: &Path) -> Result<serde_json::Value, rmcp::ErrorData> {
    let mut map = note.frontmatter_json().map_err(note_err)?;
    map.insert(
        "path".to_string(),
        serde_json::Value::String(vault_rel_path(&note.path, vault_path)),
    );
    Ok(serde_json::Value::Object(map))
}

#[tool_router]
impl VaultServer {
    #[tool(
        description = "Read a note's content and/or frontmatter from the vault",
        annotations(read_only_hint = true, destructive_hint = false, open_world_hint = false)
    )]
    async fn read_note(&self, Parameters(p): Parameters<ReadNoteParams>) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let vault = vault.lock().unwrap();
            let include_frontmatter = p.include_frontmatter.unwrap_or(true);
            let include_content = p.include_content.unwrap_or(true);
            let mut note = vault.resolve_note(&p.note).map_err(vault_err)?;

            let mut out = json!({});
            if include_frontmatter {
                out["frontmatter"] = serde_json::Value::Object(note.frontmatter_json().map_err(note_err)?);
            }
            if include_content {
                note.load_body().map_err(note_err)?; // Ensure content is loaded before reading.
                out["content"] = json!(note.read(false).map_err(note_err)?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| other_err(e.to_string()))??;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    #[tool(
        description = "List all notes in the vault",
        annotations(read_only_hint = true, destructive_hint = false, open_world_hint = false)
    )]
    async fn list_notes(&self, Parameters(p): Parameters<ListNotesParams>) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let vault = vault.lock().unwrap();
            let mut query = vault.search();
            if let Some(sort) = p.sort {
                query = query.sort_by(sort.into())
            };

            let notes: Vec<Note> = query
                .execute()
                .map_err(search_err)?
                .into_iter()
                .filter_map(|r| r.ok())
                .collect();

            let items: Result<Vec<serde_json::Value>, rmcp::ErrorData> =
                notes.iter().map(|n| note_to_json(n, vault.path())).collect();
            Ok(serde_json::Value::Array(items?))
        })
        .await
        .map_err(|e| other_err(e.to_string()))??;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    #[tool(
        description = "Create or overwrite a note in the vault",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn write_note(&self, Parameters(p): Parameters<WriteNoteParams>) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let vault = vault.lock().unwrap();
            let (note_path, _) = vault.resolve_note_path(&p.path, false).map_err(vault_err)?;
            if !p.force.unwrap_or(false) && note_path.exists() {
                return Err(other_err(format!(
                    "note already exists: {}. Set force=true to overwrite.",
                    note_path.display()
                )));
            }

            let mut note = Note::parse(&note_path, &p.content);

            for tag in p.tags.unwrap_or_default() {
                note.add_tag(tag);
            }
            for alias in p.aliases.unwrap_or_default() {
                note.add_alias(alias);
            }
            if let Some(title) = p.title {
                note.title = Some(title.clone());
                note.add_alias(title);
            } else if note.title.is_none() {
                if !note.aliases.is_empty() {
                    note.title = Some(note.aliases[0].clone());
                } else {
                    return Err(other_err(
                        "no title provided and could not infer title from content or aliases",
                    ));
                }
            }

            // Ensure parent directory exists before writing.
            if let Some(parent) = note_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| other_err(format!("failed to create directory: {e}")))?;
            }

            note.write().map_err(note_err)?;
            note_to_json(&note, vault.path())
        })
        .await
        .map_err(|e| other_err(e.to_string()))??;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    #[tool(
        description = "Replace an exact string occurrence in a note (must appear exactly once)",
        annotations(read_only_hint = false, destructive_hint = true, open_world_hint = false)
    )]
    async fn patch_note(&self, Parameters(p): Parameters<PatchNoteParams>) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let mut vault = vault.lock().unwrap();
            let mut note = vault.resolve_note(&p.note).map_err(vault_err)?;
            note.load_body().map_err(note_err)?; // Ensure content is loaded before patching.
            let patched = vault
                .patch_note(&note, &p.old_string, &p.new_string)
                .map_err(vault_err)?;
            note_to_json(&patched, vault.path())
        })
        .await
        .map_err(|e| other_err(e.to_string()))??;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    #[tool(
        description = "Update a note's frontmatter: add/remove tags, add aliases, or set arbitrary fields",
        annotations(read_only_hint = false, destructive_hint = false, open_world_hint = false)
    )]
    async fn update_note(
        &self,
        Parameters(p): Parameters<UpdateNoteParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let vault = vault.lock().unwrap();
            let mut note = vault.resolve_note(&p.note).map_err(vault_err)?;

            let mut dirty = false;
            for tag in p.add_tags.unwrap_or_default() {
                note.add_tag(tag);
                dirty = true;
            }
            for tag in p.remove_tags.unwrap_or_default() {
                note.remove_tag(&tag);
                dirty = true;
            }
            for alias in p.add_aliases.unwrap_or_default() {
                note.add_alias(alias);
                dirty = true;
            }
            for (key, json_val) in p.set_fields.unwrap_or_default() {
                let yaml_val: serde_yaml::Value = serde_yaml::to_value(&json_val).unwrap_or(serde_yaml::Value::Null);
                note.set_field(&key, &yaml_val).map_err(note_err)?;
                dirty = true;
            }

            if dirty {
                note.write_frontmatter().map_err(note_err)?;
            }

            note_to_json(&note, vault.path())
        })
        .await
        .map_err(|e| other_err(e.to_string()))??;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    #[tool(
        description = "Search for notes in the vault with optional filters",
        annotations(read_only_hint = true, destructive_hint = false, open_world_hint = false)
    )]
    async fn search_notes(
        &self,
        Parameters(p): Parameters<SearchNotesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let vault = vault.lock().unwrap();
            let mut query = vault.search();

            for tag in p.tags.unwrap_or_default() {
                query = query.and_has_tag(tag);
            }
            if let Some(title) = p.title_contains {
                query = query.and_title_contains(title);
            }
            if let Some(content) = p.content_contains {
                query = query.and_content_contains(content);
            }
            if let Some(glob) = p.glob {
                query = query.and_glob(glob);
            }
            if let Some(id) = p.id {
                query = query.and_has_id(id);
            }
            if let Some(alias) = p.alias {
                query = query.and_has_alias(alias);
            }
            if let Some(sort) = p.sort {
                query = query.sort_by(sort.into())
            };

            let notes: Vec<Note> = query
                .execute()
                .map_err(search_err)?
                .into_iter()
                .filter_map(|r| r.ok())
                .collect();

            let items: Result<Vec<serde_json::Value>, rmcp::ErrorData> =
                notes.iter().map(|n| note_to_json(n, vault.path())).collect();
            Ok(serde_json::Value::Array(items?))
        })
        .await
        .map_err(|e| other_err(e.to_string()))??;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    #[tool(
        description = "Rename a note and update all backlinks throughout the vault",
        annotations(read_only_hint = false, destructive_hint = true, open_world_hint = false)
    )]
    async fn rename_note(
        &self,
        Parameters(p): Parameters<RenameNoteParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let mut vault = vault.lock().unwrap();
            let note = vault.resolve_note(&p.note).map_err(vault_err)?;

            let mut new_path = PathBuf::from(&p.new_path);
            if !new_path.is_absolute() {
                new_path = vault.path().join(&new_path);
            }
            if new_path.extension().is_none() {
                new_path.set_extension("md");
            }

            let renamed = vault.rename(&note, &new_path).map_err(vault_err)?;
            note_to_json(&renamed, vault.path())
        })
        .await
        .map_err(|e| other_err(e.to_string()))??;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    #[tool(
        description = "List all unique tags used across the vault (frontmatter and inline)",
        annotations(read_only_hint = true, destructive_hint = false, open_world_hint = false)
    )]
    async fn list_tags(&self, Parameters(_p): Parameters<ListTagsParams>) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let vault = vault.lock().unwrap();
            let tags = vault.list_tags().map_err(vault_err)?;
            Ok(json!(tags))
        })
        .await
        .map_err(|e| other_err(e.to_string()))??;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    #[tool(
        description = "Find occurrences of specific tags. Also matches sub-tags: searching 'workout' returns notes with 'workout/upper-body'.",
        annotations(read_only_hint = true, destructive_hint = false, open_world_hint = false)
    )]
    async fn search_tags(
        &self,
        Parameters(p): Parameters<SearchByTagParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let vault = vault.lock().unwrap();
            let mut results = vault.find_tags(&p.tags).map_err(vault_err)?;
            if let Some(sort) = p.sort {
                obsidian_core::search::sort_notes_by(&mut results, |(n, _)| Some(n), &sort.into());
            }
            let mut items: Vec<serde_json::Value> = Vec::new();
            for (n, nt) in results {
                if !nt.is_empty() {
                    let tags: Vec<serde_json::Value> = nt
                        .into_iter()
                        .map(|lt| match lt.location {
                            Location::Frontmatter => json!({ "tag": lt.tag, "location": "frontmatter" }),
                            Location::Inline(loc) => json!({
                                "tag": lt.tag,
                                "location": { "line": loc.line, "col_start": loc.col_start, "col_end": loc.col_end },
                            }),
                        })
                        .collect();
                    items.push(json!({
                        "source_path": vault_rel_path(&n.path, vault.path()),
                        "source_id": n.id,
                        "tags": tags,
                    }));
                }
            }

            Ok(json!(items))
        })
        .await
        .map_err(|e| other_err(e.to_string()))??;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }
}

#[tool_handler]
impl ServerHandler for VaultServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "MCP server for Obsidian vaults. Provides tools to read, write, search, \
                 and manage notes in an Obsidian vault.",
        )
    }
}
