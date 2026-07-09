use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use obsidian_core::{
    ExtractResult, ExtractSelection, Link, LocatedLink, Location, Note, TextSpan, Vault, VaultHealthReport,
};
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde_json::json;

use crate::error::{note_err, other_err, search_err, vault_err};
use crate::tools::{
    AppendToNoteParams, CheckVaultParams, ExtractToNoteParams, ListBacklinksParams, ListNotesParams, ListTagsParams,
    PatchNoteParams, ReadNoteParams, RenameNoteParams, SearchByTagParams, SearchNotesParams, UpdateNoteParams,
    WriteNoteParams,
};

const SERVER_INSTRUCTIONS: &str = r#"Use this MCP server to work with an Obsidian vault as a collection of Markdown notes.

Capabilities:
- Discover notes by path, title, alias, ID, tag, content substring, regex, or glob.
- Read note bodies and YAML frontmatter; list notes, tags, and backlinks.
- Create notes, extract sections or spans into new notes, append to note bodies, update frontmatter, patch exact body text without rewriting frontmatter, and rename notes while updating backlinks.
- Check vault health for duplicate IDs or aliases, broken links, and stranded notes.

Operational guidance:
- Note identifiers can be vault-relative paths, current-working-directory-relative paths, absolute paths, frontmatter IDs, or aliases when a tool accepts `note`.
- Prefer read-only tools (`search_notes`, `read_note`, `list_notes`, `list_backlinks`, `list_tags`, `search_tags`, `check_vault`) before modifying content.
- Use `extract_to_note` when splitting content into a new note, `append_to_note` for additive body updates, `patch_note` only when `old_string` appears exactly once, and `write_note` with `force=true` only when intentionally replacing a whole note.
- Prefer vault-relative paths for writes and renames; tool results use vault-relative paths when possible.
"#;

fn build_ignore_set(patterns: &[String]) -> Result<globset::GlobSet, rmcp::ErrorData> {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        let glob = globset::GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| other_err(format!("invalid ignore pattern '{}': {}", pattern, e)))?;
        builder.add(glob);
    }
    builder.build().map_err(|e| other_err(e.to_string()))
}

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

fn extract_result_to_json(result: &ExtractResult, vault_path: &Path) -> Result<serde_json::Value, rmcp::ErrorData> {
    Ok(json!({
        "source_note": note_to_json(&result.source_note, vault_path)?,
        "new_note": note_to_json(&result.new_note, vault_path)?,
    }))
}

fn extract_selection_from_params(params: &ExtractToNoteParams) -> Result<ExtractSelection, rmcp::ErrorData> {
    match (&params.section, &params.span) {
        (Some(_), Some(_)) => Err(other_err("provide either section or span, not both")),
        (None, None) => Err(other_err("either section or span is required")),
        (Some(section), None) => Ok(ExtractSelection::Section(section.clone())),
        (None, Some(span)) => Ok(ExtractSelection::Span(TextSpan {
            start_line: span.start_line,
            start_col: span.start_col,
            end_line: span.end_line,
            end_col: span.end_col,
        })),
    }
}

fn backlink_link_to_json(link: &LocatedLink) -> serde_json::Value {
    let (kind, mut target, heading, display) = match &link.link {
        Link::Wiki {
            target, heading, alias, ..
        } => (
            "wiki",
            target.clone(),
            heading.clone(),
            alias.clone().unwrap_or(target.clone()),
        ),
        Link::Markdown { text, url, .. } => ("markdown", url.clone(), None, text.clone()),
        Link::Embed {
            target, heading, alias, ..
        } => (
            "embed",
            target.clone(),
            heading.clone(),
            alias.clone().unwrap_or(target.clone()),
        ),
    };
    if let Some(heading) = heading {
        target = format!("{}#{}", target, heading);
    }

    json!({
        "kind": kind,
        "target": target,
        "display": display,
        "line": link.location.line,
        "col_start": link.location.col_start,
        "col_end": link.location.col_end,
    })
}

fn backlinks_to_json(results: &[(Note, Vec<LocatedLink>)], vault_path: &Path) -> serde_json::Value {
    serde_json::Value::Array(
        results
            .iter()
            .map(|(note, links)| {
                json!({
                    "source_path": vault_rel_path(&note.path, vault_path),
                    "source_id": &note.id,
                    "links": links.iter().map(backlink_link_to_json).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

fn health_report_to_json(report: &VaultHealthReport, vault_path: &Path) -> serde_json::Value {
    let dup_ids: Vec<serde_json::Value> = report
        .duplicate_ids
        .iter()
        .map(|d| {
            json!({
                "id": d.id,
                "notes": d.notes.iter().map(|n| json!({
                    "path": vault_rel_path(&n.path, vault_path),
                    "backlink_count": n.backlink_count,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();

    let dup_aliases: Vec<serde_json::Value> = report
        .duplicate_aliases
        .iter()
        .map(|d| {
            json!({
                "alias": d.alias,
                "notes": d.notes.iter().map(|n| json!({
                    "path": vault_rel_path(&n.path, vault_path),
                    "backlink_count": n.backlink_count,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();

    let broken_links: Vec<serde_json::Value> = report
        .broken_links
        .iter()
        .map(|b| {
            json!({
                "source_path": vault_rel_path(&b.source_path, vault_path),
                "line": b.line,
                "text": b.text,
            })
        })
        .collect();

    let stranded_notes: Vec<serde_json::Value> = report
        .stranded_notes
        .iter()
        .map(|note| {
            json!({
                "path": vault_rel_path(&note.path, vault_path),
            })
        })
        .collect();

    json!({
        "note_count": report.note_count,
        "has_issues": report.has_issues(),
        "duplicate_ids": dup_ids,
        "duplicate_aliases": dup_aliases,
        "broken_links": broken_links,
        "stranded_notes": stranded_notes,
    })
}

fn list_backlinks_json(vault: &Vault, p: ListBacklinksParams) -> Result<serde_json::Value, rmcp::ErrorData> {
    let target = vault.resolve_note(&p.note).map_err(vault_err)?;
    let mut backlinks = vault.backlinks(&target).map_err(vault_err)?;
    if let Some(sort) = p.sort {
        obsidian_core::search::sort_notes_by(&mut backlinks, |(note, _)| Some(note), &sort.into());
    }
    Ok(backlinks_to_json(&backlinks, vault.path()))
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
        description = "List notes that link to a given note, including the matching links and their locations",
        annotations(read_only_hint = true, destructive_hint = false, open_world_hint = false)
    )]
    async fn list_backlinks(
        &self,
        Parameters(p): Parameters<ListBacklinksParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let vault = vault.lock().unwrap();
            list_backlinks_json(&vault, p)
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
        description = "Append content exactly to the end of a note's body without rewriting frontmatter",
        annotations(read_only_hint = false, destructive_hint = false, open_world_hint = false)
    )]
    async fn append_to_note(
        &self,
        Parameters(p): Parameters<AppendToNoteParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let mut vault = vault.lock().unwrap();
            let note = vault.resolve_note(&p.note).map_err(vault_err)?;
            let appended = vault.append_to_note(&note, &p.content).map_err(vault_err)?;
            note_to_json(&appended, vault.path())
        })
        .await
        .map_err(|e| other_err(e.to_string()))??;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    #[tool(
        description = "Extract a named section or explicit span from a note into a new note",
        annotations(read_only_hint = false, destructive_hint = true, open_world_hint = false)
    )]
    async fn extract_to_note(
        &self,
        Parameters(p): Parameters<ExtractToNoteParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let mut vault = vault.lock().unwrap();
            let note = vault.resolve_note(&p.note).map_err(vault_err)?;
            let selection = extract_selection_from_params(&p)?;
            let extracted = vault
                .extract_to_note(
                    &note,
                    &selection,
                    &p.new_path,
                    p.new_id.as_deref(),
                    p.replace_with.as_deref(),
                )
                .map_err(vault_err)?;
            extract_result_to_json(&extracted, vault.path())
        })
        .await
        .map_err(|e| other_err(e.to_string()))??;

        Ok(CallToolResult::success(vec![Content::text(result.to_string())]))
    }

    #[tool(
        description = "Replace an exact body-text occurrence in a note (must appear exactly once)",
        annotations(read_only_hint = false, destructive_hint = true, open_world_hint = false)
    )]
    async fn patch_note(&self, Parameters(p): Parameters<PatchNoteParams>) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let mut vault = vault.lock().unwrap();
            let note = vault.resolve_note(&p.note).map_err(vault_err)?;
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
            let mut query = vault.search().include_inline_tags();

            for tag in p.tags.unwrap_or_default() {
                query = query.or_has_tag(tag);
            }
            if let Some(title) = p.title_contains {
                query = query.and_title_contains(title);
            }
            if let Some(content) = p.content_contains {
                query = query.or_content_contains(content);
            }
            if let Some(pattern) = p.content_matches {
                query = query.or_content_matches(pattern);
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
        description = "Check vault health: report duplicate IDs, duplicate aliases, broken links, and stranded notes",
        annotations(read_only_hint = true, destructive_hint = false, open_world_hint = false)
    )]
    async fn check_vault(
        &self,
        Parameters(p): Parameters<CheckVaultParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let vault = Arc::clone(&self.vault);
        let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, rmcp::ErrorData> {
            let vault = vault.lock().unwrap();

            let empty: Vec<String> = Vec::new();
            let ignore_patterns = p.ignore.as_deref().unwrap_or(&empty);
            let ignore_set = build_ignore_set(ignore_patterns)?;
            let vault_path = vault.path().to_path_buf();

            let report = vault.check(move |path| {
                let rel = path.strip_prefix(&vault_path).unwrap_or(path);
                !ignore_set.is_match(rel)
            });

            Ok(health_report_to_json(&report, vault.path()))
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
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(SERVER_INSTRUCTIONS)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use super::{
        SERVER_INSTRUCTIONS, VaultHealthReport, extract_result_to_json, health_report_to_json, list_backlinks_json,
    };
    use crate::tools::{ListBacklinksParams, SortOrder};
    use obsidian_core::{ExtractSelection, TextSpan, Vault};

    fn write_note(vault_path: &Path, rel_path: &str, content: &str) {
        let path = vault_path.join(rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn server_instructions_describe_capabilities_and_safe_usage() {
        for expected in [
            "Obsidian vault",
            "Markdown notes",
            "Discover notes",
            "Read note bodies",
            "backlinks",
            "Create notes",
            "extract_to_note",
            "append_to_note",
            "rename notes while updating backlinks",
            "Check vault health",
            "Prefer read-only tools",
            "patch_note",
            "vault-relative paths",
        ] {
            assert!(
                SERVER_INSTRUCTIONS.contains(expected),
                "server instructions should mention {expected:?}"
            );
        }
    }

    #[test]
    fn health_report_json_includes_stranded_notes() {
        let report = VaultHealthReport {
            note_count: 1,
            duplicate_ids: Vec::new(),
            duplicate_aliases: Vec::new(),
            broken_links: Vec::new(),
            stranded_notes: vec![obsidian_core::StrandedNote {
                path: PathBuf::from("/vault/isolated.md"),
            }],
        };

        let value = health_report_to_json(&report, Path::new("/vault"));
        assert_eq!(value["has_issues"], json!(true));
        assert_eq!(value["stranded_notes"], json!([{ "path": "isolated.md" }]));
    }

    #[test]
    fn list_backlinks_json_returns_backlink_matches() {
        let vault_dir = tempfile::tempdir().unwrap();
        write_note(
            vault_dir.path(),
            "target.md",
            "---\nid: target-id\naliases:\n  - Target Alias\n---\nTarget.\n",
        );
        write_note(
            vault_dir.path(),
            "source.md",
            "See [[target-id|Shown]] and [Target](target.md#section).\n",
        );

        let vault = Vault::open(vault_dir.path()).unwrap();
        let result = list_backlinks_json(
            &vault,
            ListBacklinksParams {
                note: "target-id".to_string(),
                sort: Some(SortOrder::PathAsc),
            },
        )
        .unwrap();

        let items = result.as_array().unwrap();
        assert_eq!(items.len(), 1);

        let item = &items[0];
        assert_eq!(item["source_path"], Value::String("source.md".to_string()));
        assert_eq!(item["source_id"], Value::String("source".to_string()));

        let links = item["links"].as_array().unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0]["kind"], Value::String("wiki".to_string()));
        assert_eq!(links[0]["target"], Value::String("target-id".to_string()));
        assert_eq!(links[0]["display"], Value::String("Shown".to_string()));
        assert_eq!(links[0]["line"], Value::from(1));

        assert_eq!(links[1]["kind"], Value::String("markdown".to_string()));
        assert_eq!(links[1]["target"], Value::String("target.md#section".to_string()));
        assert_eq!(links[1]["display"], Value::String("Target".to_string()));
        assert_eq!(links[1]["line"], Value::from(1));
    }

    #[test]
    fn list_backlinks_json_applies_sorting() {
        let vault_dir = tempfile::tempdir().unwrap();
        write_note(
            vault_dir.path(),
            "target.md",
            "---\naliases:\n  - Target Alias\n---\nTarget.\n",
        );
        write_note(vault_dir.path(), "a.md", "See [[Target Alias]].\n");
        write_note(vault_dir.path(), "z.md", "See [Target](target.md).\n");

        let vault = Vault::open(vault_dir.path()).unwrap();
        let result = list_backlinks_json(
            &vault,
            ListBacklinksParams {
                note: "Target Alias".to_string(),
                sort: Some(SortOrder::PathDesc),
            },
        )
        .unwrap();

        let items = result.as_array().unwrap();
        let source_paths: Vec<_> = items
            .iter()
            .map(|item| item["source_path"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(source_paths, vec!["z.md".to_string(), "a.md".to_string()]);
    }

    #[test]
    fn extract_result_json_includes_source_and_new_notes() {
        let vault_dir = tempfile::tempdir().unwrap();
        write_note(vault_dir.path(), "source.md", "Hello world.");

        let mut vault = Vault::open(vault_dir.path()).unwrap();
        let note = vault.resolve_note("source.md").unwrap();
        let result = vault
            .extract_to_note(
                &note,
                &ExtractSelection::Span(TextSpan {
                    start_line: 1,
                    start_col: 6,
                    end_line: 1,
                    end_col: 11,
                }),
                vault_dir.path().join("new.md"),
                None,
                None,
            )
            .unwrap();

        let value = extract_result_to_json(&result, vault.path()).unwrap();
        assert_eq!(value["source_note"]["path"], json!("source.md"));
        assert_eq!(value["new_note"]["path"], json!("new.md"));
        assert_eq!(value["new_note"]["id"], json!("new"));
    }
}
